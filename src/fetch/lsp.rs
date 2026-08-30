//! Minimal stdio LSP client used by the fetch layer.
//!
//! The transport is implemented locally because `tower-lsp` supplies protocol
//! types and server infrastructure, but cgraph needs to act as an LSP client.

mod capabilities;
mod clangd;
mod documents;
mod framing;
mod normalize;
mod profile;
mod progress;
mod pyrefly;
mod settings;
mod symbol_names;

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fmt,
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use tokio::{
    io::{AsyncBufRead, AsyncWrite, BufReader},
    process::{Child, Command},
    sync::{Mutex, OnceCell, mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tower_lsp::lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    ClientInfo, DidOpenTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse,
    InitializeParams, InitializeResult, PartialResultParams, Position, ServerInfo, SymbolKind,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, TypeHierarchyItem,
    TypeHierarchyPrepareParams, TypeHierarchySubtypesParams, TypeHierarchySupertypesParams, Url,
    WorkDoneProgressParams, WorkspaceFolder, WorkspaceSymbolParams, WorkspaceSymbolResponse,
    request::{
        CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
        DocumentSymbolRequest, Initialize, Request, Shutdown, TypeHierarchyPrepare,
        TypeHierarchySubtypes, TypeHierarchySupertypes, WorkspaceSymbolRequest,
    },
};

use crate::{
    config::FilterConfig,
    fetch::{FetchSource, HierarchyQuery, HierarchyResponse, WorkspaceSymbolMatch},
    state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
};
use capabilities::{
    ServerHierarchyCapabilities, call_hierarchy_supported, client_capabilities, hierarchy_name,
    requested_configuration, uses_utf16_positions, workspace_symbol_initialization_options,
    workspace_symbol_supported,
};
use framing::{read_message, write_message};
use normalize::{
    DocumentSymbolOwner, call_item_identity, deduplicate_identities, deduplicate_symbols,
    document_position, find_document_symbol_container, normalize_document_symbols,
    normalize_symbols, symbol_kind_matches_hierarchy, symbol_leaf_name, type_item_identity,
};
use profile::{
    ServerProfile, from_name as server_profile_from_name,
    from_program as server_profile_from_program,
};
use progress::{LspProgressTracker, handle_server_notification};
use symbol_names::SymbolNameAdapter;

pub use progress::LspStatusUpdate;
pub use settings::{LspConfig, builtin_file_extensions};

fn symbol_uri_is_visible(
    symbol: &str,
    uri: &Url,
    workspace_root: &Path,
    filters: &FilterConfig,
) -> bool {
    uri.to_file_path()
        .map(|path| filters.is_visible_symbol_path(symbol, &path, workspace_root))
        .unwrap_or(!filters.workspace_only())
}

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct WorkspaceSymbolClient {
    client: JsonRpcClient,
    workspace_root: PathBuf,
    symbol_names: SymbolNameAdapter,
    filters: FilterConfig,
}

impl fmt::Debug for WorkspaceSymbolClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceSymbolClient")
            .finish_non_exhaustive()
    }
}

impl WorkspaceSymbolClient {
    pub async fn query(&self, query: &str) -> Result<Vec<WorkspaceSymbolMatch>> {
        let started = Instant::now();
        let params = WorkspaceSymbolParams {
            query: query.to_owned(),
            ..WorkspaceSymbolParams::default()
        };
        let response: Option<WorkspaceSymbolResponse> = self
            .client
            .request(WorkspaceSymbolRequest::METHOD, params)
            .await
            .with_context(|| format!("workspace symbol query failed for {query:?}"))?;

        let symbols = response
            .map(|response| normalize_symbols(response, self.symbol_names))
            .unwrap_or_default();
        let received = symbols.len();
        let visible = deduplicate_symbols(symbols.into_iter().filter(|symbol| {
            symbol_uri_is_visible(
                &symbol.name,
                &symbol.uri,
                &self.workspace_root,
                &self.filters,
            )
        }));
        if visible.is_empty() {
            self.client.report_diagnostic(format!(
                "workspace/symbol({query:?}) returned {received} candidate(s), 0 after project filters in {} ms; the server may still be indexing",
                started.elapsed().as_millis()
            ));
        }
        Ok(visible)
    }

    pub fn set_filters(&mut self, filters: FilterConfig) {
        self.filters = filters;
    }
}

#[derive(Clone)]
pub struct HierarchyClient {
    client: JsonRpcClient,
    workspace_root: PathBuf,
    symbol_names: SymbolNameAdapter,
    document_symbols: DocumentSymbolCache,
    capabilities: Arc<ServerHierarchyCapabilities>,
    filters: FilterConfig,
}

type DocumentSymbolCache = Arc<Mutex<HashMap<Url, Arc<OnceCell<Vec<DocumentSymbolOwner>>>>>>;

impl fmt::Debug for HierarchyClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HierarchyClient")
            .finish_non_exhaustive()
    }
}

impl HierarchyClient {
    pub async fn query(&self, mut query: HierarchyQuery) -> Result<HierarchyResponse> {
        if !self.supports(query.symbol.kind) {
            bail!(
                "language server does not advertise {} hierarchy support",
                hierarchy_name(query.symbol.kind)
            );
        }
        let (document_position, resolved_location) =
            self.resolve_document_position(&query.symbol).await?;
        self.client
            .ensure_document_open(&document_position.text_document.uri)
            .await?;
        query.symbol.location = Some(resolved_location);
        let children = match query.symbol.kind {
            HierarchyKind::Call => {
                self.call_children(document_position, query.direction)
                    .await?
            }
            HierarchyKind::Type => {
                self.type_children(document_position, query.direction)
                    .await?
            }
        };

        Ok(HierarchyResponse {
            query,
            children: deduplicate_identities(children),
            source: FetchSource::Lsp,
        })
    }

    pub fn supports(&self, kind: HierarchyKind) -> bool {
        self.capabilities.supports(kind)
    }

    async fn resolve_document_position(
        &self,
        symbol: &SymbolIdentity,
    ) -> Result<(TextDocumentPositionParams, SourceLocation)> {
        if let Some(location) = symbol.location.as_ref()
            && let (Some(line), Some(character)) = (location.line, location.character)
        {
            let uri = Url::parse(&location.uri)
                .with_context(|| format!("invalid symbol URI: {}", location.uri))?;
            return Ok(document_position(uri, Position::new(line, character)));
        }

        let lookup_name = symbol_leaf_name(&symbol.symbol);
        let candidates = WorkspaceSymbolClient {
            client: self.client.clone(),
            workspace_root: self.workspace_root.clone(),
            symbol_names: self.symbol_names,
            filters: self.filters.clone(),
        }
        .query(lookup_name)
        .await?
        .into_iter()
        .filter(|candidate| {
            candidate.range.is_some()
                && symbol_leaf_name(&candidate.name) == lookup_name
                && symbol_kind_matches_hierarchy(symbol.kind, candidate.kind)
        })
        .collect::<Vec<_>>();

        let candidate = match candidates.as_slice() {
            [candidate] => candidate,
            [] => bail!(
                "could not resolve {:?} to a workspace symbol with a source position",
                symbol.symbol
            ),
            _ => bail!(
                "symbol {:?} is ambiguous; add it through ac/at to select an exact location",
                symbol.symbol
            ),
        };
        let position = candidate
            .range
            .expect("workspace symbol candidates were filtered to exact locations")
            .start;
        Ok(document_position(candidate.uri.clone(), position))
    }

    async fn call_children(
        &self,
        document_position: TextDocumentPositionParams,
        direction: HierarchyDirection,
    ) -> Result<Vec<SymbolIdentity>> {
        let prepared: Option<Vec<CallHierarchyItem>> = self
            .client
            .request(
                CallHierarchyPrepare::METHOD,
                CallHierarchyPrepareParams {
                    text_document_position_params: document_position,
                    work_done_progress_params: WorkDoneProgressParams::default(),
                },
            )
            .await
            .context("failed to prepare call hierarchy")?;
        let Some(item) = prepared.and_then(|items| items.into_iter().next()) else {
            return Ok(Vec::new());
        };

        match direction {
            HierarchyDirection::Incoming => {
                let calls: Option<Vec<CallHierarchyIncomingCall>> = self
                    .client
                    .request(
                        CallHierarchyIncomingCalls::METHOD,
                        CallHierarchyIncomingCallsParams {
                            item,
                            work_done_progress_params: WorkDoneProgressParams::default(),
                            partial_result_params: PartialResultParams::default(),
                        },
                    )
                    .await
                    .context("failed to query incoming calls")?;
                self.call_item_identities(
                    calls
                        .unwrap_or_default()
                        .into_iter()
                        .map(|call| call.from)
                        .collect(),
                )
                .await
            }
            HierarchyDirection::Outgoing => {
                let calls: Option<Vec<CallHierarchyOutgoingCall>> = self
                    .client
                    .request(
                        CallHierarchyOutgoingCalls::METHOD,
                        CallHierarchyOutgoingCallsParams {
                            item,
                            work_done_progress_params: WorkDoneProgressParams::default(),
                            partial_result_params: PartialResultParams::default(),
                        },
                    )
                    .await
                    .context("failed to query outgoing calls")?;
                self.call_item_identities(
                    calls
                        .unwrap_or_default()
                        .into_iter()
                        .map(|call| call.to)
                        .collect(),
                )
                .await
            }
        }
    }

    async fn call_item_identities(
        &self,
        items: Vec<CallHierarchyItem>,
    ) -> Result<Vec<SymbolIdentity>> {
        let mut identities = Vec::with_capacity(items.len());
        for item in items.into_iter().filter(|item| {
            symbol_uri_is_visible(&item.name, &item.uri, &self.workspace_root, &self.filters)
        }) {
            let container = if self.symbol_names.uses_document_symbols() {
                self.document_symbol_container(&item).await
            } else {
                None
            };
            identities.push(call_item_identity(
                item,
                self.symbol_names,
                container.as_deref(),
            ));
        }
        Ok(identities)
    }

    async fn document_symbol_container(&self, item: &CallHierarchyItem) -> Option<String> {
        if !matches!(
            item.kind,
            SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::CONSTRUCTOR
        ) {
            return None;
        }

        // rust-analyzer's call hierarchy exposes only a signature in `detail`.
        // The map lock only creates a per-URI cell; the LSP round trip happens
        // outside it, so different documents can resolve concurrently.
        let document_symbols = {
            let mut cache = self.document_symbols.lock().await;
            Arc::clone(
                cache
                    .entry(item.uri.clone())
                    .or_insert_with(|| Arc::new(OnceCell::new())),
            )
        };
        let symbols = document_symbols
            .get_or_init(|| async {
                let response: Option<DocumentSymbolResponse> = self
                    .client
                    .request(
                        DocumentSymbolRequest::METHOD,
                        DocumentSymbolParams {
                            text_document: TextDocumentIdentifier::new(item.uri.clone()),
                            work_done_progress_params: WorkDoneProgressParams::default(),
                            partial_result_params: PartialResultParams::default(),
                        },
                    )
                    .await
                    .ok()
                    .flatten();
                response.map(normalize_document_symbols).unwrap_or_default()
            })
            .await;
        find_document_symbol_container(symbols, item).map(str::to_owned)
    }

    async fn type_children(
        &self,
        document_position: TextDocumentPositionParams,
        direction: HierarchyDirection,
    ) -> Result<Vec<SymbolIdentity>> {
        let prepared: Option<Vec<TypeHierarchyItem>> = self
            .client
            .request(
                TypeHierarchyPrepare::METHOD,
                TypeHierarchyPrepareParams {
                    text_document_position_params: document_position,
                    work_done_progress_params: WorkDoneProgressParams::default(),
                },
            )
            .await
            .context("failed to prepare type hierarchy")?;
        let Some(item) = prepared.and_then(|items| items.into_iter().next()) else {
            return Ok(Vec::new());
        };

        let items: Option<Vec<TypeHierarchyItem>> = match direction {
            HierarchyDirection::Incoming => self
                .client
                .request(
                    TypeHierarchySupertypes::METHOD,
                    TypeHierarchySupertypesParams {
                        item,
                        work_done_progress_params: WorkDoneProgressParams::default(),
                        partial_result_params: PartialResultParams::default(),
                    },
                )
                .await
                .context("failed to query supertypes")?,
            HierarchyDirection::Outgoing => self
                .client
                .request(
                    TypeHierarchySubtypes::METHOD,
                    TypeHierarchySubtypesParams {
                        item,
                        work_done_progress_params: WorkDoneProgressParams::default(),
                        partial_result_params: PartialResultParams::default(),
                    },
                )
                .await
                .context("failed to query subtypes")?,
        };
        Ok(items
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                symbol_uri_is_visible(&item.name, &item.uri, &self.workspace_root, &self.filters)
            })
            .map(type_item_identity)
            .collect())
    }

    pub fn set_filters(&mut self, filters: FilterConfig) {
        self.filters = filters;
    }
}

pub struct LspProvider {
    child: Child,
    client: JsonRpcClient,
    connection_task: JoinHandle<Result<()>>,
    workspace_root: PathBuf,
    server_info: Option<ServerInfo>,
    symbol_names: SymbolNameAdapter,
    document_symbols: DocumentSymbolCache,
    hierarchy_capabilities: Arc<ServerHierarchyCapabilities>,
    status_receiver: Option<mpsc::UnboundedReceiver<LspStatusUpdate>>,
    filters: FilterConfig,
}

impl fmt::Debug for LspProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LspProvider")
            .field("workspace_root", &self.workspace_root)
            .field("server_info", &self.server_info)
            .finish_non_exhaustive()
    }
}

impl LspProvider {
    pub async fn start(config: LspConfig) -> Result<Self> {
        let workspace_root = config.workspace_root.canonicalize().with_context(|| {
            format!(
                "failed to resolve workspace root {}",
                config.workspace_root.display()
            )
        })?;
        if !workspace_root.is_dir() {
            bail!(
                "workspace root is not a directory: {}",
                workspace_root.display()
            );
        }

        let workspace_uri = Url::from_directory_path(&workspace_root).map_err(|()| {
            anyhow::anyhow!(
                "workspace root cannot be represented as a file URI: {}",
                workspace_root.display()
            )
        })?;
        let workspace_name = workspace_name(&workspace_root);
        let stderr_log = config.stderr_log.as_ref().map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                workspace_root.join(path)
            }
        });
        let stderr = match stderr_log.as_ref() {
            Some(path) => {
                let mut options = OpenOptions::new();
                options.create(true).append(true);
                #[cfg(unix)]
                options.mode(0o600);
                Stdio::from(options.open(path).with_context(|| {
                    format!("failed to open language-server log {}", path.display())
                })?)
            }
            None => Stdio::null(),
        };

        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .current_dir(&workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .kill_on_drop(true);

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start language server {}",
                config.program.to_string_lossy()
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .context("language server did not expose stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("language server did not expose stdout")?;
        let (client, status_receiver, connection_task, hierarchy_capabilities) = spawn_json_rpc(
            BufReader::new(stdout),
            stdin,
            workspace_uri.clone(),
            workspace_name.clone(),
        );

        let capabilities = client_capabilities();
        let initialization_options = workspace_symbol_initialization_options(
            &config.program,
            config.server_name.as_deref(),
            config.initialization_options.clone(),
        );
        let initialize_params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(workspace_uri.clone()),
            initialization_options,
            capabilities,
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: workspace_uri,
                name: workspace_name,
            }]),
            client_info: Some(ClientInfo {
                name: env!("CARGO_PKG_NAME").to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            ..InitializeParams::default()
        };

        let initialize_result: InitializeResult = client
            .request(Initialize::METHOD, initialize_params)
            .await
            .context("language server initialization failed")?;
        if !uses_utf16_positions(initialize_result.capabilities.position_encoding.as_ref()) {
            bail!("language server selected a position encoding other than UTF-16");
        }
        if !workspace_symbol_supported(&initialize_result) {
            bail!("language server does not support workspace/symbol");
        }
        hierarchy_capabilities.set_static_call(call_hierarchy_supported(&initialize_result));

        client
            .notify("initialized", json!({}))
            .await
            .context("failed to notify language server that initialization completed")?;

        let advertised_server_name = initialize_result
            .server_info
            .as_ref()
            .map(|info| info.name.as_str());
        let detected_server_name = config.server_name.as_deref().or(advertised_server_name);
        let symbol_names = SymbolNameAdapter::detect(&config.program, detected_server_name);
        let bootstrap_document = if symbol_names.is_pyrefly() {
            match pyrefly::bootstrap_document(&workspace_root, &config.file_extensions) {
                Some(document) => {
                    client
                        .notify(
                            "textDocument/didOpen",
                            DidOpenTextDocumentParams {
                                text_document: TextDocumentItem {
                                    uri: document.uri.clone(),
                                    language_id: "python".to_owned(),
                                    version: 0,
                                    text: document.text,
                                },
                            },
                        )
                        .await
                        .context("failed to open Pyrefly index bootstrap document")?;
                    client.mark_document_open(&document.uri).await;
                    Some(document.uri)
                }
                None => None,
            }
        } else if server_profile_from_program(&config.program) == ServerProfile::Clangd
            || config
                .server_name
                .as_deref()
                .is_some_and(|name| server_profile_from_name(name) == ServerProfile::Clangd)
            || advertised_server_name
                .is_some_and(|name| server_profile_from_name(name) == ServerProfile::Clangd)
        {
            client.enable_document_opening();
            match clangd::bootstrap_document(&workspace_root, &config.file_extensions) {
                Some(document) => {
                    client
                        .notify(
                            "textDocument/didOpen",
                            DidOpenTextDocumentParams {
                                text_document: TextDocumentItem {
                                    uri: document.uri.clone(),
                                    language_id: document.language_id.to_owned(),
                                    version: 0,
                                    text: document.text,
                                },
                            },
                        )
                        .await
                        .context("failed to open clangd index bootstrap document")?;
                    client.mark_document_open(&document.uri).await;
                    Some(document.uri)
                }
                None => None,
            }
        } else {
            None
        };
        let server_label = initialize_result.server_info.as_ref().map_or_else(
            || config.program.to_string_lossy().into_owned(),
            |info| match info.version.as_deref() {
                Some(version) => format!("{} {version}", info.name),
                None => info.name.clone(),
            },
        );
        let bootstrap = bootstrap_document
            .as_ref()
            .map_or_else(|| "none".to_owned(), ToString::to_string);
        let stderr = stderr_log
            .as_ref()
            .map_or_else(|| "disabled".to_owned(), |path| path.display().to_string());
        client.report_diagnostic(format!(
            "LSP initialized: {server_label}; workspace={}; bootstrap={bootstrap}; stderr_log={stderr}",
            workspace_root.display()
        ));
        Ok(Self {
            child,
            client,
            connection_task,
            workspace_root,
            server_info: initialize_result.server_info,
            symbol_names,
            document_symbols: Arc::new(Mutex::new(HashMap::new())),
            hierarchy_capabilities,
            status_receiver: Some(status_receiver),
            filters: config.filters,
        })
    }

    pub fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }

    pub fn take_status_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<LspStatusUpdate>> {
        self.status_receiver.take()
    }

    pub fn workspace_symbol_client(&self) -> WorkspaceSymbolClient {
        // The clone owns only an actor sender. Child process ownership stays in
        // LspProvider, which keeps shutdown deterministic in main.
        WorkspaceSymbolClient {
            client: self.client.clone(),
            workspace_root: self.workspace_root.clone(),
            symbol_names: self.symbol_names,
            filters: self.filters.clone(),
        }
    }

    pub fn hierarchy_client(&self) -> HierarchyClient {
        HierarchyClient {
            client: self.client.clone(),
            workspace_root: self.workspace_root.clone(),
            symbol_names: self.symbol_names,
            document_symbols: Arc::clone(&self.document_symbols),
            capabilities: Arc::clone(&self.hierarchy_capabilities),
            filters: self.filters.clone(),
        }
    }

    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbolMatch>> {
        self.workspace_symbol_client().query(query).await
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let documents_close_result = self.client.close_open_documents().await;
        let shutdown_result = self
            .client
            .request::<_, ()>(Shutdown::METHOD, ())
            .await
            .context("language server shutdown request failed");
        let _ = self.client.notify("exit", Value::Null).await;

        if timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await.is_err() {
            self.child
                .kill()
                .await
                .context("failed to stop language server after shutdown timeout")?;
            self.child
                .wait()
                .await
                .context("failed to reap language server process")?;
        }

        self.connection_task.abort();
        let _ = self.connection_task.await;

        shutdown_result.and(documents_close_result)
    }
}

#[derive(Clone)]
struct JsonRpcClient {
    commands: mpsc::Sender<JsonRpcCommand>,
    cancellations: mpsc::UnboundedSender<u64>,
    status_updates: mpsc::UnboundedSender<LspStatusUpdate>,
    opened_documents: Arc<Mutex<HashSet<Url>>>,
    auto_open_documents: Arc<AtomicBool>,
}

enum JsonRpcCommand {
    Request {
        method: String,
        params: Value,
        started: oneshot::Sender<u64>,
        response: oneshot::Sender<std::result::Result<Value, String>>,
    },
    Notify {
        method: String,
        params: Value,
        response: oneshot::Sender<std::result::Result<(), String>>,
    },
}

struct RequestCancellationGuard {
    request_id: u64,
    cancellations: mpsc::UnboundedSender<u64>,
    armed: bool,
}

impl RequestCancellationGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RequestCancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            // Drop cannot await actor I/O. The unbounded control channel makes
            // cancellation reliable even when the bounded request queue is full.
            let _ = self.cancellations.send(self.request_id);
        }
    }
}

impl JsonRpcClient {
    fn report_diagnostic(&self, message: impl Into<String>) {
        let _ = self
            .status_updates
            .send(LspStatusUpdate::Diagnostic(message.into()));
    }

    async fn request<P, T>(&self, method: &str, params: P) -> Result<T>
    where
        P: Serialize,
        T: DeserializeOwned,
    {
        let params = serde_json::to_value(params)
            .with_context(|| format!("failed to encode parameters for LSP request {method}"))?;
        let (started_sender, started_receiver) = oneshot::channel();
        let (response_sender, response_receiver) = oneshot::channel();
        self.commands
            .send(JsonRpcCommand::Request {
                method: method.to_owned(),
                params,
                started: started_sender,
                response: response_sender,
            })
            .await
            .map_err(|_| anyhow::anyhow!("LSP connection closed before request {method}"))?;
        let request_id = started_receiver.await.map_err(|_| {
            anyhow::anyhow!("LSP connection closed while starting request {method}")
        })?;
        let mut cancellation_guard = RequestCancellationGuard {
            request_id,
            cancellations: self.cancellations.clone(),
            armed: true,
        };
        let response = response_receiver
            .await
            .map_err(|_| anyhow::anyhow!("LSP connection closed during request {method}"))?
            .map_err(anyhow::Error::msg)?;
        cancellation_guard.disarm();

        serde_json::from_value(response)
            .with_context(|| format!("invalid response to LSP request {method}"))
    }

    async fn notify<P>(&self, method: &str, params: P) -> Result<()>
    where
        P: Serialize,
    {
        let params = serde_json::to_value(params).with_context(|| {
            format!("failed to encode parameters for LSP notification {method}")
        })?;
        let (response_sender, response_receiver) = oneshot::channel();
        self.commands
            .send(JsonRpcCommand::Notify {
                method: method.to_owned(),
                params,
                response: response_sender,
            })
            .await
            .map_err(|_| anyhow::anyhow!("LSP connection closed before notification {method}"))?;
        response_receiver
            .await
            .map_err(|_| anyhow::anyhow!("LSP connection closed during notification {method}"))?
            .map_err(anyhow::Error::msg)
    }
}

fn spawn_json_rpc<R, W>(
    reader: R,
    writer: W,
    workspace_uri: Url,
    workspace_name: String,
) -> (
    JsonRpcClient,
    mpsc::UnboundedReceiver<LspStatusUpdate>,
    JoinHandle<Result<()>>,
    Arc<ServerHierarchyCapabilities>,
)
where
    R: AsyncBufRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    // The reader must run even while no user request is active: servers commonly
    // send workspace/configuration immediately after initialization and may wait
    // for its response before indexing. The actor is the sole stdin writer so
    // concurrent client requests and server-request replies cannot interleave.
    let (command_sender, command_receiver) = mpsc::channel(32);
    let (cancellation_sender, cancellation_receiver) = mpsc::unbounded_channel();
    let (status_sender, status_receiver) = mpsc::unbounded_channel();
    let (incoming_sender, incoming_receiver) = mpsc::channel(64);
    let opened_documents = Arc::new(Mutex::new(HashSet::new()));
    let hierarchy_capabilities = Arc::new(ServerHierarchyCapabilities::default());
    let reader_task = tokio::spawn(read_messages(reader, incoming_sender));
    let actor_hierarchy_capabilities = Arc::clone(&hierarchy_capabilities);
    let server_context = LspServerContext {
        workspace_uri,
        workspace_name,
        hierarchy_capabilities: actor_hierarchy_capabilities,
    };
    let actor_status_sender = status_sender.clone();
    let connection_task = tokio::spawn(async move {
        let result = run_json_rpc(
            writer,
            command_receiver,
            cancellation_receiver,
            incoming_receiver,
            actor_status_sender,
            server_context,
        )
        .await;
        reader_task.abort();
        let _ = reader_task.await;
        result
    });

    let client = JsonRpcClient {
        commands: command_sender,
        cancellations: cancellation_sender,
        status_updates: status_sender,
        opened_documents,
        auto_open_documents: Arc::new(AtomicBool::new(false)),
    };
    (
        client,
        status_receiver,
        connection_task,
        hierarchy_capabilities,
    )
}

struct LspServerContext {
    workspace_uri: Url,
    workspace_name: String,
    hierarchy_capabilities: Arc<ServerHierarchyCapabilities>,
}

async fn read_messages<R>(mut reader: R, sender: mpsc::Sender<std::result::Result<Value, String>>)
where
    R: AsyncBufRead + Unpin,
{
    loop {
        match read_message(&mut reader).await {
            Ok(message) => {
                if sender.send(Ok(message)).await.is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error.to_string())).await;
                break;
            }
        }
    }
}

async fn run_json_rpc<W>(
    mut writer: W,
    mut commands: mpsc::Receiver<JsonRpcCommand>,
    mut cancellations: mpsc::UnboundedReceiver<u64>,
    mut incoming: mpsc::Receiver<std::result::Result<Value, String>>,
    status_sender: mpsc::UnboundedSender<LspStatusUpdate>,
    server_context: LspServerContext,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut next_request_id = 1_u64;
    let mut pending = HashMap::new();
    let mut progress_tracker = LspProgressTracker::default();

    let connection_result = loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    break Ok(());
                };
                match command {
                    JsonRpcCommand::Request { method, params, started, response } => {
                        let request_id = next_request_id;
                        next_request_id += 1;
                        let message = json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "method": method,
                            "params": params,
                        });
                        if let Err(error) = write_message(&mut writer, &message).await {
                            let _ = response.send(Err(error.to_string()));
                            break Err(error);
                        }
                        pending.insert(request_id, response);
                        if started.send(request_id).is_err() {
                            pending.remove(&request_id);
                            write_cancel_request(&mut writer, request_id).await?;
                        }
                    }
                    JsonRpcCommand::Notify { method, params, response } => {
                        let message = json!({
                            "jsonrpc": "2.0",
                            "method": method,
                            "params": params,
                        });
                        match write_message(&mut writer, &message).await {
                            Ok(()) => {
                                let _ = response.send(Ok(()));
                            }
                            Err(error) => {
                                let _ = response.send(Err(error.to_string()));
                                break Err(error);
                            }
                        }
                    }
                }
            }
            request_id = cancellations.recv() => {
                let Some(request_id) = request_id else {
                    break Ok(());
                };
                if pending.remove(&request_id).is_some() {
                    write_cancel_request(&mut writer, request_id).await?;
                }
            }
            message = incoming.recv() => {
                let Some(message) = message else {
                    break Err(anyhow::anyhow!("LSP message reader stopped unexpectedly"));
                };
                let message = match message {
                    Ok(message) => message,
                    Err(error) => break Err(anyhow::Error::msg(error)),
                };

                if let Some(request_id) = response_id(&message) {
                    if let Some(response) = pending.remove(&request_id) {
                        let result = match message.get("error") {
                            Some(error) if !error.is_null() => Err(format!(
                                "LSP request failed: {error}"
                            )),
                            _ => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = response.send(result);
                    }
                } else if message.get("method").is_some() {
                    handle_server_notification(
                        &message,
                        &mut progress_tracker,
                        &status_sender,
                    );
                    if let Err(error) = handle_server_message(
                            &mut writer,
                            &message,
                            &server_context.workspace_uri,
                            &server_context.workspace_name,
                            &server_context.hierarchy_capabilities,
                        )
                        .await
                    {
                        break Err(error);
                    }
                }
            }
        }
    };

    let failure = connection_result
        .as_ref()
        .err()
        .map_or_else(|| "LSP connection closed".to_owned(), ToString::to_string);
    let _ = status_sender.send(LspStatusUpdate::Disconnected(failure.clone()));
    for (_, response) in pending {
        let _ = response.send(Err(failure.clone()));
    }

    connection_result
}

async fn write_cancel_request<W>(writer: &mut W, request_id: u64) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_message(
        writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": request_id },
        }),
    )
    .await
}

async fn handle_server_message<W>(
    writer: &mut W,
    message: &Value,
    workspace_uri: &Url,
    workspace_name: &str,
    hierarchy_capabilities: &ServerHierarchyCapabilities,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let Some(id) = message.get("id").cloned() else {
        return Ok(());
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .context("LSP server request has no method")?;

    let response = match method {
        "workspace/configuration" => {
            let values = message
                .pointer("/params/items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            requested_configuration(item.get("section").and_then(Value::as_str))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": values,
            })
        }
        "workspace/workspaceFolders" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": [{
                "uri": workspace_uri,
                "name": workspace_name,
            }],
        }),
        "client/registerCapability" => {
            register_hierarchy_capabilities(message, hierarchy_capabilities);
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": null,
            })
        }
        "client/unregisterCapability" => {
            unregister_hierarchy_capabilities(message, hierarchy_capabilities);
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": null,
            })
        }
        "window/workDoneProgress/create" | "window/showMessageRequest" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": null,
        }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("cgraph does not implement {method}"),
            },
        }),
    };

    write_message(writer, &response).await
}

fn register_hierarchy_capabilities(message: &Value, capabilities: &ServerHierarchyCapabilities) {
    let Some(registrations) = message
        .pointer("/params/registrations")
        .and_then(Value::as_array)
    else {
        return;
    };
    for registration in registrations {
        let Some(id) = registration.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(method) = registration.get("method").and_then(Value::as_str) else {
            continue;
        };
        capabilities.register(id, method);
    }
}

fn unregister_hierarchy_capabilities(message: &Value, capabilities: &ServerHierarchyCapabilities) {
    let Some(unregistrations) = message
        .pointer("/params/unregistrations")
        .or_else(|| message.pointer("/params/unregisterations"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for unregistration in unregistrations {
        if let Some(id) = unregistration.get("id").and_then(Value::as_str) {
            capabilities.unregister(id);
        }
    }
}

fn workspace_name(workspace_root: &Path) -> String {
    workspace_root
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("workspace")
        .to_owned()
}

fn response_id(message: &Value) -> Option<u64> {
    message.get("id").and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        fs,
        path::{Path, PathBuf},
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use crate::config::FilterConfig;
    use serde_json::{Value, json};
    use tokio::io::{BufReader, duplex, split};
    use tokio::time::timeout;
    use tower_lsp::lsp_types::{
        SymbolKind, Url, WorkspaceSymbolParams, WorkspaceSymbolResponse,
        request::{DocumentSymbolRequest, Request},
    };

    use super::symbol_names::SymbolNameAdapter;
    use super::{
        HierarchyClient, LspConfig, LspProgressTracker, LspProvider, LspStatusUpdate,
        WorkspaceSymbolClient, client_capabilities, deduplicate_symbols,
        handle_server_notification, normalize_symbols, read_message, requested_configuration,
        response_id, spawn_json_rpc, symbol_leaf_name, uses_utf16_positions,
        workspace_symbol_initialization_options, write_message,
    };
    use crate::fetch::treesitter::{TreeSitterLanguage, TreeSitterProvider};
    use crate::{
        fetch::{
            FetchSource, HierarchyClient as FetchHierarchyClient, HierarchyQuery,
            WorkspaceSymbolMatch,
        },
        state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
    };

    #[test]
    fn configures_rust_analyzer_for_project_only_all_symbol_queries() {
        assert_eq!(
            requested_configuration(Some("rust-analyzer.workspace.symbol.search.kind")),
            json!("all_symbols")
        );
        assert_eq!(
            requested_configuration(Some("rust-analyzer.workspace.symbol.search.scope")),
            json!("workspace")
        );
        assert_eq!(
            requested_configuration(Some("rust-analyzer.workspace.symbol.search.limit")),
            Value::Null
        );
        assert_eq!(requested_configuration(Some("clangd")), Value::Null);
        assert_eq!(requested_configuration(Some("python")), json!({}));

        let options = workspace_symbol_initialization_options(
            OsStr::new("rust-analyzer"),
            None,
            Some(json!({ "cargo": { "features": "all" } })),
        )
        .unwrap();
        assert_eq!(options["cargo"]["features"], "all");
        assert_eq!(
            options["workspace"]["symbol"]["search"]["kind"],
            "all_symbols"
        );
        assert_eq!(
            options["workspace"]["symbol"]["search"]["scope"],
            "workspace"
        );
        assert!(
            options["workspace"]["symbol"]["search"]
                .get("limit")
                .is_none()
        );

        assert_eq!(
            workspace_symbol_initialization_options(
                OsStr::new("clangd"),
                None,
                Some(json!({ "clangd": true })),
            ),
            Some(json!({ "clangd": true }))
        );
        let wrapper = LspConfig::for_server("/opt/tools/lsp-wrapper", "/workspace")
            .server_name("rust-analyzer");
        assert_eq!(
            workspace_symbol_initialization_options(
                &wrapper.program,
                wrapper.server_name.as_deref(),
                None,
            )
            .unwrap()["workspace"]["symbol"]["search"]["kind"],
            "all_symbols"
        );
        let pyrefly_wrapper =
            LspConfig::for_server("/opt/tools/lsp-wrapper", "/workspace").server_name("pyrefly");
        assert_eq!(pyrefly_wrapper.args, ["lsp"].map(std::ffi::OsString::from));
        assert!(
            !LspConfig::for_server("clangd", "/workspace")
                .filters(FilterConfig::from_rules(std::iter::empty::<&str>(), false).unwrap())
                .filters
                .workspace_only()
        );
        assert_eq!(
            LspConfig::for_server("/usr/bin/clangd", "/workspace").file_extensions,
            ["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"]
        );
        assert_eq!(
            LspConfig::for_server("/usr/bin/clangd", "/workspace").args,
            [std::ffi::OsString::from("--background-index")]
        );
    }

    #[test]
    fn configures_pyrefly_command_and_python_symbol_leaf_names() {
        let config = LspConfig::for_server("/tools/pyrefly.exe", "/workspace")
            .arg("--indexing-mode")
            .arg("lazy-blocking");
        assert_eq!(config.program, OsStr::new("/tools/pyrefly.exe"));
        assert_eq!(
            config.args,
            ["lsp", "--indexing-mode", "lazy-blocking"].map(std::ffi::OsString::from)
        );
        assert!(LspConfig::for_server("pylsp", "/workspace").args.is_empty());
        assert_eq!(symbol_leaf_name("Worker.run"), "run");
        assert_eq!(symbol_leaf_name("Worker::run"), "run");
        assert_eq!(symbol_leaf_name("run"), "run");
    }

    #[test]
    fn negotiates_only_utf16_source_positions() {
        let capabilities = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(
            capabilities["general"]["positionEncodings"],
            json!(["utf-16"])
        );
        assert_eq!(
            capabilities["textDocument"]["callHierarchy"]["dynamicRegistration"],
            json!(true)
        );
        assert_eq!(
            capabilities["textDocument"]["typeHierarchy"]["dynamicRegistration"],
            json!(true)
        );
        assert!(uses_utf16_positions(None));
        assert!(uses_utf16_positions(Some(
            &tower_lsp::lsp_types::PositionEncodingKind::UTF16
        )));
        assert!(!uses_utf16_positions(Some(
            &tower_lsp::lsp_types::PositionEncodingKind::UTF8
        )));
    }

    #[test]
    fn deduplicates_identical_workspace_symbols() {
        let duplicate = symbol("file:///workspace/src/main.rs");
        assert_eq!(
            deduplicate_symbols([duplicate.clone(), duplicate.clone(), duplicate]),
            vec![symbol("file:///workspace/src/main.rs")]
        );
    }

    #[tokio::test]
    async fn reports_empty_workspace_symbol_query_diagnostics() {
        let (client_stream, server_stream) = duplex(8 * 1024);
        let (client_reader, client_writer) = split(client_stream);
        let (server_reader, mut server_writer) = split(server_stream);
        let workspace_uri = Url::parse("file:///workspace").unwrap();
        let (rpc_client, mut status_receiver, connection_task, _capabilities) = spawn_json_rpc(
            BufReader::new(client_reader),
            client_writer,
            workspace_uri,
            "workspace".to_owned(),
        );
        let client = WorkspaceSymbolClient {
            client: rpc_client.clone(),
            workspace_root: PathBuf::from("/workspace"),
            symbol_names: SymbolNameAdapter::Standard,
            filters: FilterConfig::default(),
        };
        let query = tokio::spawn(async move { client.query("missing").await.unwrap() });
        let mut server_reader = BufReader::new(server_reader);
        let request = read_message(&mut server_reader).await.unwrap();
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": response_id(&request).unwrap(),
                "result": []
            }),
        )
        .await
        .unwrap();

        assert!(query.await.unwrap().is_empty());
        let diagnostic = status_receiver.recv().await.unwrap();
        let LspStatusUpdate::Diagnostic(message) = diagnostic else {
            panic!("expected workspace-symbol diagnostic");
        };
        assert!(message.contains("workspace/symbol(\"missing\") returned 0 candidate(s)"));
        assert!(message.contains("server may still be indexing"));

        drop(rpc_client);
        connection_task.abort();
        let _ = connection_task.await;
    }

    #[tokio::test]
    async fn prepares_and_queries_outgoing_call_hierarchy() {
        let (client_stream, server_stream) = duplex(8 * 1024);
        let (client_reader, client_writer) = split(client_stream);
        let (server_reader, mut server_writer) = split(server_stream);
        let workspace_uri = Url::parse("file:///workspace").unwrap();
        let (rpc_client, _status_receiver, connection_task, capabilities) = spawn_json_rpc(
            BufReader::new(client_reader),
            client_writer,
            workspace_uri,
            "workspace".to_owned(),
        );
        capabilities.set_static_call(true);
        let hierarchy_client = HierarchyClient {
            client: rpc_client.clone(),
            workspace_root: PathBuf::from("/workspace"),
            symbol_names: SymbolNameAdapter::RustAnalyzer,
            document_symbols: Default::default(),
            capabilities,
            filters: FilterConfig::default(),
        };
        let query = HierarchyQuery {
            symbol: SymbolIdentity {
                symbol: "root".to_owned(),
                kind: HierarchyKind::Call,
                location: Some(SourceLocation {
                    uri: "file:///workspace/src/main.rs".to_owned(),
                    line: Some(4),
                    character: Some(3),
                }),
            },
            direction: HierarchyDirection::Outgoing,
        };
        let client_task = tokio::spawn(async move { hierarchy_client.query(query).await.unwrap() });
        let mut server_reader = BufReader::new(server_reader);

        let prepare = read_message(&mut server_reader).await.unwrap();
        assert_eq!(prepare["method"], "textDocument/prepareCallHierarchy");
        assert_eq!(
            prepare["params"]["position"],
            json!({ "line": 4, "character": 3 })
        );
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": response_id(&prepare).unwrap(),
                "result": [call_item("root", 4)]
            }),
        )
        .await
        .unwrap();

        let outgoing = read_message(&mut server_reader).await.unwrap();
        assert_eq!(outgoing["method"], "callHierarchy/outgoingCalls");
        assert_eq!(outgoing["params"]["item"]["name"], "root");
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": response_id(&outgoing).unwrap(),
                "result": [
                    { "to": rust_method_item("child", 8), "fromRanges": [] },
                    { "to": rust_method_item("child", 8), "fromRanges": [] },
                    { "to": external_call_item("printf", 12), "fromRanges": [] }
                ]
            }),
        )
        .await
        .unwrap();

        let document_symbols = read_message(&mut server_reader).await.unwrap();
        assert_eq!(document_symbols["method"], DocumentSymbolRequest::METHOD);
        assert_eq!(
            document_symbols["params"]["textDocument"]["uri"],
            "file:///workspace/src/main.rs"
        );
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": response_id(&document_symbols).unwrap(),
                "result": [{
                    "name": "child",
                    "kind": 12,
                    "location": {
                        "uri": "file:///workspace/src/main.rs",
                        "range": {
                            "start": { "line": 8, "character": 0 },
                            "end": { "line": 10, "character": 1 }
                        }
                    },
                    "containerName": "impl Worker"
                }]
            }),
        )
        .await
        .unwrap();

        let response = client_task.await.unwrap();
        assert_eq!(response.source, FetchSource::Lsp);
        assert_eq!(response.children.len(), 1);
        assert_eq!(response.children[0].symbol, "Worker::child");
        assert_eq!(response.children[0].kind, HierarchyKind::Call);
        assert!(
            !response
                .children
                .iter()
                .any(|child| child.symbol == "printf")
        );
        assert_eq!(
            response.children[0].location.as_ref().unwrap().line,
            Some(8)
        );

        drop(rpc_client);
        connection_task.abort();
        let _ = connection_task.await;
    }

    #[tokio::test]
    async fn prepares_and_queries_type_supertypes() {
        let (client_stream, server_stream) = duplex(8 * 1024);
        let (client_reader, client_writer) = split(client_stream);
        let (server_reader, mut server_writer) = split(server_stream);
        let workspace_uri = Url::parse("file:///workspace").unwrap();
        let (rpc_client, _status_receiver, connection_task, capabilities) = spawn_json_rpc(
            BufReader::new(client_reader),
            client_writer,
            workspace_uri,
            "workspace".to_owned(),
        );
        let mut server_reader = BufReader::new(server_reader);
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": "register-type-hierarchy",
                "method": "client/registerCapability",
                "params": {
                    "registrations": [{
                        "id": "type-hierarchy",
                        "method": "textDocument/prepareTypeHierarchy",
                        "registerOptions": {}
                    }]
                }
            }),
        )
        .await
        .unwrap();
        let registration = read_message(&mut server_reader).await.unwrap();
        assert_eq!(registration["id"], "register-type-hierarchy");
        assert!(capabilities.supports(HierarchyKind::Type));
        let hierarchy_client = HierarchyClient {
            client: rpc_client.clone(),
            workspace_root: PathBuf::from("/workspace"),
            symbol_names: SymbolNameAdapter::Standard,
            document_symbols: Default::default(),
            capabilities: Arc::clone(&capabilities),
            filters: FilterConfig::default(),
        };
        let query = HierarchyQuery {
            symbol: SymbolIdentity {
                symbol: "Child".to_owned(),
                kind: HierarchyKind::Type,
                location: Some(SourceLocation {
                    uri: "file:///workspace/src/main.rs".to_owned(),
                    line: Some(10),
                    character: Some(7),
                }),
            },
            direction: HierarchyDirection::Incoming,
        };
        let client_task = tokio::spawn(async move { hierarchy_client.query(query).await.unwrap() });
        let prepare = read_message(&mut server_reader).await.unwrap();
        assert_eq!(prepare["method"], "textDocument/prepareTypeHierarchy");
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": response_id(&prepare).unwrap(),
                "result": [type_item("Child", 10)]
            }),
        )
        .await
        .unwrap();

        let supertypes = read_message(&mut server_reader).await.unwrap();
        assert_eq!(supertypes["method"], "typeHierarchy/supertypes");
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": response_id(&supertypes).unwrap(),
                "result": [type_item("Parent", 2)]
            }),
        )
        .await
        .unwrap();

        let response = client_task.await.unwrap();
        assert_eq!(response.children.len(), 1);
        assert_eq!(response.children[0].symbol, "Parent");
        assert_eq!(response.children[0].kind, HierarchyKind::Type);

        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": "unregister-type-hierarchy",
                "method": "client/unregisterCapability",
                "params": {
                    "unregistrations": [{
                        "id": "type-hierarchy",
                        "method": "textDocument/prepareTypeHierarchy"
                    }]
                }
            }),
        )
        .await
        .unwrap();
        let unregistration = read_message(&mut server_reader).await.unwrap();
        assert_eq!(unregistration["id"], "unregister-type-hierarchy");
        assert!(!capabilities.supports(HierarchyKind::Type));

        drop(rpc_client);
        connection_task.abort();
        let _ = connection_task.await;
    }

    #[tokio::test]
    async fn falls_back_to_tree_sitter_for_unregistered_type_hierarchy() {
        let workspace = external_server_workspace("type-fallback");
        fs::write(
            workspace.join("lib.rs"),
            "trait Command {}\nstruct Cli;\nimpl Command for Cli {}\n",
        )
        .unwrap();
        let tree_sitter = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Rust).unwrap();
        let symbols = tree_sitter
            .workspace_symbol_client()
            .query("")
            .await
            .unwrap();
        let cli = symbols.iter().find(|symbol| symbol.name == "Cli").unwrap();
        let position = cli.range.unwrap().start;

        let (client_stream, server_stream) = duplex(8 * 1024);
        let (client_reader, client_writer) = split(client_stream);
        let (server_reader, _server_writer) = split(server_stream);
        let workspace_uri = Url::from_directory_path(&workspace).unwrap();
        let (rpc_client, _status_receiver, connection_task, capabilities) = spawn_json_rpc(
            BufReader::new(client_reader),
            client_writer,
            workspace_uri,
            workspace
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        let lsp = HierarchyClient {
            client: rpc_client.clone(),
            workspace_root: workspace.clone(),
            symbol_names: SymbolNameAdapter::RustAnalyzer,
            document_symbols: Default::default(),
            capabilities,
            filters: FilterConfig::default(),
        };
        let hybrid = FetchHierarchyClient::with_fallback(lsp, tree_sitter.hierarchy_client());

        let response = hybrid
            .query(HierarchyQuery {
                symbol: SymbolIdentity {
                    symbol: "Cli".to_owned(),
                    kind: HierarchyKind::Type,
                    location: Some(SourceLocation {
                        uri: cli.uri.to_string(),
                        line: Some(position.line),
                        character: Some(position.character),
                    }),
                },
                direction: HierarchyDirection::Incoming,
            })
            .await
            .unwrap();

        assert_eq!(response.source, FetchSource::TreeSitter);
        assert_eq!(
            response
                .children
                .iter()
                .map(|child| child.symbol.as_str())
                .collect::<Vec<_>>(),
            ["Command"]
        );
        let mut server_reader = BufReader::new(server_reader);
        assert!(
            timeout(Duration::from_millis(20), read_message(&mut server_reader))
                .await
                .is_err(),
            "unsupported type hierarchy must not reach the LSP server"
        );

        drop(rpc_client);
        connection_task.abort();
        let _ = connection_task.await;
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn tracks_work_done_progress_until_the_last_operation_ends() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut tracker = LspProgressTracker::default();

        handle_server_notification(
            &json!({
                "method": "$/progress",
                "params": {
                    "token": "index",
                    "value": {
                        "kind": "begin",
                        "title": "Indexing",
                        "message": "1/2 crates",
                        "percentage": 50
                    }
                }
            }),
            &mut tracker,
            &sender,
        );
        assert_eq!(
            receiver.try_recv().unwrap(),
            LspStatusUpdate::Progress {
                title: "Indexing".to_owned(),
                message: Some("1/2 crates".to_owned()),
                percentage: Some(50),
            }
        );

        handle_server_notification(
            &json!({
                "method": "$/progress",
                "params": {
                    "token": "index",
                    "value": { "kind": "end", "message": "Indexed" }
                }
            }),
            &mut tracker,
            &sender,
        );
        assert_eq!(
            receiver.try_recv().unwrap(),
            LspStatusUpdate::Ready {
                message: Some("Indexed".to_owned())
            }
        );
    }

    #[test]
    fn translates_rust_analyzer_server_status() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut tracker = LspProgressTracker::default();
        handle_server_notification(
            &json!({
                "method": "experimental/serverStatus",
                "params": {
                    "health": "warning",
                    "quiescent": true,
                    "message": "proc macro unavailable"
                }
            }),
            &mut tracker,
            &sender,
        );

        assert_eq!(
            receiver.try_recv().unwrap(),
            LspStatusUpdate::Warning("proc macro unavailable".to_owned())
        );
    }

    #[tokio::test]
    async fn handles_server_requests_while_waiting_for_symbols() {
        let (client_stream, server_stream) = duplex(8 * 1024);
        let (client_reader, client_writer) = split(client_stream);
        let (server_reader, mut server_writer) = split(server_stream);
        let workspace_uri = Url::parse("file:///workspace").unwrap();
        let (client, _status_receiver, connection_task, _capabilities) = spawn_json_rpc(
            BufReader::new(client_reader),
            client_writer,
            workspace_uri,
            "workspace".to_owned(),
        );

        let mut server_reader = BufReader::new(server_reader);
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": "server-request",
                "method": "workspace/configuration",
                "params": { "items": [{}, {}] },
            }),
        )
        .await
        .unwrap();
        let configuration_response = read_message(&mut server_reader).await.unwrap();
        assert_eq!(configuration_response["id"], "server-request");
        assert_eq!(configuration_response["result"], json!([null, null]));

        let query_client = client.clone();
        let client_task = tokio::spawn(async move {
            let response: Option<WorkspaceSymbolResponse> = query_client
                .request(
                    "workspace/symbol",
                    WorkspaceSymbolParams {
                        query: "run".to_owned(),
                        ..WorkspaceSymbolParams::default()
                    },
                )
                .await
                .unwrap();
            normalize_symbols(response.unwrap(), SymbolNameAdapter::Standard)
        });

        let request = read_message(&mut server_reader).await.unwrap();
        assert_eq!(request["method"], "workspace/symbol");
        assert_eq!(request["params"]["query"], "run");
        let request_id = response_id(&request).unwrap();

        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "method": "window/logMessage",
                "params": { "type": 3, "message": "indexed" },
            }),
        )
        .await
        .unwrap();
        write_message(
            &mut server_writer,
            &json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": [{
                    "name": "run",
                    "kind": 12,
                    "location": {
                        "uri": "file:///workspace/src/main.rs",
                        "range": {
                            "start": { "line": 4, "character": 3 },
                            "end": { "line": 4, "character": 6 }
                        }
                    },
                    "containerName": "App"
                }]
            }),
        )
        .await
        .unwrap();

        let symbols = client_task.await.unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "run");
        assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
        assert_eq!(symbols[0].container_name.as_deref(), Some("App"));
        assert_eq!(symbols[0].uri.as_str(), "file:///workspace/src/main.rs");
        assert_eq!(symbols[0].range.unwrap().start.line, 4);

        drop(client);
        connection_task.abort();
        let _ = connection_task.await;
    }

    #[tokio::test]
    async fn cancels_an_lsp_request_when_its_future_is_dropped() {
        let (client_stream, server_stream) = duplex(8 * 1024);
        let (client_reader, client_writer) = split(client_stream);
        let (server_reader, _server_writer) = split(server_stream);
        let workspace_uri = Url::parse("file:///workspace").unwrap();
        let (client, _status_receiver, connection_task, _capabilities) = spawn_json_rpc(
            BufReader::new(client_reader),
            client_writer,
            workspace_uri,
            "workspace".to_owned(),
        );
        let mut server_reader = BufReader::new(server_reader);

        let query_client = client.clone();
        let client_task = tokio::spawn(async move {
            let _: Option<WorkspaceSymbolResponse> = query_client
                .request(
                    "workspace/symbol",
                    WorkspaceSymbolParams {
                        query: "first".to_owned(),
                        ..WorkspaceSymbolParams::default()
                    },
                )
                .await
                .unwrap();
        });

        let request = read_message(&mut server_reader).await.unwrap();
        let request_id = response_id(&request).unwrap();
        client_task.abort();
        let _ = client_task.await;

        let cancellation = timeout(Duration::from_secs(1), read_message(&mut server_reader))
            .await
            .expect("client did not send $/cancelRequest")
            .unwrap();
        assert_eq!(cancellation["method"], "$/cancelRequest");
        assert_eq!(cancellation["params"]["id"], request_id);

        drop(client);
        connection_task.abort();
        let _ = connection_task.await;
    }

    #[tokio::test]
    async fn integrates_with_installed_pyrefly() {
        if !external_server_available("pyrefly").await {
            eprintln!("skipping real Pyrefly integration test: pyrefly is not in PATH");
            return;
        }

        let workspace = external_server_workspace("pyrefly");
        fs::write(workspace.join("pyrefly.toml"), "").unwrap();
        fs::write(
            workspace.join("main.py"),
            "class Base:\n    pass\n\n\nclass Worker(Base):\n    def run(self) -> None:\n        helper()\n\n\ndef helper() -> None:\n    pass\n\n\ndef main() -> None:\n    Worker().run()\n",
        )
        .unwrap();

        let lsp = timeout(
            Duration::from_secs(30),
            LspProvider::start(
                LspConfig::for_server("pyrefly", &workspace)
                    .arg("--indexing-mode")
                    .arg("lazy-blocking"),
            ),
        )
        .await
        .expect("Pyrefly initialization timed out")
        .unwrap();
        assert_eq!(
            lsp.server_info().map(|info| info.name.as_str()),
            Some("pyrefly-lsp")
        );

        let helper = wait_for_workspace_symbol(&lsp, "helper").await;
        let position = helper.range.unwrap().start;
        let response = timeout(
            Duration::from_secs(30),
            lsp.hierarchy_client().query(HierarchyQuery {
                symbol: SymbolIdentity {
                    symbol: helper.name,
                    kind: HierarchyKind::Call,
                    location: Some(SourceLocation {
                        uri: helper.uri.to_string(),
                        line: Some(position.line),
                        character: Some(position.character),
                    }),
                },
                direction: HierarchyDirection::Incoming,
            }),
        )
        .await
        .expect("Pyrefly call hierarchy timed out")
        .unwrap();
        assert!(
            response
                .children
                .iter()
                .any(|child| child.symbol == "Worker.run")
        );

        let tree_sitter = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Python)
            .expect("Python Tree-sitter fallback failed to initialize");
        let worker = tree_sitter
            .workspace_symbol_client()
            .query("Worker")
            .await
            .unwrap()
            .into_iter()
            .find(|symbol| symbol.name == "Worker")
            .expect("Tree-sitter did not index Worker");
        let worker_position = worker.range.expect("Worker has no source range").start;
        let types = FetchHierarchyClient::with_fallback(
            lsp.hierarchy_client(),
            tree_sitter.hierarchy_client(),
        )
        .query(HierarchyQuery {
            symbol: SymbolIdentity {
                symbol: worker.name,
                kind: HierarchyKind::Type,
                location: Some(SourceLocation {
                    uri: worker.uri.to_string(),
                    line: Some(worker_position.line),
                    character: Some(worker_position.character),
                }),
            },
            direction: HierarchyDirection::Incoming,
        })
        .await
        .expect("Pyrefly type hierarchy or Tree-sitter fallback failed");
        assert!(types.children.iter().any(|child| child.symbol == "Base"));

        lsp.shutdown().await.unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn integrates_with_installed_rust_analyzer() {
        if !external_server_available("rust-analyzer").await {
            eprintln!("skipping real rust-analyzer integration test: rust-analyzer is not in PATH");
            return;
        }

        let workspace = external_server_workspace("rust-analyzer");
        fs::create_dir(workspace.join("src")).unwrap();
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"cgraph-ra-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(
            workspace.join("src/main.rs"),
            "trait Command {}\nstruct Cli;\nimpl Command for Cli {}\n\nfn helper() {}\n\nfn main() {\n    helper();\n}\n",
        )
        .unwrap();

        let lsp = timeout(
            Duration::from_secs(60),
            LspProvider::start(LspConfig::for_server("rust-analyzer", &workspace)),
        )
        .await
        .expect("rust-analyzer initialization timed out")
        .unwrap();
        let main = wait_for_workspace_symbol(&lsp, "main").await;
        let position = main.range.unwrap().start;
        let query = HierarchyQuery {
            symbol: SymbolIdentity {
                symbol: main.name,
                kind: HierarchyKind::Call,
                location: Some(SourceLocation {
                    uri: main.uri.to_string(),
                    line: Some(position.line),
                    character: Some(position.character),
                }),
            },
            direction: HierarchyDirection::Outgoing,
        };
        let response = timeout(Duration::from_secs(30), async {
            loop {
                match lsp.hierarchy_client().query(query.clone()).await {
                    Ok(response) => break response,
                    Err(error)
                        if error
                            .chain()
                            .any(|cause| cause.to_string().contains("content modified")) =>
                    {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(error) => panic!("rust-analyzer call hierarchy failed: {error:#}"),
                }
            }
        })
        .await
        .expect("rust-analyzer call hierarchy did not stabilize");
        assert!(
            response
                .children
                .iter()
                .any(|child| child.symbol == "helper")
        );

        let tree_sitter = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Rust)
            .expect("Rust Tree-sitter fallback failed to initialize");
        let cli = tree_sitter
            .workspace_symbol_client()
            .query("Cli")
            .await
            .unwrap()
            .into_iter()
            .find(|symbol| symbol.name == "Cli")
            .expect("Tree-sitter did not index Cli");
        let cli_position = cli.range.expect("Cli has no source range").start;
        let types = FetchHierarchyClient::with_fallback(
            lsp.hierarchy_client(),
            tree_sitter.hierarchy_client(),
        )
        .query(HierarchyQuery {
            symbol: SymbolIdentity {
                symbol: cli.name,
                kind: HierarchyKind::Type,
                location: Some(SourceLocation {
                    uri: cli.uri.to_string(),
                    line: Some(cli_position.line),
                    character: Some(cli_position.character),
                }),
            },
            direction: HierarchyDirection::Incoming,
        })
        .await
        .expect("rust-analyzer type hierarchy or Tree-sitter fallback failed");
        assert!(types.children.iter().any(|child| child.symbol == "Command"));

        lsp.shutdown().await.unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn integrates_with_installed_clangd_workspace_symbols() {
        if !external_server_available("clangd").await {
            eprintln!("skipping real clangd integration test: clangd is not in PATH");
            return;
        }

        let workspace = external_server_workspace("clangd");
        let source = workspace.join("src/main.cpp");
        let header = workspace.join("include/worker.hpp");
        fs::create_dir(workspace.join("src")).unwrap();
        fs::create_dir(workspace.join("include")).unwrap();
        fs::write(
            workspace.join("compile_commands.json"),
            serde_json::to_vec(&vec![json!({
                "directory": workspace,
                "file": source,
                "arguments": [
                    "clang++",
                    "-std=c++17",
                    "-Wall",
                    "-Iinclude",
                    "-c",
                    source,
                ],
            })])
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &source,
            "#include \"worker.hpp\"\n\nvoid helper() {}\nint main() { demo::Worker worker; worker.run(); helper(); return 0; }\n",
        )
        .unwrap();
        fs::write(
            &header,
            "#pragma once\nnamespace demo {\nvoid helper() {}\nclass Base {};\nclass Worker : public Base { public: void run() { helper(); } };\n}\n",
        )
        .unwrap();

        let lsp = timeout(
            Duration::from_secs(60),
            LspProvider::start(LspConfig::for_server("clangd", &workspace)),
        )
        .await
        .expect("clangd initialization timed out")
        .unwrap();

        let method = wait_for_workspace_symbol_leaf(&lsp, "run").await;
        assert_eq!(method.name, "demo::Worker::run");
        assert_eq!(method.kind, SymbolKind::METHOD);
        assert_eq!(method.uri, Url::from_file_path(&header).unwrap());
        assert!(method.range.is_some());

        let method_position = method.range.unwrap().start;
        let calls = lsp
            .hierarchy_client()
            .query(HierarchyQuery {
                symbol: SymbolIdentity {
                    symbol: method.name.clone(),
                    kind: HierarchyKind::Call,
                    location: Some(SourceLocation {
                        uri: method.uri.to_string(),
                        line: Some(method_position.line),
                        character: Some(method_position.character),
                    }),
                },
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .expect("clangd call hierarchy failed");
        assert!(calls.children.iter().any(|child| child.symbol == "helper"));

        let tree_sitter = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Cpp)
            .expect("C++ Tree-sitter fallback failed to initialize");
        let worker = tree_sitter
            .workspace_symbol_client()
            .query("Worker")
            .await
            .unwrap()
            .into_iter()
            .find(|symbol| symbol.name == "Worker")
            .expect("Tree-sitter did not index Worker");
        let worker_position = worker.range.expect("Worker has no source range").start;
        let types = FetchHierarchyClient::with_fallback(
            lsp.hierarchy_client(),
            tree_sitter.hierarchy_client(),
        )
        .query(HierarchyQuery {
            symbol: SymbolIdentity {
                symbol: worker.name,
                kind: HierarchyKind::Type,
                location: Some(SourceLocation {
                    uri: worker.uri.to_string(),
                    line: Some(worker_position.line),
                    character: Some(worker_position.character),
                }),
            },
            direction: HierarchyDirection::Incoming,
        })
        .await
        .expect("clangd type hierarchy or Tree-sitter fallback failed");
        assert!(types.children.iter().any(|child| child.symbol == "Base"));

        let main = wait_for_workspace_symbol(&lsp, "main").await;
        assert_eq!(main.name, "main");
        assert_eq!(main.kind, SymbolKind::FUNCTION);
        assert_eq!(main.uri, Url::from_file_path(&source).unwrap());

        lsp.shutdown().await.unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn opens_clangd_header_before_hierarchy_query() {
        if !external_server_available("clangd").await {
            eprintln!(
                "skipping real clangd header hierarchy regression test: clangd is not in PATH"
            );
            return;
        }

        let workspace = external_server_workspace("clangd-header-hierarchy");
        let source = workspace.join("hello.cpp");
        let header = workspace.join("hello.h");
        fs::write(
            workspace.join("compile_commands.json"),
            serde_json::to_vec(&vec![json!({
                "directory": workspace,
                "file": source,
                "arguments": ["clang++", "-std=c++17", "-c", source],
            })])
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &source,
            "#include \"hello.h\"\nint main() { bar(); return 0; }\n",
        )
        .unwrap();
        fs::write(&header, "#pragma once\nvoid bar() {}\n").unwrap();

        let lsp = timeout(
            Duration::from_secs(60),
            LspProvider::start(LspConfig::for_server("clangd", &workspace)),
        )
        .await
        .expect("clangd header hierarchy initialization timed out")
        .unwrap();
        let bar = wait_for_workspace_symbol_in_file(&lsp, "bar", &header).await;
        assert_eq!(bar.name, "bar");
        assert_eq!(bar.kind, SymbolKind::FUNCTION);
        let hierarchy_client = lsp.hierarchy_client();
        let incoming_query = HierarchyQuery {
            symbol: SymbolIdentity {
                symbol: bar.name.clone(),
                kind: HierarchyKind::Call,
                location: None,
            },
            direction: HierarchyDirection::Incoming,
        };
        let outgoing_query = HierarchyQuery {
            symbol: incoming_query.symbol.clone(),
            direction: HierarchyDirection::Outgoing,
        };
        let (incoming, outgoing) = tokio::join!(
            hierarchy_client.query(incoming_query),
            hierarchy_client.query(outgoing_query)
        );
        let incoming =
            incoming.expect("clangd failed to prepare incoming hierarchy for an opened header");
        let outgoing =
            outgoing.expect("clangd failed to prepare outgoing hierarchy for an opened header");
        assert!(incoming.children.iter().any(|child| child.symbol == "main"));
        assert!(outgoing.children.is_empty());

        lsp.shutdown().await.unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn integrates_with_clangd_external_compilation_database_and_project_config() {
        if !external_server_available("clangd").await {
            eprintln!("skipping real clangd integration test: clangd is not in PATH");
            return;
        }

        let fixture_root = external_server_workspace("clangd-external-build");
        let workspace = fixture_root.join("cpp-project");
        let build_directory = fixture_root.join("tmp-build");
        let include_directory = workspace.join("include");
        let source_directory = workspace.join("src");
        fs::create_dir_all(&include_directory).unwrap();
        fs::create_dir_all(&source_directory).unwrap();
        fs::create_dir_all(&build_directory).unwrap();

        let main_source = source_directory.join("main.cpp");
        let worker_source = source_directory.join("a_worker.cpp");
        let helper_source = source_directory.join("z_helper.cpp");
        let base_header = include_directory.join("base.hpp");
        let worker_header = include_directory.join("worker.hpp");
        let helper_header = include_directory.join("helper.hpp");

        fs::write(
            workspace.join(".clangd"),
            "CompileFlags:\n  CompilationDatabase: ../tmp-build\n",
        )
        .unwrap();
        fs::write(
            &base_header,
            "#pragma once\nnamespace demo {\nclass Base { public: virtual ~Base() = default; virtual void run() = 0; };\n}\n",
        )
        .unwrap();
        fs::write(
            &worker_header,
            "#pragma once\n#include \"base.hpp\"\nnamespace demo {\nclass Worker final : public Base { public: void run() override; };\n}\n",
        )
        .unwrap();
        fs::write(
            &helper_header,
            "#pragma once\nnamespace demo { void helper(); }\n",
        )
        .unwrap();
        fs::write(
            &helper_source,
            "#include \"helper.hpp\"\nnamespace demo { void helper() {} }\n",
        )
        .unwrap();
        fs::write(
            &worker_source,
            "#include \"worker.hpp\"\n#include \"helper.hpp\"\nnamespace demo { void Worker::run() { helper(); } }\n",
        )
        .unwrap();
        fs::write(
            &main_source,
            "#include \"worker.hpp\"\nint main() { demo::Worker worker; worker.run(); return 0; }\n",
        )
        .unwrap();

        let compilation_database = [&main_source, &worker_source, &helper_source]
            .into_iter()
            .map(|source| {
                json!({
                    "directory": workspace,
                    "file": source,
                    "arguments": [
                        "clang++",
                        "-std=c++17",
                        format!("-I{}", include_directory.display()),
                        "-c",
                        source,
                    ],
                })
            })
            .collect::<Vec<_>>();
        fs::write(
            build_directory.join("compile_commands.json"),
            serde_json::to_vec(&compilation_database).unwrap(),
        )
        .unwrap();

        let lsp = timeout(
            Duration::from_secs(60),
            LspProvider::start(
                LspConfig::for_server("clangd", &workspace).file_extensions(["cpp"]),
            ),
        )
        .await
        .expect("clangd initialization with external compilation database timed out")
        .unwrap();

        let main = wait_for_workspace_symbol_in_file(&lsp, "main", &main_source).await;
        assert_eq!(main.kind, SymbolKind::FUNCTION);
        let helper = wait_for_workspace_symbol_in_file(&lsp, "helper", &helper_source).await;
        assert_eq!(helper.name, "helper");
        assert_eq!(helper.kind, SymbolKind::FUNCTION);
        let method = wait_for_workspace_symbol_in_file(&lsp, "run", &worker_source).await;
        assert_eq!(method.name, "demo::Worker::run");
        assert_eq!(method.kind, SymbolKind::METHOD);

        let method_position = method.range.expect("Worker::run has no source range").start;
        let outgoing = lsp
            .hierarchy_client()
            .query(HierarchyQuery {
                symbol: SymbolIdentity {
                    symbol: method.name.clone(),
                    kind: HierarchyKind::Call,
                    location: Some(SourceLocation {
                        uri: method.uri.to_string(),
                        line: Some(method_position.line),
                        character: Some(method_position.character),
                    }),
                },
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .expect("clangd cross-file outgoing call hierarchy failed");
        assert!(
            outgoing
                .children
                .iter()
                .any(|child| symbol_leaf_name(&child.symbol) == "helper")
        );

        let tree_sitter = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Cpp)
            .expect("C++ Tree-sitter fallback failed to initialize");
        let worker = tree_sitter
            .workspace_symbol_client()
            .query("Worker")
            .await
            .unwrap()
            .into_iter()
            .find(|symbol| symbol.name == "Worker")
            .expect("Tree-sitter did not index Worker from worker.hpp");
        assert_eq!(worker.uri, Url::from_file_path(&worker_header).unwrap());
        let worker_position = worker.range.expect("Worker has no source range").start;
        let supertypes = FetchHierarchyClient::with_fallback(
            lsp.hierarchy_client(),
            tree_sitter.hierarchy_client(),
        )
        .query(HierarchyQuery {
            symbol: SymbolIdentity {
                symbol: worker.name,
                kind: HierarchyKind::Type,
                location: Some(SourceLocation {
                    uri: worker.uri.to_string(),
                    line: Some(worker_position.line),
                    character: Some(worker_position.character),
                }),
            },
            direction: HierarchyDirection::Incoming,
        })
        .await
        .expect("clangd type hierarchy or Tree-sitter fallback failed");
        assert!(
            supertypes
                .children
                .iter()
                .any(|child| child.symbol == "Base")
        );

        lsp.shutdown().await.unwrap();
        fs::remove_dir_all(fixture_root).unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_messages() {
        use tokio::io::AsyncWriteExt;

        let (mut client_stream, server_stream) = duplex(128);
        let _server_task = tokio::spawn(async move {
            client_stream
                .write_all(b"Content-Length: 16777217\r\n\r\n")
                .await
                .unwrap();
        });

        let error = read_message(&mut BufReader::new(server_stream))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("too large"));
    }

    async fn external_server_available(program: &str) -> bool {
        tokio::process::Command::new(program)
            .arg("--version")
            .output()
            .await
            .is_ok_and(|output| output.status.success())
    }

    fn external_server_workspace(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-{name}-{unique}"));
        fs::create_dir(&workspace).unwrap();
        workspace
    }

    async fn wait_for_workspace_symbol(
        lsp: &LspProvider,
        expected_name: &str,
    ) -> WorkspaceSymbolMatch {
        timeout(Duration::from_secs(30), async {
            loop {
                if let Some(symbol) = lsp
                    .workspace_symbols(expected_name)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|symbol| symbol.name == expected_name)
                {
                    return symbol;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{expected_name:?} did not appear in workspace symbols"))
    }

    async fn wait_for_workspace_symbol_leaf(
        lsp: &LspProvider,
        expected_name: &str,
    ) -> WorkspaceSymbolMatch {
        timeout(Duration::from_secs(30), async {
            loop {
                if let Some(symbol) = lsp
                    .workspace_symbols(expected_name)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|symbol| symbol_leaf_name(&symbol.name) == expected_name)
                {
                    return symbol;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{expected_name:?} did not appear in workspace symbols"))
    }

    async fn wait_for_workspace_symbol_in_file(
        lsp: &LspProvider,
        expected_leaf: &str,
        expected_path: &Path,
    ) -> WorkspaceSymbolMatch {
        let expected_uri = Url::from_file_path(expected_path).unwrap();
        timeout(Duration::from_secs(30), async {
            loop {
                if let Some(symbol) = lsp
                    .workspace_symbols(expected_leaf)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|symbol| {
                        symbol_leaf_name(&symbol.name) == expected_leaf
                            && symbol.uri == expected_uri
                    })
                {
                    return symbol;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{expected_leaf:?} in {} did not appear in workspace symbols",
                expected_path.display()
            )
        })
    }

    fn symbol(uri: &str) -> WorkspaceSymbolMatch {
        WorkspaceSymbolMatch {
            name: "symbol".to_owned(),
            kind: SymbolKind::FUNCTION,
            container_name: None,
            uri: Url::parse(uri).unwrap(),
            range: None,
        }
    }

    fn call_item(name: &str, line: u32) -> Value {
        json!({
            "name": name,
            "kind": 12,
            "uri": "file:///workspace/src/main.rs",
            "range": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line, "character": name.len() }
            },
            "selectionRange": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line, "character": name.len() }
            }
        })
    }

    fn rust_method_item(name: &str, line: u32) -> Value {
        let mut item = call_item(name, line);
        item["detail"] = json!(format!("pub fn {name}(&self)"));
        item
    }

    fn external_call_item(name: &str, line: u32) -> Value {
        json!({
            "name": name,
            "kind": 12,
            "uri": "file:///usr/include/stdio.h",
            "range": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line, "character": name.len() }
            },
            "selectionRange": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line, "character": name.len() }
            }
        })
    }

    fn type_item(name: &str, line: u32) -> Value {
        json!({
            "name": name,
            "kind": 23,
            "uri": "file:///workspace/src/main.rs",
            "range": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line, "character": name.len() }
            },
            "selectionRange": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line, "character": name.len() }
            }
        })
    }
}
