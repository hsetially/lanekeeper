//! Size limits shared by the hub, the agent and the sentinel.
//!
//! The byte and entry limits of a scan delta live with the domain type ([`domain::ScanDelta`]); they are
//! re-exported here, never redefined, so the two cannot drift. Every limit is checked by [`crate::convert`]
//! before a message becomes a domain value, and the gRPC limit is set on every client and server built by
//! [`crate::grpc`].

use domain::ScanDelta;

/// The largest message gRPC accepts or sends (4 MiB, tonic's default receive limit made explicit).
/// This applies after decompression, so a zstd bomb is cut off at the same size.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// The most file bytes one `ScanDelta` message may carry (3 MiB), which leaves room for the paths, hashes
/// and framing under [`MAX_MESSAGE_BYTES`]. This is [`domain::ScanDelta::MAX_BYTES`].
pub const MAX_SCAN_DELTA_BYTES: usize = ScanDelta::MAX_BYTES;

/// The most file bytes any other single message may carry: a write, a read result or a served response.
pub const MAX_FILE_BYTES: usize = MAX_SCAN_DELTA_BYTES;

/// The most items in any one repeated field: scan entries, removed paths, deployments, audit records.
/// This is [`domain::ScanDelta::MAX_ENTRIES`].
pub const MAX_ENTRIES: usize = ScanDelta::MAX_ENTRIES;

/// The most paths one `NotifyConfigServer` command may carry.
pub const MAX_NOTIFY_PATHS: usize = 1_000;

/// The most items in a configuration list: deny globs, the environment allowlist, tenants.
pub const MAX_CONFIG_ITEMS: usize = 1_000;

/// The largest certificate signing request (DER).
pub const MAX_CSR_BYTES: usize = 16 * 1024;

/// The longest bearer credential (Google ID token or join token).
pub const MAX_TOKEN_BYTES: usize = 8 * 1024;

/// The most certificates in an issued chain, and the largest one (DER).
pub const MAX_CERT_CHAIN: usize = 8;
pub const MAX_CERT_BYTES: usize = 16 * 1024;
