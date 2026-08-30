#![doc = include_str!("README.md")]

use std::{
    collections::HashMap,
    fs, io,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use tokio::{
    net::UnixListener,
    runtime::Handle,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{
    ipc::protocol::{IpcEvent, IpcRequest, IpcResponse},
    state::SourceLocation,
};

mod connection;
pub mod protocol;
mod socket;

#[cfg(test)]
use connection::MAX_FRAME_BYTES;
use connection::{encode_frame, spawn_client_connection};
use socket::{
    SocketGuard, prepare_socket_path, remove_socket_if_identity_matches, socket_identity,
    validate_socket_parent,
};

const COMMAND_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Debug)]
pub struct IpcEventSender {
    sender: mpsc::UnboundedSender<IpcEvent>,
    client_count: Arc<AtomicUsize>,
}

impl IpcEventSender {
    pub fn send_open_location(&self, location: &SourceLocation) -> Result<usize> {
        let Some(line) = location.line else {
            bail!("selected node has no source line");
        };
        let Some(character) = location.character else {
            bail!("selected node has no source column");
        };
        if location.uri.is_empty() {
            bail!("selected node has an empty source URI");
        }
        let client_count = self.client_count.load(Ordering::Acquire);
        if client_count == 0 {
            bail!("no IPC editor client is connected");
        }
        self.sender
            .send(IpcEvent::OpenLocation {
                uri: location.uri.clone(),
                line,
                character,
            })
            .map_err(|_| anyhow::anyhow!("IPC server is no longer running"))?;
        Ok(client_count)
    }

    pub fn connected_clients(&self) -> usize {
        self.client_count.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub struct IpcCommand {
    request_id: u64,
    request: IpcRequest,
    responder: IpcResponder,
}

impl IpcCommand {
    pub(crate) fn new(request_id: u64, request: IpcRequest, responder: IpcResponder) -> Self {
        Self {
            request_id,
            request,
            responder,
        }
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn request(&self) -> &IpcRequest {
        &self.request
    }

    pub fn into_parts(self) -> (IpcRequest, IpcResponder) {
        (self.request, self.responder)
    }

    #[cfg(test)]
    pub(crate) fn test_command(
        request_id: u64,
        request: IpcRequest,
    ) -> (Self, mpsc::Receiver<Arc<[u8]>>) {
        let (sender, receiver) = mpsc::channel(1);
        let responder = IpcResponder { request_id, sender };
        (Self::new(request_id, request, responder), receiver)
    }
}

#[derive(Clone, Debug)]
pub struct IpcResponder {
    request_id: u64,
    sender: mpsc::Sender<Arc<[u8]>>,
}

impl IpcResponder {
    pub fn respond(self, response: IpcResponse) -> Result<()> {
        let frame = encode_frame(Some(self.request_id), response)?;
        self.sender.try_send(frame).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                anyhow::anyhow!("IPC client response queue is full")
            }
            mpsc::error::TrySendError::Closed(_) => {
                anyhow::anyhow!("IPC client disconnected before its response")
            }
        })
    }
}

#[derive(Debug)]
pub struct IpcServer {
    event_sender: IpcEventSender,
    command_receiver: Option<mpsc::Receiver<IpcCommand>>,
    shutdown_sender: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<io::Result<()>>>,
    socket_guard: Option<SocketGuard>,
}

impl IpcServer {
    pub fn start(socket_path: impl Into<PathBuf>) -> Result<Self> {
        let socket_path = socket_path.into();
        let runtime = Handle::try_current().context("IPC server requires a Tokio runtime")?;
        validate_socket_parent(&socket_path)?;
        prepare_socket_path(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("failed to bind IPC socket {}", socket_path.display()))?;
        let bound_identity = socket_identity(&socket_path)
            .context("bound IPC socket disappeared before permission setup")?;
        if let Err(error) = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)) {
            remove_socket_if_identity_matches(&socket_path, bound_identity);
            return Err(error).with_context(|| {
                format!(
                    "failed to restrict IPC socket permissions for {}",
                    socket_path.display()
                )
            });
        }
        let socket_guard = SocketGuard::create(socket_path.clone())?;
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let (command_sender, command_receiver) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let client_count = Arc::new(AtomicUsize::new(0));
        let task_client_count = Arc::clone(&client_count);
        let task = runtime.spawn(async move {
            let result = run_server(
                listener,
                event_receiver,
                command_sender,
                shutdown_receiver,
                Arc::clone(&task_client_count),
            )
            .await;
            task_client_count.store(0, Ordering::Release);
            result
        });

        Ok(Self {
            event_sender: IpcEventSender {
                sender: event_sender,
                client_count,
            },
            command_receiver: Some(command_receiver),
            shutdown_sender: Some(shutdown_sender),
            task: Some(task),
            socket_guard: Some(socket_guard),
        })
    }

    pub fn event_sender(&self) -> IpcEventSender {
        self.event_sender.clone()
    }

    pub fn take_command_receiver(&mut self) -> Option<mpsc::Receiver<IpcCommand>> {
        self.command_receiver.take()
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        if let Some(task) = self.task.take() {
            task.await
                .context("IPC server task failed")?
                .context("IPC server stopped with an I/O error")?;
        }
        self.socket_guard.take();
        Ok(())
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn run_server(
    listener: UnixListener,
    mut event_receiver: mpsc::UnboundedReceiver<IpcEvent>,
    command_sender: mpsc::Sender<IpcCommand>,
    mut shutdown_receiver: oneshot::Receiver<()>,
    client_count: Arc<AtomicUsize>,
) -> io::Result<()> {
    let (disconnected_sender, mut disconnected_receiver) = mpsc::unbounded_channel();
    let mut clients = HashMap::<u64, mpsc::Sender<Arc<[u8]>>>::new();
    let next_client_id = AtomicU64::new(1);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let client_id = next_client_id.fetch_add(1, Ordering::Relaxed);
                let sender = spawn_client_connection(
                    client_id,
                    stream,
                    command_sender.clone(),
                    disconnected_sender.clone(),
                );
                clients.insert(client_id, sender);
                client_count.store(clients.len(), Ordering::Release);
            }
            Some(event) = event_receiver.recv() => {
                let frame = encode_frame(None, event)?;
                clients.retain(|_, sender| sender.try_send(Arc::clone(&frame)).is_ok());
                client_count.store(clients.len(), Ordering::Release);
            }
            Some(client_id) = disconnected_receiver.recv() => {
                clients.remove(&client_id);
                client_count.store(clients.len(), Ordering::Release);
            }
            _ = &mut shutdown_receiver => break,
        }
    }

    clients.clear();
    client_count.store(0, Ordering::Release);
    Ok(())
}

#[cfg(test)]
mod tests;
