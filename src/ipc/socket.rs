use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

const OWNER_MARKER_MAGIC: &str = "cgraph-ipc-v1";

#[derive(Debug)]
pub(super) struct SocketGuard {
    socket_path: PathBuf,
    marker_path: PathBuf,
    identity: SocketIdentity,
}

impl SocketGuard {
    pub(super) fn create(socket_path: PathBuf) -> Result<Self> {
        let metadata = fs::symlink_metadata(&socket_path)?;
        let identity = SocketIdentity::from_metadata(&metadata);
        let marker_path = marker_path(&socket_path);
        let marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&marker_path)
            .with_context(|| {
                format!(
                    "failed to create IPC ownership marker {}",
                    marker_path.display()
                )
            });
        let mut marker = match marker {
            Ok(marker) => marker,
            Err(error) => {
                remove_socket_if_identity_matches(&socket_path, identity);
                return Err(error);
            }
        };
        if let Err(error) = marker
            .write_all(identity.marker_contents().as_bytes())
            .and_then(|()| marker.sync_all())
        {
            remove_socket_if_identity_matches(&socket_path, identity);
            let _ = fs::remove_file(&marker_path);
            return Err(error.into());
        }
        Ok(Self {
            socket_path,
            marker_path,
            identity,
        })
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if read_marker(&self.marker_path) == Some(self.identity) {
            if socket_identity(&self.socket_path) == Some(self.identity) {
                let _ = fs::remove_file(&self.socket_path);
            }
            let _ = fs::remove_file(&self.marker_path);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SocketIdentity {
    device: u64,
    inode: u64,
}

impl SocketIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    fn marker_contents(self) -> String {
        format!("{OWNER_MARKER_MAGIC} {} {}\n", self.device, self.inode)
    }
}

pub(super) fn marker_path(socket_path: &Path) -> PathBuf {
    let mut marker = socket_path.as_os_str().to_owned();
    marker.push(".cgraph-owner");
    PathBuf::from(marker)
}

pub(super) fn socket_identity(path: &Path) -> Option<SocketIdentity> {
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata
        .file_type()
        .is_socket()
        .then(|| SocketIdentity::from_metadata(&metadata))
}

pub(super) fn remove_socket_if_identity_matches(path: &Path, identity: SocketIdentity) {
    if socket_identity(path) == Some(identity) {
        let _ = fs::remove_file(path);
    }
}

fn read_marker(path: &Path) -> Option<SocketIdentity> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    let mut fields = contents.split_whitespace();
    if fields.next()? != OWNER_MARKER_MAGIC {
        return None;
    }
    let identity = SocketIdentity {
        device: fields.next()?.parse().ok()?,
        inode: fields.next()?.parse().ok()?,
    };
    fields.next().is_none().then_some(identity)
}

pub(super) fn prepare_socket_path(socket_path: &Path) -> Result<()> {
    let marker_path = marker_path(socket_path);
    let metadata = match fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if read_marker(&marker_path).is_some() {
                fs::remove_file(&marker_path).with_context(|| {
                    format!(
                        "failed to remove orphan IPC marker {}",
                        marker_path.display()
                    )
                })?;
            } else if fs::symlink_metadata(&marker_path).is_ok() {
                bail!(
                    "refusing to replace unrecognized IPC marker {}",
                    marker_path.display()
                );
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_socket() {
        bail!(
            "refusing to replace non-socket IPC path {}",
            socket_path.display()
        );
    }
    let identity = SocketIdentity::from_metadata(&metadata);
    if read_marker(&marker_path) != Some(identity) {
        bail!(
            "refusing to remove IPC socket without a matching cgraph marker: {}",
            socket_path.display()
        );
    }
    match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(_) => bail!("IPC socket is already active: {}", socket_path.display()),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "could not prove IPC socket is stale: {}",
                    socket_path.display()
                )
            });
        }
    }
    if socket_identity(socket_path) != Some(identity) {
        bail!(
            "IPC socket changed while checking whether it was stale: {}",
            socket_path.display()
        );
    }
    fs::remove_file(socket_path)?;
    fs::remove_file(marker_path)?;
    Ok(())
}

pub(super) fn validate_socket_parent(socket_path: &Path) -> Result<()> {
    let parent = socket_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::symlink_metadata(parent).with_context(|| {
        format!(
            "IPC socket parent directory does not exist: {}",
            parent.display()
        )
    })?;
    if !metadata.file_type().is_dir() {
        bail!(
            "IPC socket parent must be a real directory, not a symlink or file: {}",
            parent.display()
        );
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        bail!(
            "IPC socket parent must not be group- or world-writable: {}",
            parent.display()
        );
    }
    Ok(())
}
