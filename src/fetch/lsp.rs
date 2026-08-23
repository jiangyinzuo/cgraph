//! Minimal stdio LSP client used by the fetch layer.
//!
//! The transport is implemented locally because `tower-lsp` supplies protocol
//! types and server infrastructure, but ctree needs to act as an LSP client.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fmt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tower_lsp::lsp_types::{
    CallHierarchyClientCapabilities, CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams,
    CallHierarchyItem, CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams,
    CallHierarchyPrepareParams, ClientCapabilities, ClientInfo, InitializeParams, InitializeResult,
    Location, NumberOrString, OneOf, PartialResultParams, Position, ProgressParams,
    ProgressParamsValue, Range, ServerInfo, SymbolKind, TextDocumentClientCapabilities,
    TextDocumentIdentifier, TextDocumentPositionParams, TypeHierarchyClientCapabilities,
    TypeHierarchyItem, TypeHierarchyPrepareParams, TypeHierarchySubtypesParams,
    TypeHierarchySupertypesParams, Url, WindowClientCapabilities, WorkDoneProgress,
    WorkDoneProgressParams, WorkspaceClientCapabilities, WorkspaceFolder,
    WorkspaceSymbolClientCapabilities, WorkspaceSymbolParams, WorkspaceSymbolResponse,
    request::{
        CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare, Initialize,
        Request, Shutdown, TypeHierarchyPrepare, TypeHierarchySubtypes, TypeHierarchySupertypes,
        WorkspaceSymbolRequest,
    },
};

use crate::{
    fetch::{FetchSource, HierarchyQuery, HierarchyResponse},
    state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
};

// A corrupt Content-Length must not turn into an attacker-controlled allocation.
const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct LspConfig {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub workspace_root: PathBuf,
    pub initialization_options: Option<Value>,
}

impl LspConfig {
    pub fn new(program: impl Into<OsString>, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            workspace_root: workspace_root.into(),
            initialization_options: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn initialization_options(mut self, options: Value) -> Self {
        self.initialization_options = Some(options);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSymbolMatch {
    pub name: String,
    pub kind: SymbolKind,
    pub container_name: Option<String>,
    pub uri: Url,
    pub range: Option<Range>,
}

impl WorkspaceSymbolMatch {
    pub fn display_name(&self) -> String {
        qualified_callable_name(&self.name, self.kind, self.container_name.as_deref())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A provider-level status event, kept separate from individual request results.
///
/// Language servers may run several work-done tasks concurrently. The JSON-RPC
/// actor collapses those protocol tokens into the most useful current update;
/// the TUI then maps this LSP-specific type into its backend-neutral status.
pub enum LspStatusUpdate {
    Ready {
        message: Option<String>,
    },
    Progress {
        title: String,
        message: Option<String>,
        percentage: Option<u32>,
    },
    Warning(String),
    Error(String),
    Disconnected(String),
}

#[derive(Clone, Debug)]
struct ActiveProgress {
    sequence: u64,
    title: String,
    message: Option<String>,
    percentage: Option<u32>,
}

#[derive(Default)]
struct LspProgressTracker {
    next_sequence: u64,
    active: HashMap<String, ActiveProgress>,
}

#[derive(Clone)]
pub struct WorkspaceSymbolClient {
    client: JsonRpcClient,
    workspace_root: PathBuf,
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
        let params = WorkspaceSymbolParams {
            query: query.to_owned(),
            ..WorkspaceSymbolParams::default()
        };
        let response: Option<WorkspaceSymbolResponse> = self
            .client
            .request(WorkspaceSymbolRequest::METHOD, params)
            .await
            .with_context(|| format!("workspace symbol query failed for {query:?}"))?;

        let symbols = response.map(normalize_symbols).unwrap_or_default();
        Ok(deduplicate_symbols(symbols.into_iter().filter(|symbol| {
            symbol_belongs_to_workspace(symbol, &self.workspace_root)
        })))
    }
}

#[derive(Clone)]
pub struct HierarchyClient {
    client: JsonRpcClient,
    workspace_root: PathBuf,
}

impl fmt::Debug for HierarchyClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HierarchyClient")
            .finish_non_exhaustive()
    }
}

impl HierarchyClient {
    pub async fn query(&self, mut query: HierarchyQuery) -> Result<HierarchyResponse> {
        let (document_position, resolved_location) =
            self.resolve_document_position(&query.symbol).await?;
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

        let lookup_name = symbol.symbol.rsplit("::").next().unwrap_or(&symbol.symbol);
        let candidates = WorkspaceSymbolClient {
            client: self.client.clone(),
            workspace_root: self.workspace_root.clone(),
        }
        .query(lookup_name)
        .await?
        .into_iter()
        .filter(|candidate| {
            candidate.range.is_some()
                && candidate.name == lookup_name
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
                Ok(calls
                    .unwrap_or_default()
                    .into_iter()
                    .map(|call| call_item_identity(call.from))
                    .collect())
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
                Ok(calls
                    .unwrap_or_default()
                    .into_iter()
                    .map(|call| call_item_identity(call.to))
                    .collect())
            }
        }
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
            .map(type_item_identity)
            .collect())
    }
}

pub struct LspProvider {
    child: Child,
    client: JsonRpcClient,
    connection_task: JoinHandle<Result<()>>,
    workspace_root: PathBuf,
    server_info: Option<ServerInfo>,
    status_receiver: Option<mpsc::UnboundedReceiver<LspStatusUpdate>>,
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

        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .current_dir(&workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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
        let (client, status_receiver, connection_task) = spawn_json_rpc(
            BufReader::new(stdout),
            stdin,
            workspace_uri.clone(),
            workspace_name.clone(),
        );

        let capabilities = ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                call_hierarchy: Some(CallHierarchyClientCapabilities::default()),
                type_hierarchy: Some(TypeHierarchyClientCapabilities::default()),
                ..TextDocumentClientCapabilities::default()
            }),
            workspace: Some(WorkspaceClientCapabilities {
                symbol: Some(WorkspaceSymbolClientCapabilities::default()),
                workspace_folders: Some(true),
                configuration: Some(true),
                ..WorkspaceClientCapabilities::default()
            }),
            window: Some(WindowClientCapabilities {
                work_done_progress: Some(true),
                ..WindowClientCapabilities::default()
            }),
            experimental: Some(json!({
                "serverStatusNotification": true,
            })),
            ..ClientCapabilities::default()
        };
        let initialization_options = workspace_symbol_initialization_options(
            &config.program,
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
        if !workspace_symbol_supported(&initialize_result) {
            bail!("language server does not support workspace/symbol");
        }

        client
            .notify("initialized", json!({}))
            .await
            .context("failed to notify language server that initialization completed")?;

        Ok(Self {
            child,
            client,
            connection_task,
            workspace_root,
            server_info: initialize_result.server_info,
            status_receiver: Some(status_receiver),
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
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
        }
    }

    pub fn hierarchy_client(&self) -> HierarchyClient {
        HierarchyClient {
            client: self.client.clone(),
            workspace_root: self.workspace_root.clone(),
        }
    }

    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbolMatch>> {
        self.workspace_symbol_client().query(query).await
    }

    pub async fn shutdown(mut self) -> Result<()> {
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

        shutdown_result
    }
}

#[derive(Clone)]
struct JsonRpcClient {
    commands: mpsc::Sender<JsonRpcCommand>,
    cancellations: mpsc::UnboundedSender<u64>,
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
    let reader_task = tokio::spawn(read_messages(reader, incoming_sender));
    let connection_task = tokio::spawn(async move {
        let result = run_json_rpc(
            writer,
            command_receiver,
            cancellation_receiver,
            incoming_receiver,
            status_sender,
            workspace_uri,
            workspace_name,
        )
        .await;
        reader_task.abort();
        let _ = reader_task.await;
        result
    });

    let client = JsonRpcClient {
        commands: command_sender,
        cancellations: cancellation_sender,
    };
    (client, status_receiver, connection_task)
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
    workspace_uri: Url,
    workspace_name: String,
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
                            &workspace_uri,
                            &workspace_name,
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

fn handle_server_notification(
    message: &Value,
    tracker: &mut LspProgressTracker,
    sender: &mpsc::UnboundedSender<LspStatusUpdate>,
) {
    match message.get("method").and_then(Value::as_str) {
        Some("$/progress") => {
            let Some(params) = message.get("params").cloned() else {
                return;
            };
            let Ok(params) = serde_json::from_value::<ProgressParams>(params) else {
                return;
            };
            let ProgressParamsValue::WorkDone(progress) = params.value;
            tracker.update(params.token, progress, sender);
        }
        Some("experimental/serverStatus") => {
            let Some(params) = message.get("params") else {
                return;
            };
            let health = params.get("health").and_then(Value::as_str).unwrap_or("ok");
            let quiescent = params
                .get("quiescent")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned);

            let update = match health {
                "warning" => LspStatusUpdate::Warning(
                    message.unwrap_or_else(|| "Language server reported a warning".to_owned()),
                ),
                "error" => LspStatusUpdate::Error(
                    message.unwrap_or_else(|| "Language server reported an error".to_owned()),
                ),
                _ if quiescent => {
                    if tracker.emit_latest(sender) {
                        return;
                    }
                    LspStatusUpdate::Ready { message }
                }
                _ => {
                    if tracker.emit_latest(sender) {
                        return;
                    }
                    LspStatusUpdate::Progress {
                        title: "rust-analyzer".to_owned(),
                        message: message.or_else(|| Some("Background work in progress".to_owned())),
                        percentage: None,
                    }
                }
            };
            let _ = sender.send(update);
        }
        _ => {}
    }
}

impl LspProgressTracker {
    fn update(
        &mut self,
        token: NumberOrString,
        progress: WorkDoneProgress,
        sender: &mpsc::UnboundedSender<LspStatusUpdate>,
    ) {
        let token = progress_token_key(token);
        self.next_sequence = self.next_sequence.wrapping_add(1);
        match progress {
            WorkDoneProgress::Begin(progress) => {
                self.active.insert(
                    token,
                    ActiveProgress {
                        sequence: self.next_sequence,
                        title: progress.title,
                        message: progress.message,
                        percentage: progress.percentage,
                    },
                );
                self.emit_latest(sender);
            }
            WorkDoneProgress::Report(progress) => {
                if let Some(active) = self.active.get_mut(&token) {
                    active.sequence = self.next_sequence;
                    if progress.message.is_some() {
                        active.message = progress.message;
                    }
                    if progress.percentage.is_some() {
                        active.percentage = progress.percentage;
                    }
                    self.emit_latest(sender);
                }
            }
            WorkDoneProgress::End(progress) => {
                self.active.remove(&token);
                if !self.emit_latest(sender) {
                    let _ = sender.send(LspStatusUpdate::Ready {
                        message: progress.message,
                    });
                }
            }
        }
    }

    fn emit_latest(&self, sender: &mpsc::UnboundedSender<LspStatusUpdate>) -> bool {
        let Some(progress) = self
            .active
            .values()
            .max_by_key(|progress| progress.sequence)
        else {
            return false;
        };
        let _ = sender.send(LspStatusUpdate::Progress {
            title: progress.title.clone(),
            message: progress.message.clone(),
            percentage: progress.percentage,
        });
        true
    }
}

fn progress_token_key(token: NumberOrString) -> String {
    match token {
        NumberOrString::Number(number) => format!("number:{number}"),
        NumberOrString::String(string) => format!("string:{string}"),
    }
}

async fn handle_server_message<W>(
    writer: &mut W,
    message: &Value,
    workspace_uri: &Url,
    workspace_name: &str,
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
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "window/showMessageRequest" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": null,
        }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("ctree does not implement {method}"),
            },
        }),
    };

    write_message(writer, &response).await
}

fn requested_configuration(section: Option<&str>) -> Value {
    match section {
        Some("rust-analyzer") => json!({
            "workspace": {
                "symbol": {
                    "search": {
                        "kind": "all_symbols",
                        "scope": "workspace",
                    }
                }
            }
        }),
        Some("rust-analyzer.workspace.symbol.search.kind") => json!("all_symbols"),
        Some("rust-analyzer.workspace.symbol.search.scope") => json!("workspace"),
        _ => Value::Null,
    }
}

fn workspace_name(workspace_root: &Path) -> String {
    workspace_root
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("workspace")
        .to_owned()
}

fn workspace_symbol_supported(initialize_result: &InitializeResult) -> bool {
    match &initialize_result.capabilities.workspace_symbol_provider {
        Some(OneOf::Left(supported)) => *supported,
        Some(OneOf::Right(_)) => true,
        None => false,
    }
}

fn workspace_symbol_initialization_options(
    program: &OsStr,
    options: Option<Value>,
) -> Option<Value> {
    if !is_rust_analyzer_program(program) {
        return options;
    }

    let mut options = options.unwrap_or_else(|| json!({}));
    merge_json(
        &mut options,
        json!({
            "workspace": {
                "symbol": {
                    "search": {
                        "kind": "all_symbols",
                        "scope": "workspace",
                    }
                }
            }
        }),
    );
    Some(options)
}

fn is_rust_analyzer_program(program: &OsStr) -> bool {
    let program_name = Path::new(program)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    program_name.eq_ignore_ascii_case("rust-analyzer")
        || program_name.eq_ignore_ascii_case("rust-analyzer.exe")
}

fn merge_json(target: &mut Value, overlay: Value) {
    match (target, overlay) {
        (Value::Object(target), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge_json(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, overlay) => *target = overlay,
    }
}

fn symbol_belongs_to_workspace(symbol: &WorkspaceSymbolMatch, workspace_root: &Path) -> bool {
    symbol
        .uri
        .to_file_path()
        .is_ok_and(|path| path.starts_with(workspace_root))
}

fn deduplicate_symbols(
    symbols: impl IntoIterator<Item = WorkspaceSymbolMatch>,
) -> Vec<WorkspaceSymbolMatch> {
    let mut unique = Vec::new();
    for symbol in symbols {
        let duplicate = unique.iter().any(|existing: &WorkspaceSymbolMatch| {
            existing.name == symbol.name
                && existing.kind == symbol.kind
                && existing.uri == symbol.uri
                && existing.range == symbol.range
                && existing.container_name == symbol.container_name
        });
        if !duplicate {
            unique.push(symbol);
        }
    }
    unique
}

fn document_position(uri: Url, position: Position) -> (TextDocumentPositionParams, SourceLocation) {
    let location = SourceLocation {
        uri: uri.to_string(),
        line: Some(position.line),
        character: Some(position.character),
    };
    (
        TextDocumentPositionParams::new(TextDocumentIdentifier::new(uri), position),
        location,
    )
}

fn symbol_kind_matches_hierarchy(kind: HierarchyKind, symbol_kind: SymbolKind) -> bool {
    match kind {
        HierarchyKind::Call => matches!(
            symbol_kind,
            SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::CONSTRUCTOR
        ),
        HierarchyKind::Type => matches!(
            symbol_kind,
            SymbolKind::CLASS
                | SymbolKind::INTERFACE
                | SymbolKind::STRUCT
                | SymbolKind::ENUM
                | SymbolKind::TYPE_PARAMETER
        ),
    }
}

fn call_item_identity(item: CallHierarchyItem) -> SymbolIdentity {
    let symbol = qualified_callable_name(&item.name, item.kind, item.detail.as_deref());
    SymbolIdentity {
        symbol,
        kind: HierarchyKind::Call,
        location: Some(SourceLocation {
            uri: item.uri.to_string(),
            line: Some(item.selection_range.start.line),
            character: Some(item.selection_range.start.character),
        }),
    }
}

fn qualified_callable_name(
    name: &str,
    kind: SymbolKind,
    container_or_detail: Option<&str>,
) -> String {
    if !matches!(kind, SymbolKind::METHOD | SymbolKind::CONSTRUCTOR) || name.contains("::") {
        return name.to_owned();
    }
    let Some(container) = container_or_detail
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return name.to_owned();
    };
    let container = container.strip_prefix("impl ").unwrap_or(container).trim();
    if container.contains(['/', '\\', '(', ')', '\n']) || container.split_whitespace().count() > 1 {
        return name.to_owned();
    }
    if container.ends_with(&format!("::{name}")) || container.ends_with(&format!(".{name}")) {
        return container.to_owned();
    }
    format!("{container}::{name}")
}

fn type_item_identity(item: TypeHierarchyItem) -> SymbolIdentity {
    SymbolIdentity {
        symbol: item.name,
        kind: HierarchyKind::Type,
        location: Some(SourceLocation {
            uri: item.uri.to_string(),
            line: Some(item.selection_range.start.line),
            character: Some(item.selection_range.start.character),
        }),
    }
}

fn deduplicate_identities(
    identities: impl IntoIterator<Item = SymbolIdentity>,
) -> Vec<SymbolIdentity> {
    let mut unique = Vec::new();
    for identity in identities {
        if !unique.contains(&identity) {
            unique.push(identity);
        }
    }
    unique
}

fn normalize_symbols(response: WorkspaceSymbolResponse) -> Vec<WorkspaceSymbolMatch> {
    match response {
        WorkspaceSymbolResponse::Flat(symbols) => symbols
            .into_iter()
            .map(|symbol| WorkspaceSymbolMatch {
                name: symbol.name,
                kind: symbol.kind,
                container_name: symbol.container_name,
                uri: symbol.location.uri,
                range: Some(symbol.location.range),
            })
            .collect(),
        WorkspaceSymbolResponse::Nested(symbols) => symbols
            .into_iter()
            .map(|symbol| {
                let (uri, range) = match symbol.location {
                    OneOf::Left(Location { uri, range }) => (uri, Some(range)),
                    OneOf::Right(location) => (location.uri, None),
                };
                WorkspaceSymbolMatch {
                    name: symbol.name,
                    kind: symbol.kind,
                    container_name: symbol.container_name,
                    uri,
                    range,
                }
            })
            .collect(),
    }
}

fn response_id(message: &Value) -> Option<u64> {
    message.get("id").and_then(Value::as_u64)
}

async fn read_message<R>(reader: &mut R) -> Result<Value>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length = None;

    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).await? == 0 {
            bail!("language server closed its output stream");
        }
        let header = header.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }

        let Some((name, value)) = header.split_once(':') else {
            bail!("malformed LSP header: {header:?}");
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("invalid LSP Content-Length header")?,
            );
        }
    }

    let content_length = content_length.context("LSP message has no Content-Length header")?;
    if content_length > MAX_MESSAGE_SIZE {
        bail!("LSP message is too large: {content_length} bytes (limit: {MAX_MESSAGE_SIZE} bytes)");
    }

    let mut body = vec![0; content_length];
    reader
        .read_exact(&mut body)
        .await
        .context("language server closed its output stream mid-message")?;
    serde_json::from_slice(&body).context("language server sent invalid JSON")
}

async fn write_message<W>(writer: &mut W, message: &Value) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(message).context("failed to encode LSP message")?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        path::{Path, PathBuf},
        time::Duration,
    };

    use serde_json::{Value, json};
    use tokio::io::{BufReader, duplex, split};
    use tokio::time::timeout;
    use tower_lsp::lsp_types::{SymbolKind, Url, WorkspaceSymbolParams, WorkspaceSymbolResponse};

    use super::{
        HierarchyClient, LspProgressTracker, LspStatusUpdate, WorkspaceSymbolMatch,
        deduplicate_symbols, handle_server_notification, normalize_symbols, read_message,
        requested_configuration, response_id, spawn_json_rpc, symbol_belongs_to_workspace,
        workspace_symbol_initialization_options, write_message,
    };
    use crate::{
        fetch::{FetchSource, HierarchyQuery},
        state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
    };

    #[test]
    fn excludes_symbols_outside_the_workspace() {
        let project_symbol = symbol("file:///workspace/src/main.rs");
        let dependency_symbol = symbol("file:///registry/dependency/src/lib.rs");

        assert!(symbol_belongs_to_workspace(
            &project_symbol,
            Path::new("/workspace")
        ));
        assert!(!symbol_belongs_to_workspace(
            &dependency_symbol,
            Path::new("/workspace")
        ));
    }

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

        let options = workspace_symbol_initialization_options(
            OsStr::new("rust-analyzer"),
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
                Some(json!({ "clangd": true })),
            ),
            Some(json!({ "clangd": true }))
        );
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
    async fn prepares_and_queries_outgoing_call_hierarchy() {
        let (client_stream, server_stream) = duplex(8 * 1024);
        let (client_reader, client_writer) = split(client_stream);
        let (server_reader, mut server_writer) = split(server_stream);
        let workspace_uri = Url::parse("file:///workspace").unwrap();
        let (rpc_client, _status_receiver, connection_task) = spawn_json_rpc(
            BufReader::new(client_reader),
            client_writer,
            workspace_uri,
            "workspace".to_owned(),
        );
        let hierarchy_client = HierarchyClient {
            client: rpc_client.clone(),
            workspace_root: PathBuf::from("/workspace"),
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
                    { "to": method_item("child", "Worker", 8), "fromRanges": [] },
                    { "to": method_item("child", "Worker", 8), "fromRanges": [] }
                ]
            }),
        )
        .await
        .unwrap();

        let response = client_task.await.unwrap();
        assert_eq!(response.source, FetchSource::Lsp);
        assert_eq!(response.children.len(), 1);
        assert_eq!(response.children[0].symbol, "Worker::child");
        assert_eq!(response.children[0].kind, HierarchyKind::Call);
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
        let (rpc_client, _status_receiver, connection_task) = spawn_json_rpc(
            BufReader::new(client_reader),
            client_writer,
            workspace_uri,
            "workspace".to_owned(),
        );
        let hierarchy_client = HierarchyClient {
            client: rpc_client.clone(),
            workspace_root: PathBuf::from("/workspace"),
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
        let mut server_reader = BufReader::new(server_reader);

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

        drop(rpc_client);
        connection_task.abort();
        let _ = connection_task.await;
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
        let (client, _status_receiver, connection_task) = spawn_json_rpc(
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
            normalize_symbols(response.unwrap())
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
        let (client, _status_receiver, connection_task) = spawn_json_rpc(
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

    fn method_item(name: &str, container: &str, line: u32) -> Value {
        let mut item = call_item(name, line);
        item["kind"] = json!(6);
        item["detail"] = json!(container);
        item
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
