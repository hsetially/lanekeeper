//! Entry points for the fuzz target `registry_delta` and the replay of its seeds (S22). Not part of the API.
//!
//! The target cannot start a database, so it exercises what is pure: the checks a scan delta passes before anything is stored
//! ([`super::validate`]), the bounds on a whole logical delta, and the decision about what becomes of the versions of a path
//! ([`super::plan`]). The invariants it asserts are written in `fuzz/invariants.rs`.

use std::collections::HashMap;

use bytes::Bytes;
use domain::{ContentHash, FileClass, NfsPath, ScanDelta, Timestamp};
use engine::DenyMatcher;

use super::plan::{PlanCtx, Stored, Version, plan_path};
use super::validate::{Bounds, Rules, StagedPart, Totals, validate};
use crate::error::RegistryError;

/// One entry the hub accepted, as it keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedEntry {
    pub path: NfsPath,
    pub hash: ContentHash,
    pub drift_hash: ContentHash,
    pub size: u64,
    pub observed_at_ms: i64,
    pub denied: bool,
    pub class: FileClass,
    pub has_content: bool,
}

/// A message the hub accepted.
#[derive(Debug)]
pub struct Checked {
    pub entries: Vec<CheckedEntry>,
    /// The content to store, in the order of the entries that have it.
    pub contents: Vec<Bytes>,
    pub removed: usize,
    pub skipped: usize,
    pub bytes: u64,
    /// The staged form of the message survives a trip through JSON, which is how a part waits in `delta_staging`.
    pub round_trips: bool,
}

/// The checks of `delta`, on one message.
pub fn check_message(
    d: ScanDelta,
    deny: &DenyMatcher,
    max_file_bytes: u64,
    now_ms: i64,
    max_skew_ms: i64,
) -> Result<Checked, RegistryError> {
    let v = validate(
        d,
        &Rules {
            deny,
            max_file_bytes,
            now_ms,
            max_skew_ms,
        },
    )?;
    let round_trips = serde_json::to_value(&v.staged)
        .ok()
        .and_then(|j| serde_json::from_value::<StagedPart>(j).ok())
        .is_some_and(|back| back == v.staged);
    Ok(Checked {
        entries: v
            .staged
            .entries
            .iter()
            .map(|e| CheckedEntry {
                path: e.path.clone(),
                hash: e.hash,
                drift_hash: e.drift_hash,
                size: e.size,
                observed_at_ms: e.observed_at_ms,
                denied: e.denied,
                class: e.class,
                has_content: e.has_content,
            })
            .collect(),
        contents: v.contents,
        removed: v.staged.removed.len(),
        skipped: v.staged.skipped.len(),
        bytes: v.staged.bytes,
        round_trips,
    })
}

/// What a logical delta holds: parts, entries and removals and skips, and content bytes.
pub type Held = (u64, u64, u64);

/// Add one accepted message to a logical delta, or `TooLarge` when that passes `bounds` (parts, items, bytes).
pub fn add_part(held: Held, items: u64, bytes: u64, bounds: Held) -> Result<Held, RegistryError> {
    let totals = Totals {
        parts: held.0,
        items: held.1,
        bytes: held.2,
    };
    let bounds = Bounds {
        parts: bounds.0,
        items: bounds.1,
        bytes: bounds.2,
    };
    let next = totals.with_counts(items, bytes, bounds)?;
    Ok((next.parts, next.items, next.bytes))
}

/// One step of the plan for a path: the version, what it replaced, and whether it became the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub hash: Option<ContentHash>,
    pub before: Option<ContentHash>,
    pub moves: bool,
    pub at_ms: i64,
}

/// Plan the versions `(hash, observed_at_ms)` of one path against what is stored (`None`: never seen; otherwise the hash, if
/// any, and the time). `removal` appends a removal.
pub fn plan_versions(
    stored: Option<(Option<ContentHash>, i64)>,
    versions: &[(ContentHash, i64)],
    removal: bool,
    delta_time_ms: i64,
    initial_inventory: bool,
) -> Vec<Step> {
    let Ok(path) = NfsPath::parse("fuzz/path.yml") else {
        return Vec::new();
    };
    let stored = stored.map(|(hash, observed_at_ms)| Stored {
        hash,
        observed: true,
        observed_at_ms,
        class: FileClass::Structured,
        denied: false,
        state: None,
        drift_hash: None,
    });
    let mut all: Vec<Version> = versions
        .iter()
        .map(|(hash, at)| Version::File {
            entry: Box::new(super::validate::StagedEntry {
                path: path.clone(),
                hash: *hash,
                drift_hash: *hash,
                size: 0,
                mtime_ms: 0,
                observed_at_ms: *at,
                denied: false,
                class: FileClass::Structured,
                eol: None,
                encoding: None,
                bom: None,
                has_content: true,
            }),
            during_job: None,
        })
        .collect();
    if removal {
        all.push(Version::Removal);
    }
    let writes = HashMap::new();
    let ctx = PlanCtx {
        initial_inventory,
        delta_time_ms,
        windows: &[],
        tool_writes: &writes,
    };
    plan_path(&path, stored.as_ref(), &all, &ctx)
        .candidates
        .into_iter()
        .map(|c| Step {
            hash: c.hash,
            before: c.before,
            moves: c.moves_state,
            at_ms: c.observed_at_ms,
        })
        .collect()
}

/// A timestamp for the fuzz target's messages.
pub fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_millis(ms)
}
