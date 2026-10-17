//! What the hub checks in a scan delta before it stores anything (S11, S17, D79).
//!
//! The agent is trusted to speak for its own swimlane and for nothing else, and even for that the hub checks what it can:
//!
//! - **Bounds.** A message has at most [`ScanDelta::MAX_ENTRIES`] entries (files, removals and skips together) and
//!   [`ScanDelta::MAX_BYTES`] bytes of content; a file is at most `max_file_bytes`. Over any of them the message is
//!   [`RegistryError::TooLarge`].
//! - **Bytes tell the truth.** The SHA-256 of an entry's bytes equals its declared hash, and its length its declared size.
//!   Otherwise the message is [`RegistryError::Invalid`] and nothing in it is kept.
//! - **Denied files carry no bytes** (D79). An entry the agent marks denied that comes with bytes is invalid. An entry that
//!   the hub's own deny globs match is denied whatever the agent said, and its bytes are dropped (defence in depth, S17): a
//!   key must not reach `blobs` because an agent's list was out of date.
//! - **Times are sane.** An `observed_at` outside the range Postgres can store is invalid, and one in the future is clamped to
//!   the hub's clock plus the allowed skew, so a wrong agent clock cannot make every later change look older than the stored one.
//!
//! The function is synchronous and does the hashing, the classification and the format detection, so the caller runs it on a
//! blocking thread when the message is large (rule 4).

use bytes::Bytes;
use domain::{ContentHash, EolStyle, FileClass, JobRef, NfsPath, ScanDelta, SpoolGap, TextEncoding};
use engine::{DenyMatcher, classify, detect_format, drift_hash};
use ports::content_hash;
use serde::{Deserialize, Serialize};

use crate::error::RegistryError;

/// 9999-12-31T23:59:59.999Z: the latest instant an `observed_at` may name.
pub(crate) const MAX_TS_MS: i64 = 253_402_300_799_999;
/// Messages up to this many content bytes are checked on the async thread; bigger ones on a blocking thread.
pub(crate) const INLINE_VALIDATION_BYTES: usize = 64 * 1024;

/// What the hub keeps of one file version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StagedEntry {
    pub path: NfsPath,
    /// The SHA-256 of the file as it is on NFS.
    pub hash: ContentHash,
    /// The hash for drift: line endings normalised for `structured` and `text` (D19). The raw hash when the hub has no bytes.
    pub drift_hash: ContentHash,
    pub size: u64,
    pub mtime_ms: i64,
    pub observed_at_ms: i64,
    pub denied: bool,
    pub class: FileClass,
    pub eol: Option<EolStyle>,
    pub encoding: Option<TextEncoding>,
    pub bom: Option<bool>,
    /// The blob is in `blobs` (false for a denied file, which is hashed and never stored).
    pub has_content: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StagedGap {
    pub from_ms: i64,
    pub to_ms: i64,
    pub lost: u64,
}

/// One message of a scan delta as the hub stores it while it waits for the rest (`delta_staging.metadata`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StagedPart {
    /// The format of this document. A part written by another version of the code is refused, not guessed at.
    pub v: u8,
    pub entries: Vec<StagedEntry>,
    pub removed: Vec<NfsPath>,
    pub skipped: Vec<NfsPath>,
    pub during_job: Option<JobRef>,
    pub gap: Option<StagedGap>,
    /// Content bytes of this part, for the bound on a whole logical delta.
    pub bytes: u64,
}

impl StagedPart {
    pub(crate) const VERSION: u8 = 1;

    /// Entries, removals and skips, for the bound on a whole logical delta.
    pub(crate) fn items(&self) -> usize {
        self.entries.len() + self.removed.len() + self.skipped.len()
    }
}

/// A message that passed every check, with the content to store.
#[derive(Debug)]
pub(crate) struct Validated {
    pub seq: u64,
    pub part: u32,
    pub more: bool,
    pub base_root: Option<ContentHash>,
    pub new_root: ContentHash,
    pub staged: StagedPart,
    /// The content of the entries that have `has_content`, in the order of `staged.entries`.
    pub contents: Vec<Bytes>,
}

/// What a logical delta holds so far, over all its parts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub parts: u64,
    pub items: u64,
    pub bytes: u64,
}

/// The bounds on a whole logical delta (rule 5): the number of parts, of entries and removals and skips, and of content bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Bounds {
    pub parts: u64,
    pub items: u64,
    pub bytes: u64,
}

impl Totals {
    /// The totals with one more part, or `TooLarge` when that would pass a bound.
    pub(crate) fn with(self, part: &StagedPart, bounds: Bounds) -> Result<Self, RegistryError> {
        self.with_counts(part.items() as u64, part.bytes, bounds)
    }

    /// The totals with one more part of `items` entries, removals and skips and `bytes` bytes of content.
    pub(crate) fn with_counts(self, items: u64, bytes: u64, bounds: Bounds) -> Result<Self, RegistryError> {
        let next = Self {
            parts: self.parts.saturating_add(1),
            items: self.items.saturating_add(items),
            bytes: self.bytes.saturating_add(bytes),
        };
        if next.parts > bounds.parts || next.items > bounds.items || next.bytes > bounds.bytes {
            return Err(RegistryError::TooLarge);
        }
        Ok(next)
    }
}

pub(crate) struct Rules<'a> {
    pub deny: &'a DenyMatcher,
    pub max_file_bytes: u64,
    pub now_ms: i64,
    pub max_skew_ms: i64,
}

fn observed(ms: i64, rules: &Rules<'_>) -> Result<i64, RegistryError> {
    if !(0..=MAX_TS_MS).contains(&ms) {
        return Err(RegistryError::Invalid);
    }
    Ok(ms.min(rules.now_ms.saturating_add(rules.max_skew_ms)))
}

/// `mtime` is information, not a decision input: a value Postgres cannot store is clamped, not refused.
fn mtime(ms: i64) -> i64 {
    ms.clamp(0, MAX_TS_MS)
}

pub(crate) fn validate(d: ScanDelta, rules: &Rules<'_>) -> Result<Validated, RegistryError> {
    if d.entries.len() + d.removed.len() + d.skipped.len() > ScanDelta::MAX_ENTRIES
        || d.payload_bytes() > ScanDelta::MAX_BYTES
    {
        return Err(RegistryError::TooLarge);
    }
    if d.removed.iter().any(NfsPath::is_root) || d.skipped.iter().any(|s| s.path.is_root()) {
        return Err(RegistryError::Invalid);
    }
    let mut entries = Vec::with_capacity(d.entries.len());
    let mut contents = Vec::new();
    let mut file_bytes = 0_u64;
    for e in d.entries {
        if e.path.is_root() {
            return Err(RegistryError::Invalid);
        }
        let observed_at_ms = observed(e.observed_at.unix_millis(), rules)?;
        if e.denied && e.bytes.is_some() {
            return Err(RegistryError::Invalid);
        }
        if let Some(bytes) = &e.bytes {
            let len = bytes.len() as u64;
            if len > rules.max_file_bytes {
                return Err(RegistryError::TooLarge);
            }
            if len != e.size || content_hash(bytes) != e.hash {
                return Err(RegistryError::Invalid);
            }
        }
        // The hub's own globs decide too: a file they match is denied whatever the agent said, and never stored.
        let denied = e.denied || rules.deny.is_denied(&e.path);
        let content = if denied { None } else { e.bytes };
        let class = if denied {
            FileClass::Denied
        } else {
            classify(&e.path, content.as_deref(), rules.deny)
        };
        let (eol, encoding, bom) = match (&content, class) {
            (Some(bytes), FileClass::Structured | FileClass::Text) => {
                let f = detect_format(bytes);
                (Some(f.eol), Some(f.encoding), Some(f.bom))
            }
            _ => (None, None, None),
        };
        let drift = match (&content, class) {
            (Some(bytes), FileClass::Structured | FileClass::Text | FileClass::Binary) => {
                drift_hash(Some(bytes), class).unwrap_or(e.hash)
            }
            _ => e.hash,
        };
        let has_content = content.is_some();
        if let Some(bytes) = content {
            file_bytes += bytes.len() as u64;
            contents.push(bytes);
        }
        entries.push(StagedEntry {
            path: e.path,
            hash: e.hash,
            drift_hash: drift,
            size: e.size,
            mtime_ms: mtime(e.mtime.unix_millis()),
            observed_at_ms,
            denied,
            class,
            eol,
            encoding,
            bom,
            has_content,
        });
    }
    Ok(Validated {
        seq: d.seq,
        part: d.part,
        more: d.more,
        base_root: d.base_root,
        new_root: d.new_root,
        staged: StagedPart {
            v: StagedPart::VERSION,
            entries,
            removed: d.removed,
            skipped: d.skipped.into_iter().map(|s| s.path).collect(),
            during_job: d.during_job,
            gap: d.gap.map(|g: SpoolGap| StagedGap {
                from_ms: g.from.unix_millis(),
                to_ms: g.to.unix_millis(),
                lost: g.lost_entries,
            }),
            bytes: file_bytes,
        },
        contents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ShortText, SkippedEntry, Timestamp};

    const NOW: i64 = 1_800_000_000_000;

    fn rules(deny: &DenyMatcher) -> Rules<'_> {
        Rules {
            deny,
            max_file_bytes: 2 * 1024 * 1024,
            now_ms: NOW,
            max_skew_ms: 5 * 60 * 1000,
        }
    }

    fn entry(path: &str, content: &'static [u8]) -> domain::ScanEntry {
        domain::ScanEntry {
            path: NfsPath::parse(path).unwrap(),
            hash: content_hash(content),
            size: content.len() as u64,
            mtime: Timestamp::from_unix_millis(NOW - 5_000),
            observed_at: Timestamp::from_unix_millis(NOW - 1_000),
            denied: false,
            bytes: Some(Bytes::from_static(content)),
        }
    }

    fn delta(entries: Vec<domain::ScanEntry>) -> ScanDelta {
        ScanDelta {
            seq: 7,
            base_root: None,
            new_root: ContentHash::from_bytes([9; 32]),
            entries,
            removed: Vec::new(),
            skipped: Vec::new(),
            during_job: None,
            more: false,
            part: 0,
            gap: None,
        }
    }

    #[test]
    fn a_good_entry_is_classified_hashed_and_its_content_kept() {
        let deny = DenyMatcher::default();
        let v = validate(
            delta(vec![entry("svc/app.yml", b"a: 1\r\nb: 2\r\n")]),
            &rules(&deny),
        )
        .unwrap();
        let e = &v.staged.entries[0];
        assert_eq!(e.class, FileClass::Structured);
        assert_eq!(e.eol, Some(EolStyle::Crlf));
        assert_eq!(e.encoding, Some(TextEncoding::Utf8));
        assert_eq!(e.bom, Some(false));
        assert!(e.has_content && !e.denied);
        // The drift hash is over the LF-normalised text (D19), the content hash over the bytes as they are.
        assert_ne!(e.drift_hash, e.hash);
        assert_eq!(e.drift_hash, content_hash(b"a: 1\nb: 2\n"));
        assert_eq!(v.contents.len(), 1);
        assert_eq!(v.staged.bytes, 12);
    }

    #[test]
    fn bytes_that_do_not_match_their_hash_or_size_are_invalid() {
        let deny = DenyMatcher::default();
        let mut wrong_hash = entry("a.yml", b"x: 1");
        wrong_hash.hash = content_hash(b"something else");
        assert_eq!(
            validate(delta(vec![wrong_hash]), &rules(&deny)).unwrap_err(),
            RegistryError::Invalid
        );
        let mut wrong_size = entry("a.yml", b"x: 1");
        wrong_size.size = 99;
        assert_eq!(
            validate(delta(vec![wrong_size]), &rules(&deny)).unwrap_err(),
            RegistryError::Invalid
        );
    }

    #[test]
    fn a_denied_entry_with_bytes_is_invalid_and_one_without_is_recorded_unread() {
        let deny = DenyMatcher::default();
        let mut with_bytes = entry("keys/app.pem", b"-----BEGIN-----");
        with_bytes.denied = true;
        assert_eq!(
            validate(delta(vec![with_bytes]), &rules(&deny)).unwrap_err(),
            RegistryError::Invalid
        );

        let mut bare = entry("keys/app.pem", b"-----BEGIN-----");
        bare.denied = true;
        bare.bytes = None;
        let v = validate(delta(vec![bare]), &rules(&deny)).unwrap();
        let e = &v.staged.entries[0];
        assert!(e.denied && !e.has_content);
        assert_eq!(e.class, FileClass::Denied);
        assert_eq!(
            e.drift_hash, e.hash,
            "the hub never has the bytes of a denied file"
        );
        assert!(v.contents.is_empty());
    }

    #[test]
    fn the_hubs_own_globs_deny_a_file_the_agent_did_not() {
        // S17, D79: a key must not reach `blobs` because an agent's list was out of date.
        let deny = DenyMatcher::default();
        let v = validate(delta(vec![entry("tls/server.key", b"PRIVATE")]), &rules(&deny)).unwrap();
        let e = &v.staged.entries[0];
        assert!(e.denied && !e.has_content);
        assert_eq!(e.class, FileClass::Denied);
        assert!(v.contents.is_empty(), "the bytes are dropped");
        assert_eq!(v.staged.bytes, 0);
    }

    #[test]
    fn admin_globs_deny_too() {
        let deny = DenyMatcher::with_globs(["secrets/*"]).unwrap();
        let v = validate(delta(vec![entry("secrets/token.yml", b"t: 1")]), &rules(&deny)).unwrap();
        assert!(v.staged.entries[0].denied);
    }

    #[test]
    fn size_and_count_limits() {
        let deny = DenyMatcher::default();
        let r = Rules {
            max_file_bytes: 8,
            ..rules(&deny)
        };
        assert_eq!(
            validate(delta(vec![entry("a.yml", b"123456789")]), &r).unwrap_err(),
            RegistryError::TooLarge,
            "a file over max_file_bytes"
        );
        let many = (0..=ScanDelta::MAX_ENTRIES)
            .map(|i| entry(&format!("f{i}.yml"), b"x"))
            .collect();
        assert_eq!(
            validate(delta(many), &rules(&deny)).unwrap_err(),
            RegistryError::TooLarge,
            "more entries than a message may carry"
        );
        // Removals and skips count against the same bound.
        let mut d = delta(vec![entry("a.yml", b"x")]);
        d.removed = (0..ScanDelta::MAX_ENTRIES)
            .map(|i| NfsPath::parse(&format!("r{i}.yml")).unwrap())
            .collect();
        assert_eq!(validate(d, &rules(&deny)).unwrap_err(), RegistryError::TooLarge);
        let mut d = delta(vec![]);
        d.skipped = (0..=ScanDelta::MAX_ENTRIES)
            .map(|i| SkippedEntry {
                path: NfsPath::parse(&format!("s{i}.bin")).unwrap(),
                reason: ShortText::parse("too large").unwrap(),
            })
            .collect();
        assert_eq!(validate(d, &rules(&deny)).unwrap_err(), RegistryError::TooLarge);
    }

    #[test]
    fn times_are_range_checked_and_the_future_is_clamped() {
        let deny = DenyMatcher::default();
        let at = |ms: i64| {
            let mut e = entry("a.yml", b"x: 1");
            e.observed_at = Timestamp::from_unix_millis(ms);
            validate(delta(vec![e]), &rules(&deny))
        };
        assert_eq!(at(-1).unwrap_err(), RegistryError::Invalid);
        assert_eq!(at(MAX_TS_MS + 1).unwrap_err(), RegistryError::Invalid);
        assert_eq!(at(0).unwrap().staged.entries[0].observed_at_ms, 0);
        // An agent clock a year ahead is clamped to the hub's clock plus the allowed skew.
        let far = at(NOW + 365 * 24 * 3600 * 1000).unwrap();
        assert_eq!(far.staged.entries[0].observed_at_ms, NOW + 5 * 60 * 1000);
        // A modification time outside the stored range is clamped, not refused.
        let mut e = entry("a.yml", b"x: 1");
        e.mtime = Timestamp::from_unix_millis(i64::MIN);
        assert_eq!(
            validate(delta(vec![e]), &rules(&deny)).unwrap().staged.entries[0].mtime_ms,
            0
        );
    }

    #[test]
    fn removals_skips_the_job_and_the_gap_are_kept() {
        let deny = DenyMatcher::default();
        let mut d = delta(vec![]);
        d.removed = vec![NfsPath::parse("gone.yml").unwrap()];
        d.skipped = vec![SkippedEntry {
            path: NfsPath::parse("huge.bin").unwrap(),
            reason: ShortText::parse("too large").unwrap(),
        }];
        d.during_job = Some(JobRef::new("sync", "uid-1").unwrap());
        d.gap = Some(SpoolGap {
            from: Timestamp::from_unix_millis(1),
            to: Timestamp::from_unix_millis(2),
            lost_entries: 5,
        });
        let v = validate(d, &rules(&deny)).unwrap();
        assert_eq!(v.staged.removed.len(), 1);
        assert_eq!(v.staged.skipped, vec![NfsPath::parse("huge.bin").unwrap()]);
        assert_eq!(v.staged.during_job.unwrap().uid(), "uid-1");
        assert_eq!(
            v.staged.gap,
            Some(StagedGap {
                from_ms: 1,
                to_ms: 2,
                lost: 5
            })
        );
    }

    #[test]
    fn a_staged_part_round_trips_through_json() {
        let deny = DenyMatcher::default();
        let v = validate(
            delta(vec![entry("a.yml", b"x: 1"), entry("b.png", b"\x89PNG")]),
            &rules(&deny),
        )
        .unwrap();
        let json = serde_json::to_value(&v.staged).unwrap();
        let back: StagedPart = serde_json::from_value(json).unwrap();
        assert_eq!(back, v.staged);
        assert_eq!(back.items(), 2);
    }
}
