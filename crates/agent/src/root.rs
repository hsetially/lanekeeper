//! The NFS root as a cap-std handle (T1, S17).
//!
//! The root is opened once, here, with ambient authority. Every later file operation goes through the [`cap_std::fs::Dir`]
//! it holds, which cannot name anything outside the root: `..` and absolute paths are refused by cap-std itself.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::config::SettingsError;

/// Why the agent could not start. Paths are configuration, not file content, so they appear in the messages.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error("the NFS root {} does not exist; is the volume mounted?", .path.display())]
    RootMissing { path: PathBuf },
    #[error("the NFS root {} is not a directory", .path.display())]
    RootNotDirectory { path: PathBuf },
    #[error("the NFS root {} is not readable by this user", .path.display())]
    RootNotReadable { path: PathBuf },
    #[error("the NFS root {} could not be opened: {kind}", .path.display())]
    RootIo { path: PathBuf, kind: io::ErrorKind },
}

/// The mounted export, as a capability.
#[derive(Debug, Clone)]
pub struct NfsRoot {
    dir: Arc<Dir>,
    path: PathBuf,
}

impl NfsRoot {
    /// Open the mount at `path` and check that it can be listed. This is blocking file I/O: call it before the
    /// runtime starts, or inside `spawn_blocking`.
    pub fn open(path: &Path) -> Result<Self, StartupError> {
        let dir = Dir::open_ambient_dir(path, ambient_authority()).map_err(|e| classify(path, &e))?;
        // Listing proves the mount answers. An unreadable or stale export fails here, not at the first scan.
        if let Some(Err(e)) = dir.entries().map_err(|e| classify(path, &e))?.next() {
            return Err(classify(path, &e));
        }
        Ok(Self {
            dir: Arc::new(dir),
            path: path.to_path_buf(),
        })
    }

    /// The handle every file operation uses.
    pub fn dir(&self) -> &Dir {
        &self.dir
    }

    /// The handle, shared with the blocking tasks that scan and write.
    pub fn shared(&self) -> Arc<Dir> {
        Arc::clone(&self.dir)
    }

    /// Where the export is mounted. For `Hello` and for log lines; never used to open anything.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn classify(path: &Path, error: &io::Error) -> StartupError {
    let path = path.to_path_buf();
    match error.kind() {
        io::ErrorKind::NotFound => StartupError::RootMissing { path },
        io::ErrorKind::NotADirectory => StartupError::RootNotDirectory { path },
        io::ErrorKind::PermissionDenied => StartupError::RootNotReadable { path },
        kind => StartupError::RootIo { path, kind },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_map_to_distinct_startup_errors() {
        let p = Path::new("/mnt/csp");
        let of = |kind| classify(p, &io::Error::from(kind));
        assert!(matches!(
            of(io::ErrorKind::NotFound),
            StartupError::RootMissing { .. }
        ));
        assert!(matches!(
            of(io::ErrorKind::NotADirectory),
            StartupError::RootNotDirectory { .. }
        ));
        assert!(matches!(
            of(io::ErrorKind::PermissionDenied),
            StartupError::RootNotReadable { .. }
        ));
        assert!(matches!(
            of(io::ErrorKind::StaleNetworkFileHandle),
            StartupError::RootIo {
                kind: io::ErrorKind::StaleNetworkFileHandle,
                ..
            }
        ));
        // Every message names the path and tells the operator what to check.
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::NotADirectory,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::TimedOut,
        ] {
            assert!(of(kind).to_string().contains("/mnt/csp"));
        }
    }
}
