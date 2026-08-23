use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::timeout,
};

use super::{IpcServer, MAX_FRAME_BYTES, socket::marker_path};
use crate::{
    ipc::protocol::{Envelope, IpcEvent, IpcRequest, IpcResponse, PROTOCOL_VERSION},
    state::{HierarchyKind, SourceLocation},
};

#[tokio::test]
async fn broadcasts_open_locations_to_all_connected_clients() {
    let directory = temporary_directory("broadcast");
    let socket_path = directory.join("cgraph.sock");
    let server = IpcServer::start(&socket_path).unwrap();
    assert_eq!(
        fs::metadata(&socket_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(marker_path(&socket_path))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let no_client_error = server
        .event_sender()
        .send_open_location(&SourceLocation {
            uri: "file:///workspace/src/main.rs".to_owned(),
            line: Some(8),
            character: Some(3),
        })
        .unwrap_err();
    assert!(no_client_error.to_string().contains("no IPC editor client"));
    let first = UnixStream::connect(&socket_path).await.unwrap();
    let second = UnixStream::connect(&socket_path).await.unwrap();
    wait_for_clients(&server, 2).await;

    let delivered = server
        .event_sender()
        .send_open_location(&SourceLocation {
            uri: "file:///workspace/src/main.rs".to_owned(),
            line: Some(8),
            character: Some(3),
        })
        .unwrap();

    assert_eq!(delivered, 2);
    for stream in [first, second] {
        let mut line = String::new();
        timeout(
            Duration::from_secs(1),
            BufReader::new(stream).read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        let message: Envelope<IpcEvent> = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(message.version, PROTOCOL_VERSION);
        assert_eq!(message.request_id, None);
        assert_eq!(
            message.payload,
            IpcEvent::OpenLocation {
                uri: "file:///workspace/src/main.rs".to_owned(),
                line: 8,
                character: 3,
            }
        );
    }
    wait_for_clients(&server, 0).await;

    server.shutdown().await.unwrap();
    assert!(!socket_path.exists());
    assert!(!marker_path(&socket_path).exists());
    fs::remove_dir(directory).unwrap();
}

#[tokio::test]
async fn shutdown_does_not_remove_a_replacement_socket() {
    let directory = temporary_directory("replacement");
    let socket_path = directory.join("cgraph.sock");
    let server = IpcServer::start(&socket_path).unwrap();
    fs::remove_file(&socket_path).unwrap();
    let replacement = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

    server.shutdown().await.unwrap();

    assert!(socket_path.exists());
    assert!(!marker_path(&socket_path).exists());
    drop(replacement);
    fs::remove_file(socket_path).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[tokio::test]
async fn routes_valid_requests_and_rejects_incompatible_versions() {
    let directory = temporary_directory("requests");
    let socket_path = directory.join("cgraph.sock");
    let mut server = IpcServer::start(&socket_path).unwrap();
    let mut commands = server.take_command_receiver().unwrap();
    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let request = IpcRequest::FocusSymbol {
        hierarchy: HierarchyKind::Call,
        symbol: "main".to_owned(),
        location: Some(SourceLocation {
            uri: "file:///workspace/src/main.rs".to_owned(),
            line: Some(7),
            character: Some(2),
        }),
    };
    let mut frame = serde_json::to_vec(&Envelope::new(Some(42), request.clone())).unwrap();
    frame.push(b'\n');
    writer.write_all(&frame).await.unwrap();

    let command = timeout(Duration::from_secs(1), commands.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(command.request_id(), 42);
    assert_eq!(command.request(), &request);
    let (_, responder) = command.into_parts();
    responder.respond(IpcResponse::Accepted).unwrap();
    let response: Envelope<IpcResponse> = read_envelope(&mut reader).await;
    assert_eq!(response.request_id, Some(42));
    assert_eq!(response.payload, IpcResponse::Accepted);

    let incompatible = Envelope {
        version: PROTOCOL_VERSION + 1,
        request_id: Some(43),
        payload: request,
    };
    let mut frame = serde_json::to_vec(&incompatible).unwrap();
    frame.push(b'\n');
    writer.write_all(&frame).await.unwrap();
    let response: Envelope<IpcResponse> = read_envelope(&mut reader).await;
    assert_eq!(response.request_id, Some(43));
    let IpcResponse::Error { message } = response.payload else {
        panic!("incompatible protocol version must return an error");
    };
    assert!(message.contains("unsupported IPC protocol version"));
    assert!(commands.try_recv().is_err());

    server.shutdown().await.unwrap();
    fs::remove_dir(directory).unwrap();
}

#[tokio::test]
async fn rejects_oversized_inbound_frames_before_deserialization() {
    let directory = temporary_directory("oversized-frame");
    let socket_path = directory.join("cgraph.sock");
    let mut server = IpcServer::start(&socket_path).unwrap();
    let mut commands = server.take_command_receiver().unwrap();
    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut frame = vec![b' '; MAX_FRAME_BYTES + 1];
    frame.push(b'\n');

    writer.write_all(&frame).await.unwrap();

    let response: Envelope<IpcResponse> = read_envelope(&mut reader).await;
    assert_eq!(response.request_id, None);
    let IpcResponse::Error { message } = response.payload else {
        panic!("oversized frame must return an error");
    };
    assert!(message.contains("exceeds 1 MiB"));
    assert!(commands.try_recv().is_err());

    server.shutdown().await.unwrap();
    fs::remove_dir(directory).unwrap();
}

#[tokio::test]
async fn refuses_regular_files_and_only_reclaims_marked_stale_sockets() {
    let directory = temporary_directory("stale");
    let regular_path = directory.join("regular.sock");
    fs::write(&regular_path, "keep me").unwrap();
    let error = IpcServer::start(&regular_path).unwrap_err();
    assert!(error.to_string().contains("non-socket"));
    assert_eq!(fs::read_to_string(&regular_path).unwrap(), "keep me");

    let socket_path = directory.join("stale.sock");
    let active_server = IpcServer::start(&socket_path).unwrap();
    let active_error = IpcServer::start(&socket_path).unwrap_err();
    assert!(active_error.to_string().contains("already active"));
    active_server.shutdown().await.unwrap();

    let mut crashed_server = IpcServer::start(&socket_path).unwrap();
    crashed_server.task.as_ref().unwrap().abort();
    let _ = crashed_server.task.as_mut().unwrap().await;
    std::mem::forget(crashed_server);
    assert!(socket_path.exists());
    let replacement = IpcServer::start(&socket_path).unwrap();
    replacement.shutdown().await.unwrap();

    fs::remove_file(regular_path).unwrap();
    fs::remove_dir(directory).unwrap();

    let insecure_directory = temporary_directory("insecure-parent");
    fs::set_permissions(&insecure_directory, fs::Permissions::from_mode(0o777)).unwrap();
    let insecure_path = insecure_directory.join("cgraph.sock");
    let error = IpcServer::start(&insecure_path).unwrap_err();
    assert!(error.to_string().contains("group- or world-writable"));
    assert!(!insecure_path.exists());
    fs::set_permissions(&insecure_directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir(insecure_directory).unwrap();
}

async fn wait_for_clients(server: &IpcServer, expected: usize) {
    timeout(Duration::from_secs(1), async {
        while server.event_sender.connected_clients() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn read_envelope<T: serde::de::DeserializeOwned>(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Envelope<T> {
    let mut line = String::new();
    timeout(Duration::from_secs(1), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(line.trim_end()).unwrap()
}

fn temporary_directory(name: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("cgraph-ipc-{name}-{unique}"));
    fs::create_dir(&path).unwrap();
    path
}
