//! The properties the registry's fuzz targets assert (S11, S17, S22, rule 5).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz_targets/*.rs`) and by
//! `crates/hub-registry/tests/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay (which `just verify` runs)
//! check exactly the same things. A check returns `Err(reason)` for a violation; the fuzz targets turn that into a panic, which
//! libFuzzer reports as a crash.
//!
//! Inputs that the hub rightly refuses are not violations: the checks look at what was accepted, and that what was refused
//! deserved it, decided by an oracle written from the rules rather than from the code under test.
//!
//! # The targets
//!
//! | Target | Input | What must hold |
//! |---|---|---|
//! | `registry_delta` | bytes read as a program: one to four messages of a scan delta, each with files (a path, whether the agent called it denied, its bytes or none, whether the hash and the size are true, a time that is sane, negative, beyond year 9999 or a year ahead), removals and skips | the checks never panic; a message is accepted exactly when an oracle written from the rules accepts it, and refused with the oracle's error otherwise (the first bad entry decides); an accepted message equals the entries given, in order, with the hashes, sizes and paths given; content is kept only for an entry that is neither marked denied nor matched by the hub's own globs, and then it hashes to the entry's hash; a denied entry is class `denied` and keeps no content; no time is below 0 or above the hub's clock plus the skew; the staged form survives JSON; the totals of a logical delta never pass a bound that was accepted, and a part that would pass it is refused; the plan for the versions of a path chains (each moving version replaces the one before it), never moves the state to the hash it already has, never moves it back to an older time, and orders a removal after what it removes |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use bytes::Bytes;
use domain::{ContentHash, NfsPath, ScanDelta, ScanEntry, ShortText, SkippedEntry, Timestamp};
use engine::DenyMatcher;
use hub_registry::RegistryError;
use hub_registry::fuzzing::{add_part, check_message, plan_versions};
use ports::content_hash;

/// A seed corpus needs at least this many files per target (`MIN_SEEDS` in the replay test).
pub const MIN_SEEDS: usize = 8;

/// A check of one input.
pub type Check = fn(&[u8]) -> Result<(), String>;

const NOW: i64 = 1_800_000_000_000;
const SKEW: i64 = 5 * 60 * 1000;
const MAX_FILE_BYTES: u64 = 32;
/// 9999-12-31T23:59:59.999Z.
const MAX_TS: i64 = 253_402_300_799_999;
const BOUNDS: (u64, u64, u64) = (4, 12, 64);
const ALPHABET: &[u8] = b"ab/./-_kepmrivt01\0";

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    /// The next byte; zeros after the end, so every input decodes.
    fn byte(&mut self) -> u8 {
        let b = self.data.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        b
    }

    fn path(&mut self) -> Option<NfsPath> {
        let len = 1 + usize::from(self.byte() % 24);
        let text: Vec<u8> = (0..len)
            .map(|_| ALPHABET[usize::from(self.byte()) % ALPHABET.len()])
            .collect();
        NfsPath::parse(std::str::from_utf8(&text).ok()?)
            .ok()
            .filter(|p| !p.is_root())
    }
}

struct Wanted {
    entry: ScanEntry,
    /// What the oracle needs: whether the agent sent bytes.
    has_bytes: bool,
    time: i64,
}

fn entry(r: &mut Reader<'_>) -> Option<Wanted> {
    let path = r.path();
    let flags = r.byte();
    let len = usize::from(r.byte() % 40);
    let content: Vec<u8> = (0..len).map(|_| r.byte()).collect();
    let jitter = i64::from(r.byte());
    let path = path?;
    let (denied, has_bytes, hash_ok, size_ok) =
        (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, flags & 8 != 0);
    let time = match (flags >> 4) & 3 {
        0 => NOW - 1_000 - jitter,
        1 => -1 - jitter,
        2 => MAX_TS + 1 + jitter,
        _ => NOW + 365 * 24 * 3600 * 1000 + jitter,
    };
    let hash = if hash_ok {
        content_hash(&content)
    } else {
        let mut other = content.clone();
        other.push(1);
        content_hash(&other)
    };
    let size = if size_ok { len as u64 } else { len as u64 + 1 };
    Some(Wanted {
        entry: ScanEntry {
            path,
            hash,
            size,
            mtime: Timestamp::from_unix_millis(NOW - 2_000),
            observed_at: Timestamp::from_unix_millis(time),
            denied,
            bytes: has_bytes.then(|| Bytes::from(content)),
        },
        has_bytes,
        time,
    })
}

/// What the rules say about a message, decided entry by entry in order: the first bad entry decides.
fn oracle(entries: &[Wanted]) -> Result<(), RegistryError> {
    for w in entries {
        let e = &w.entry;
        if !(0..=MAX_TS).contains(&w.time) {
            return Err(RegistryError::Invalid);
        }
        if e.denied && w.has_bytes {
            return Err(RegistryError::Invalid);
        }
        if let Some(bytes) = &e.bytes {
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(RegistryError::TooLarge);
            }
            if bytes.len() as u64 != e.size || content_hash(bytes) != e.hash {
                return Err(RegistryError::Invalid);
            }
        }
    }
    Ok(())
}

pub fn registry_delta(data: &[u8]) -> Result<(), String> {
    let mut r = Reader { data, at: 0 };
    let deny = DenyMatcher::default();
    let parts = 1 + usize::from(r.byte() % 4);
    let mut held = (0_u64, 0_u64, 0_u64);
    let mut all_versions: Vec<(ContentHash, i64)> = Vec::new();
    for p in 0..parts {
        let n_entries = usize::from(r.byte() % 7);
        let n_removed = usize::from(r.byte() % 3);
        let n_skipped = usize::from(r.byte() % 3);
        let wanted: Vec<Wanted> = (0..n_entries).filter_map(|_| entry(&mut r)).collect();
        let removed: Vec<NfsPath> = (0..n_removed).filter_map(|_| r.path()).collect();
        let skipped: Vec<SkippedEntry> = (0..n_skipped)
            .filter_map(|_| r.path())
            .map(|path| SkippedEntry {
                path,
                reason: ShortText::parse("too large").unwrap(),
            })
            .collect();
        let delta = ScanDelta {
            seq: p as u64 + 1,
            base_root: None,
            new_root: content_hash(b"root"),
            entries: wanted.iter().map(|w| w.entry.clone()).collect(),
            removed: removed.clone(),
            skipped: skipped.clone(),
            during_job: None,
            more: p + 1 < parts,
            part: p as u32,
            gap: None,
        };
        let verdict = check_message(delta, &deny, MAX_FILE_BYTES, NOW, SKEW);
        match (oracle(&wanted), verdict) {
            (Err(expected), Err(got)) => {
                if expected != got {
                    return Err(format!("refused with {got:?}, the rules say {expected:?}"));
                }
            }
            (Err(expected), Ok(_)) => {
                return Err(format!("accepted a message the rules refuse with {expected:?}"));
            }
            (Ok(()), Err(got)) => return Err(format!("refused a good message with {got:?}")),
            (Ok(()), Ok(checked)) => {
                if checked.entries.len() != wanted.len() {
                    return Err("an accepted message does not equal the entries given".to_owned());
                }
                if checked.removed != removed.len() || checked.skipped != skipped.len() {
                    return Err("removals or skips were lost".to_owned());
                }
                if !checked.round_trips {
                    return Err("the staged form does not survive JSON".to_owned());
                }
                let mut stored_content = checked.contents.iter();
                let mut bytes_kept = 0_u64;
                for (kept, w) in checked.entries.iter().zip(&wanted) {
                    let e = &w.entry;
                    if kept.path != e.path || kept.hash != e.hash || kept.size != e.size {
                        return Err("an entry was changed on the way in".to_owned());
                    }
                    let denied = e.denied || deny.is_denied(&e.path);
                    if kept.denied != denied {
                        return Err(format!(
                            "denied is {} for {}, the globs say {denied}",
                            kept.denied, e.path
                        ));
                    }
                    if denied && (kept.has_content || kept.class != domain::FileClass::Denied) {
                        return Err("a denied entry kept content or is not class denied".to_owned());
                    }
                    if !denied && kept.class == domain::FileClass::Denied {
                        return Err("a file nothing denies is class denied".to_owned());
                    }
                    let should_keep = !denied && w.has_bytes;
                    if kept.has_content != should_keep {
                        return Err(format!(
                            "content kept is {}, should be {should_keep}",
                            kept.has_content
                        ));
                    }
                    if kept.has_content {
                        let Some(bytes) = stored_content.next() else {
                            return Err("an entry says it has content and none is there".to_owned());
                        };
                        if content_hash(bytes) != kept.hash {
                            return Err("content is kept that does not hash to its entry".to_owned());
                        }
                        bytes_kept += bytes.len() as u64;
                    }
                    if !(0..=NOW + SKEW).contains(&kept.observed_at_ms) {
                        return Err(format!("time {} is out of range", kept.observed_at_ms));
                    }
                    all_versions.push((kept.hash, kept.observed_at_ms));
                }
                if stored_content.next().is_some() {
                    return Err("content is kept that no entry owns".to_owned());
                }
                if checked.bytes != bytes_kept {
                    return Err("the byte count is not the bytes kept".to_owned());
                }
                // The bounds on a whole logical delta.
                let items = (wanted.len() + removed.len() + skipped.len()) as u64;
                let would = (held.0 + 1, held.1 + items, held.2 + bytes_kept);
                match add_part(held, items, bytes_kept, BOUNDS) {
                    Ok(next) => {
                        if next != would || next.0 > BOUNDS.0 || next.1 > BOUNDS.1 || next.2 > BOUNDS.2 {
                            return Err(format!("totals {next:?} are not {would:?} or pass the bounds"));
                        }
                        held = next;
                    }
                    Err(RegistryError::TooLarge) => {
                        if would.0 <= BOUNDS.0 && would.1 <= BOUNDS.1 && would.2 <= BOUNDS.2 {
                            return Err(format!("refused {would:?}, which is within the bounds"));
                        }
                        return Ok(());
                    }
                    Err(other) => return Err(format!("bounds refused with {other:?}")),
                }
            }
        }
    }
    plan_invariants(&mut r, &all_versions)
}

/// The plan for the versions of one path against a stored state read from the input.
fn plan_invariants(r: &mut Reader<'_>, versions: &[(ContentHash, i64)]) -> Result<(), String> {
    let stored_kind = r.byte() % 3;
    let stored_hash = content_hash(&[r.byte() % 4]);
    let stored_at = i64::from(r.byte()) * 1_000_000_000;
    let stored = match stored_kind {
        0 => None,
        1 => Some((Some(stored_hash), stored_at)),
        _ => Some((None, stored_at)),
    };
    let removal = r.byte() % 2 == 1;
    let delta_time = if r.byte() % 2 == 1 {
        i64::from(r.byte()) * 1_000_000_000
    } else {
        0
    };
    let initial = r.byte() % 2 == 1;
    let steps = plan_versions(stored, versions, removal, delta_time, initial);

    let mut current = stored.and_then(|s| s.0);
    let mut current_at = stored.map_or(-1, |s| s.1);
    for step in &steps {
        if step.before != current {
            return Err(format!(
                "a version replaced {:?}, the state was {current:?}",
                step.before
            ));
        }
        if !step.moves {
            if step.at_ms >= current_at && stored.is_some() && step.hash.is_some() {
                // History only means older than the stored time.
                return Err("a version that is not older than the state was kept as history only".to_owned());
            }
            continue;
        }
        if step.hash.is_some() && step.hash == current {
            return Err("the state moved to the hash it already had".to_owned());
        }
        if step.at_ms < current_at {
            return Err("the state moved back to an older time".to_owned());
        }
        current = step.hash;
        current_at = step.at_ms;
    }
    // A removal comes after the files: if the plan holds one, it is the last step, and it removes a file that was there.
    if let Some(i) = steps.iter().position(|s| s.hash.is_none()) {
        if i + 1 != steps.len() {
            return Err("a removal is not the last step".to_owned());
        }
        if steps[i].before.is_none() {
            return Err("a removal of nothing was planned".to_owned());
        }
    }
    Ok(())
}

/// A one-line description of the verdict on the first message of a program, for the test that the seeds reach their cases.
pub fn describe(data: &[u8]) -> String {
    let mut r = Reader { data, at: 0 };
    let deny = DenyMatcher::default();
    let _parts = r.byte();
    let n_entries = usize::from(r.byte() % 7);
    let n_removed = usize::from(r.byte() % 3);
    let n_skipped = usize::from(r.byte() % 3);
    let wanted: Vec<Wanted> = (0..n_entries).filter_map(|_| entry(&mut r)).collect();
    let _ = (n_removed, n_skipped);
    let delta = ScanDelta {
        seq: 1,
        base_root: None,
        new_root: content_hash(b"root"),
        entries: wanted.iter().map(|w| w.entry.clone()).collect(),
        removed: Vec::new(),
        skipped: Vec::new(),
        during_job: None,
        more: false,
        part: 0,
        gap: None,
    };
    match check_message(delta, &deny, MAX_FILE_BYTES, NOW, SKEW) {
        Ok(c) => format!("accepted {} entries, {} bytes kept", c.entries.len(), c.bytes),
        Err(e) => format!("refused {e:?}"),
    }
}
