#![doc = include_str!("README.md")]

use std::path::{Path, PathBuf};

pub mod protocol;

/// Configuration holder for the future Unix socket server.
///
/// There is deliberately no `start` method yet: binding and stale-socket
/// cleanup are security-sensitive and need the lifecycle rules in README.md.
#[derive(Debug)]
pub struct IpcServer {
    socket_path: PathBuf,
}

impl IpcServer {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}
