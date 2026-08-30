use std::{io, sync::Arc};

use serde::Serialize;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixStream, unix::OwnedWriteHalf},
    sync::mpsc,
};

use super::{
    IpcCommand, IpcResponder,
    protocol::{Envelope, IpcRequest, IpcResponse, PROTOCOL_VERSION},
};

const CLIENT_QUEUE_CAPACITY: usize = 16;
pub(super) const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub(super) fn spawn_client_connection(
    client_id: u64,
    stream: UnixStream,
    command_sender: mpsc::Sender<IpcCommand>,
    disconnected_sender: mpsc::UnboundedSender<u64>,
) -> mpsc::Sender<Arc<[u8]>> {
    let (response_sender, receiver) = mpsc::channel(CLIENT_QUEUE_CAPACITY);
    let task_response_sender = response_sender.clone();
    let _connection_task = tokio::spawn(async move {
        let (reader, writer) = stream.into_split();
        let read = read_client_requests(reader, task_response_sender, command_sender);
        let mut writer_task = tokio::spawn(write_client_frames(writer, receiver));
        tokio::pin!(read);
        tokio::select! {
            _ = &mut read => {
                let _ = disconnected_sender.send(client_id);
                // Protocol errors are queued before the reader returns. Let the
                // sole writer drain them so clients receive the error before EOF.
                let _ = writer_task.await;
            }
            _ = &mut writer_task => {
                let _ = disconnected_sender.send(client_id);
            }
        }
    });
    response_sender
}

async fn read_client_requests(
    reader: tokio::net::unix::OwnedReadHalf,
    response_sender: mpsc::Sender<Arc<[u8]>>,
    command_sender: mpsc::Sender<IpcCommand>,
) -> io::Result<()> {
    let mut reader = BufReader::new(reader);
    loop {
        let mut frame = Vec::new();
        let bytes_read = (&mut reader)
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut frame)
            .await?;
        if bytes_read == 0 {
            return Ok(());
        }
        if frame.len() > MAX_FRAME_BYTES {
            send_protocol_error(&response_sender, None, "IPC frame exceeds 1 MiB").await?;
            return Ok(());
        }
        if frame.last() != Some(&b'\n') {
            send_protocol_error(&response_sender, None, "IPC frame must end with a newline")
                .await?;
            return Ok(());
        }
        frame.pop();
        if frame.last() == Some(&b'\r') {
            frame.pop();
        }

        let envelope: Envelope<Value> = match serde_json::from_slice(&frame) {
            Ok(envelope) => envelope,
            Err(error) => {
                send_protocol_error(
                    &response_sender,
                    None,
                    &format!("invalid IPC JSON: {error}"),
                )
                .await?;
                continue;
            }
        };
        if envelope.version != PROTOCOL_VERSION {
            send_protocol_error(
                &response_sender,
                envelope.request_id,
                &format!(
                    "unsupported IPC protocol version {}; expected {}",
                    envelope.version, PROTOCOL_VERSION
                ),
            )
            .await?;
            continue;
        }
        let Some(request_id) = envelope.request_id else {
            send_protocol_error(&response_sender, None, "IPC requests require a request_id")
                .await?;
            continue;
        };
        let request: IpcRequest = match serde_json::from_value(envelope.payload) {
            Ok(request) => request,
            Err(error) => {
                send_protocol_error(
                    &response_sender,
                    Some(request_id),
                    &format!("invalid IPC request: {error}"),
                )
                .await?;
                continue;
            }
        };
        let responder = IpcResponder {
            request_id,
            sender: response_sender.clone(),
        };
        if command_sender
            .send(IpcCommand::new(request_id, request, responder))
            .await
            .is_err()
        {
            send_protocol_error(
                &response_sender,
                Some(request_id),
                "cgraph command loop is no longer available",
            )
            .await?;
            return Ok(());
        }
    }
}

async fn write_client_frames(
    mut writer: OwnedWriteHalf,
    mut receiver: mpsc::Receiver<Arc<[u8]>>,
) -> io::Result<()> {
    while let Some(frame) = receiver.recv().await {
        writer.write_all(&frame).await?;
    }
    Ok(())
}

async fn send_protocol_error(
    sender: &mpsc::Sender<Arc<[u8]>>,
    request_id: Option<u64>,
    message: &str,
) -> io::Result<()> {
    sender
        .send(encode_frame(
            request_id,
            IpcResponse::Error {
                message: message.to_owned(),
            },
        )?)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "IPC client disconnected"))
}

pub(super) fn encode_frame<T: Serialize>(
    request_id: Option<u64>,
    payload: T,
) -> io::Result<Arc<[u8]>> {
    let mut frame =
        serde_json::to_vec(&Envelope::new(request_id, payload)).map_err(io::Error::other)?;
    frame.push(b'\n');
    Ok(Arc::from(frame))
}
