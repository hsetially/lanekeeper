//! Which file a request for a resource gets (D82): the whole file, never a merge.
//!
//! For a file `<folder>/<name>.<ext>` and a tenant, the candidates are, in order:
//!
//! 1. when the request names a channel: `<folder>/<channel>/<name>-<tenant>.<ext>`, then `<folder>/<channel>/<name>.<ext>`;
//! 2. `<folder>/<name>-<tenant>.<ext>`, then `<folder>/<name>.<ext>`.
//!
//! The first that exists is the file. A tenant's copy is a whole document, never combined with the base copy. A file with no
//! extension carries the suffix at the end of its name (`mappings-sit1`).

use std::collections::BTreeSet;

use domain::{ChannelName, LogicalFile, NfsPath, TenantId};

use super::role::split_name;

/// The file a request gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceChoice {
    pub path: NfsPath,
    /// The chosen file is the tenant's own copy (`<name>-<tenant>.<ext>`), not the base file.
    pub tenant_copy: bool,
    /// The chosen file is in the channel's folder.
    pub in_channel: bool,
}

fn join(dir: &NfsPath, name: &str) -> Option<NfsPath> {
    NfsPath::parse(&if dir.is_root() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    })
    .ok()
}

/// The file that the config-server serves for `file`, among `files` (the NFS paths of the swimlane), or `None` when no
/// candidate exists. `file` is the logical file: the folder and name of the base file, whether or not a base file exists.
pub fn resolve_resource(
    file: &LogicalFile,
    tenant: Option<&TenantId>,
    channel: Option<&ChannelName>,
    files: &BTreeSet<NfsPath>,
) -> Option<ResourceChoice> {
    let folder = file.as_path().parent();
    let name = file.as_path().file_name();
    let tenant_name = tenant.map(|t| {
        let (stem, ext) = split_name(name);
        ext.map_or_else(|| format!("{stem}-{t}"), |ext| format!("{stem}-{t}.{ext}"))
    });

    let mut folders: Vec<(NfsPath, bool)> = Vec::with_capacity(2);
    if let Some(channel) = channel {
        if let Some(dir) = join(&folder, channel.as_str()) {
            folders.push((dir, true));
        }
    }
    folders.push((folder, false));

    for (dir, in_channel) in folders {
        if let Some(path) = tenant_name.as_deref().and_then(|n| join(&dir, n)) {
            if files.contains(&path) {
                return Some(ResourceChoice {
                    path,
                    tenant_copy: true,
                    in_channel,
                });
            }
        }
        if let Some(path) = join(&dir, name) {
            if files.contains(&path) {
                return Some(ResourceChoice {
                    path,
                    tenant_copy: false,
                    in_channel,
                });
            }
        }
    }
    None
}
