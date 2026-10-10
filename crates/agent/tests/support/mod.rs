//! Helpers shared by the integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

pub mod clock;
pub mod fake_hub;
pub mod fake_kube;
pub mod fake_metadata;
pub mod log_capture;
pub mod raw_server;
pub mod test_ca;

use std::collections::HashMap;
use std::path::Path;

/// The smallest environment that parses, with the NFS root at `root`.
pub fn valid_env(root: &Path) -> HashMap<String, String> {
    [
        ("LK_HUB_ENDPOINT", "https://hub.example.com:8443"),
        ("LK_HUB_AUDIENCE", "https://hub.example.com"),
        ("LK_HUB_CA_FILE", "/etc/lanekeeper/hub-ca.pem"),
        ("LK_SWIMLANE", "sit1"),
        ("LK_CLUSTER", "gke-sit1"),
        ("LK_PROJECT", "bank-sit"),
        ("LK_NFS_SERVER", "10.1.2.3"),
        ("LK_NFS_EXPORT", "/export/csp"),
        ("LK_CERT_SECRET", "lanekeeper-agent-cert"),
        ("LK_NAMESPACES", "sit1"),
        // Test roots live under /tmp, which is the default temp directory, so both are moved out of the way.
        ("LK_SPOOL_DIR", "/lk-test/spool"),
        ("LK_TMP_DIR", "/lk-test/tmp"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .chain([(
        "LK_NFS_ROOT".to_owned(),
        root.to_str().unwrap_or_default().to_owned(),
    )])
    .collect()
}
