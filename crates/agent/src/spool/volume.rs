//! The spool directory as a cap-std handle (S17).
//!
//! This is the one place the spool meets ambient authority: the directory is opened once, at start-up, by the path in
//! `LK_SPOOL_DIR`. Every later operation names a file inside it by a name the spool made itself (`seg-<hex>.lks`,
//! `state`), never by anything from the hub or from a file's content, and goes through the [`Dir`] this holds.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cap_std::ambient_authority;
use cap_std::fs::Dir;

use super::SpoolError;

/// The spool directory, as a capability.
#[derive(Debug, Clone)]
pub struct SpoolVolume {
    dir: Arc<Dir>,
    path: PathBuf,
}

impl SpoolVolume {
    /// Open the directory at `path`. Blocking: call it before the runtime starts, or inside `spawn_blocking`. The
    /// directory must exist (it is a mounted volume); the spool does not create it.
    pub fn open(path: &Path) -> Result<Self, SpoolError> {
        let dir =
            Dir::open_ambient_dir(path, ambient_authority()).map_err(|e| SpoolError::Volume(e.kind()))?;
        Ok(Self {
            dir: Arc::new(dir),
            path: path.to_path_buf(),
        })
    }

    pub(super) fn dir(&self) -> &Dir {
        &self.dir
    }

    /// Where the volume is mounted, for log lines. Never used to open anything.
    pub fn path(&self) -> &Path {
        &self.path
    }
}
