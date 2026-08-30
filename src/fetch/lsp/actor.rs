use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncWrite},
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};
use tower_lsp::lsp_types::Url;

use super::{
    capabilities::{ServerHierarchyCapabilities, requested_configuration},
    framing::{read_message, write_message},
    progress::{LspProgressTracker, LspStatusUpdate, handle_server_notification},
};

#[derive(Clone)]
pub(super) struct JsonRpcClient {
    commands: mpsc::Sender<JsonRpcCommand>,
    cancellations: mpsc::UnboundedSender<u64>,
    status_updates: mpsc::UnboundedSender<LspStatusUpdate>,
    pub(super) opened_documents: Arc<Mutex<HashSet<Url>>>,
    pub(super) auto_open_documents: Arc<AtomicBool>,
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
            // Drop cannot await actor I/O. The unbounded control channel keeps
            // cancellation usable even when the bounded request queue is full.
            let _ = self.cancellations.send(self.request_id);
        }
    }
}

impl JsonRpcClient {
    pub(super) fn report_diagnostic(&self, message: impl Into<String>) {
        let _ = self
            .status_updates
            .send(LspStatusUpdate::Diagnostic(message.into()));
    }

    pub(super) async fn request<P, T>(&self, method: &str, params: P) -> Result<T>
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

    pub(super) async fn notify<P>(&self, method: &str, params: P) -> Result<()>
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

pub(super) fn spawn_json_rpc<R, W>(
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
    // Servers may send workspace/configuration before any user request and
    // wait for the reply before indexing. The reader therefore stays active
    // for the whole session, while the actor remains the sole stdin writer.
    let (command_sender, command_receiver) = mpsc::channel(32);
    let (cancellation_sender, cancellation_receiver) = mpsc::unbounded_channel();
    let (status_sender, status_receiver) = mpsc::unbounded_channel();
    let (incoming_sender, incoming_receiver) = mpsc::channel(64);
    let opened_documents = Arc::new(Mutex::new(HashSet::new()));
    let hierarchy_capabilities = Arc::new(ServerHierarchyCapabilities::default());
    let reader_task = tokio::spawn(read_messages(reader, incoming_sender));
    let server_context = LspServerContext {
        workspace_uri,
        workspace_name,
        hierarchy_capabilities: Arc::clone(&hierarchy_capabilities),
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

pub(super) fn response_id(message: &Value) -> Option<u64> {
    message.get("id").and_then(Value::as_u64)
}
