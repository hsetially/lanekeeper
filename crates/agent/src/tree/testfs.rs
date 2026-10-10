//! File operations for the unit tests in this directory, through cap-std like everything else in `src/` (the source
//! rule `all_file_access_via_cap_std` scans test code too).

use std::path::Path;

use crate::root::NfsRoot;

/// Write `content` to `rel` below `root`, creating directories on the way.
pub(crate) fn write(root: &Path, rel: &str, content: &[u8]) {
    let root = NfsRoot::open(root).unwrap();
    if let Some(parent) = Path::new(rel).parent().filter(|p| !p.as_os_str().is_empty()) {
        root.dir().create_dir_all(parent).unwrap();
    }
    root.dir().write(rel, content).unwrap();
}

pub(crate) fn create_dir(root: &Path, rel: &str) {
    NfsRoot::open(root).unwrap().dir().create_dir(rel).unwrap();
}

pub(crate) fn remove_file(root: &Path, rel: &str) {
    NfsRoot::open(root).unwrap().dir().remove_file(rel).unwrap();
}
