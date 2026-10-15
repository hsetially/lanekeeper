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
use domain::{ContentHash, Cursor, NfsPath, Page, ScanDelta, ScanEntry, ShortText, SkippedEntry, Timestamp};
use engine::DenyMatcher;
use hub_registry::RegistryError;
use hub_registry::fuzzing::{add_part, check_message, plan_versions};
use hub_registry::http::query;
use hub_registry::read::cursor::{self as rc, Context, CursorError, Field, Kind, Parsed};
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

// ------------------------------------------------------------------------------------------------ registry_cursor

/// The bounds of a key, from `docs` and the plan (A31), written here and not read from the code under test.
const KEY_FIELDS: usize = 6;
const KEY_BYTES: usize = 255;
/// The raw bytes a 512-character cursor can hold (`512 * 3 / 4`).
const KEY_RAW: usize = 384;

const KINDS: [Kind; 10] = [
    Kind::Swimlanes,
    Kind::Tree,
    Kind::Drift,
    Kind::History,
    Kind::Findings,
    Kind::SettingsSearch,
    Kind::Grid,
    Kind::CompareTree,
    Kind::Restarts,
    Kind::DriftByPath,
];

/// The text of a cursor, the way the REST edge hands it over: at most 512 characters of the cursor alphabet.
pub fn registry_cursor(data: &[u8]) -> Result<(), String> {
    // 1. The bytes as the text of a cursor (a lossy reading, so that every input is some text).
    let text = String::from_utf8_lossy(&data[..data.len().min(700)]).into_owned();
    parsed_text_is_canonical(&text)?;
    // 2. The bytes as a program that builds a cursor.
    cursor_program(data).map(|_| ())
}

fn parsed_text_is_canonical(text: &str) -> Result<(), String> {
    let Ok(parsed) = rc::parse(text) else {
        return Ok(());
    };
    if Cursor::parse(text).is_err() || text.len() > 512 {
        return Err("a text that parses is not a cursor of at most 512 characters".to_owned());
    }
    if parsed.fields.len() > KEY_FIELDS {
        return Err("a cursor with too many fields parsed".to_owned());
    }
    if parsed
        .fields
        .iter()
        .any(|f| matches!(f, Field::Bytes(b) if b.len() > KEY_BYTES))
    {
        return Err("a cursor with a field over the bound parsed".to_owned());
    }
    match rc::assemble(&parsed) {
        Ok(again) if again.as_str() == text => Ok(()),
        Ok(_) => Err("a cursor has a second spelling".to_owned()),
        Err(e) => Err(format!("a cursor that parsed does not assemble: {e}")),
    }
}

/// `Ok("refused")` when the key the program describes does not fit a cursor, `Ok("made")` when it was made and held every invariant.
fn cursor_program(data: &[u8]) -> Result<&'static str, String> {
    let mut r = Reader { data, at: 0 };
    let kind = KINDS[usize::from(r.byte()) % KINDS.len()];
    let parts: Vec<String> = (0..usize::from(r.byte() % 4))
        .map(|_| {
            let len = usize::from(r.byte() % 9);
            (0..len)
                .map(|_| char::from(ALPHABET[usize::from(r.byte()) % ALPHABET.len()]))
                .collect()
        })
        .collect();
    let part_refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    let context = Context::of(kind, &part_refs);
    let count = usize::from(r.byte() % 9);
    let mut fields = Vec::new();
    for _ in 0..count {
        if r.byte() % 2 == 0 {
            let mut n = [0_u8; 8];
            n.iter_mut().for_each(|b| *b = r.byte());
            fields.push(Field::Int(i64::from_be_bytes(n)));
        } else {
            let len = match r.byte() % 4 {
                0 => usize::from(r.byte() % 4),
                1 => usize::from(r.byte() % 40),
                2 => 253 + usize::from(r.byte() % 5),
                _ => usize::from(r.byte()) + 100 * usize::from(r.byte() % 2),
            };
            fields.push(Field::Bytes((0..len).map(|_| r.byte()).collect()));
        }
    }
    let parsed = Parsed {
        kind,
        context: context.tag(),
        fields: fields.clone(),
    };

    // The oracle: a key fits when it has at most six fields, none of them a byte string over 255 bytes, and its raw form (version, kind,
    // tag and count, then 9 bytes for a number and 3 + length for a byte string) is at most 384 bytes.
    let raw_len: usize = 1
        + 1
        + 8
        + 1
        + fields
            .iter()
            .map(|f| match f {
                Field::Int(_) => 9,
                Field::Bytes(b) => 3 + b.len(),
            })
            .sum::<usize>();
    let fits = fields.len() <= KEY_FIELDS
        && fields
            .iter()
            .all(|f| !matches!(f, Field::Bytes(b) if b.len() > KEY_BYTES))
        && raw_len <= KEY_RAW;
    let cursor = match rc::assemble(&parsed) {
        Ok(c) if fits => c,
        Ok(_) => {
            return Err(format!(
                "a key that does not fit was assembled ({} fields, {raw_len} raw bytes)",
                fields.len()
            ));
        }
        Err(CursorError::TooLarge) if !fits => return Ok("refused"),
        Err(e) => {
            return Err(format!(
                "a key that fits was refused or refused wrongly: {e} (fits: {fits})"
            ));
        }
    };
    if cursor.as_str().len() > 512 {
        return Err("a cursor of more than 512 characters was made".to_owned());
    }
    if rc::parse(cursor.as_str()) != Ok(parsed.clone()) {
        return Err("a cursor does not parse back to its parts".to_owned());
    }
    if rc::decode(&cursor, kind, &context) != Ok(fields.clone()) {
        return Err("a cursor does not decode under its own kind and query".to_owned());
    }
    let other_kind = KINDS[(KINDS.iter().position(|k| *k == kind).unwrap_or(0) + 1) % KINDS.len()];
    if rc::decode(&cursor, other_kind, &context) != Err(CursorError::WrongKind) {
        return Err("a cursor was accepted for another list".to_owned());
    }
    let other = Context::of(kind, &["a query that is not the one it was issued for"]);
    if other.tag() != context.tag() && rc::decode(&cursor, kind, &other) != Err(CursorError::WrongContext) {
        return Err("a cursor was accepted for another query".to_owned());
    }
    let page = Page::new(1, Some(cursor.clone())).map_err(|e| e.to_string())?;
    if rc::resume(&page, kind, &context) != Ok(Some(fields.clone())) {
        return Err("resume did not give the key back".to_owned());
    }
    if other.tag() != context.tag() && rc::resume(&page, kind, &other).is_ok() {
        return Err("resume accepted a cursor of another query".to_owned());
    }
    // No proper prefix of a cursor is a cursor, and a changed character is refused or is another cursor in its one canonical spelling.
    let text = cursor.as_str();
    for cut in [1, text.len() / 3, text.len() / 2, text.len() - 1] {
        if cut > 0 && cut < text.len() && rc::parse(&text[..cut]).is_ok() {
            return Err("a prefix of a cursor parsed".to_owned());
        }
    }
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_=.";
    for _ in 0..8 {
        let at = usize::from(r.byte()) % text.len();
        let with = alphabet[usize::from(r.byte()) % alphabet.len()];
        let mut changed = text.as_bytes().to_vec();
        changed[at] = with;
        let changed = String::from_utf8(changed).map_err(|e| e.to_string())?;
        parsed_text_is_canonical(&changed)?;
    }
    Ok("made")
}

// ------------------------------------------------------------------------------------------------ registry_query

/// Each parser of `hub_registry::http::query`, with the parameter names its operation has.
const OPERATIONS: [(&str, &[&str]); 14] = [
    ("page_only", &["cursor", "limit"]),
    ("none", &[]),
    ("tree", &["prefix", "drift", "cursor", "limit"]),
    ("drift", &["state", "prefix", "cursor", "limit"]),
    ("content", &["path", "source", "hash", "repo", "ref"]),
    ("history", &["path", "cursor", "limit"]),
    ("compare", &["left", "right", "file", "channel"]),
    ("compare_tree", &["left", "right", "prefix", "cursor", "limit"]),
    (
        "grid",
        &[
            "file",
            "swimlanes",
            "channel",
            "only_differing",
            "cursor",
            "limit",
        ],
    ),
    ("search_settings", &["q", "swimlanes", "file", "cursor", "limit"]),
    (
        "search_text",
        &[
            "pattern",
            "regex",
            "case_sensitive",
            "swimlanes",
            "path_glob",
            "max_total",
            "max_per_file",
        ],
    ),
    (
        "findings",
        &["swimlane", "kinds", "min_severity", "cursor", "limit"],
    ),
    ("effective", &["file", "tenant", "channel"]),
    ("served", &["application", "tenant", "channel", "file"]),
];

/// Values that are awkward in one way or another, for any parameter.
const POOL: [&str; 40] = [
    "",
    "1",
    "0",
    "5",
    "500",
    "501",
    "-1",
    "+1",
    "1.0",
    "99999999999",
    "true",
    "false",
    "yes",
    "TRUE",
    "sit1",
    "sit1,sit2",
    "sit1,sit1",
    "sit1,,sit2",
    "a/b",
    "a/b/c.yml",
    "../x",
    "%2e%2e",
    "%2e%2e%2fetc",
    "%00",
    "%FF%FE",
    "core.yml",
    "nfs:sit1",
    "git:base@main",
    "git:tenant@v1.2",
    "ghp_abcdef",
    "A",
    "a b",
    "git_ahead",
    "high",
    "drift,orphan_file",
    "nfs",
    "blob",
    "git",
    "base",
    "x*",
];

const QUERY_ALPHABET: &[u8] = b"ab/./-_01,%2F=&+: ";

/// The operation and the raw query a program stands for.
fn query_program(data: &[u8]) -> (usize, String) {
    let mut r = Reader { data, at: 0 };
    let first = r.byte();
    let op = usize::from(first & 0x0f) % OPERATIONS.len();
    if first & 0x80 != 0 {
        let raw = String::from_utf8_lossy(&data[1.min(data.len())..data.len().min(9_001)]).into_owned();
        return (op, raw);
    }
    let names = OPERATIONS[op].1;
    let pairs = usize::from(r.byte() % 9);
    let mut parts: Vec<String> = Vec::new();
    for _ in 0..pairs {
        let pick = usize::from(r.byte()) % (names.len() + 4);
        let name = match names.get(pick) {
            Some(name) => *name,
            None => ["unknown", "", "limit", "x%20y"][pick - names.len()],
        };
        let value = match usize::from(r.byte()) % (POOL.len() + 3) {
            n if n < POOL.len() => POOL[n].to_owned(),
            n if n == POOL.len() => {
                let len = usize::from(r.byte() % 32);
                (0..len)
                    .map(|_| char::from(QUERY_ALPHABET[usize::from(r.byte()) % QUERY_ALPHABET.len()]))
                    .collect()
            }
            n if n == POOL.len() + 1 => "a".repeat(usize::from(r.byte()) * 4),
            _ => format!("sit{}", r.byte() % 70),
        };
        parts.push(format!("{name}={value}"));
    }
    (op, parts.join("&"))
}

fn enc(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn page_ok(p: &Page) -> Result<(), String> {
    if !(1..=500).contains(&p.limit()) {
        return Err(format!("a limit of {} was accepted", p.limit()));
    }
    if let Some(c) = p.cursor() {
        if c.as_str().is_empty() || c.as_str().len() > 512 {
            return Err("a cursor outside 1 to 512 characters was accepted".to_owned());
        }
    }
    Ok(())
}

fn page_text(p: &Page) -> String {
    match p.cursor() {
        Some(c) => format!("limit={}&cursor={}", p.limit(), c.as_str()),
        None => format!("limit={}", p.limit()),
    }
}

/// A path that is relative, has no empty or `..` component, no control character and no backslash, and is at most 1,024 bytes.
fn path_ok(p: &NfsPath, what: &str) -> Result<(), String> {
    let s = p.as_str();
    let bad = s.len() > 1_024
        || s.starts_with('/')
        || s.contains('\\')
        || s.chars().any(char::is_control)
        || (!s.is_empty() && s.split('/').any(|c| c.is_empty() || c == ".." || c == "."));
    if bad {
        Err(format!(
            "{what}: a path that is not a clean relative path was accepted: {s:?}"
        ))
    } else {
        Ok(())
    }
}

fn list_ok<T: Ord>(items: &[T], max: usize, what: &str) -> Result<(), String> {
    let unique: std::collections::BTreeSet<&T> = items.iter().collect();
    if items.len() > max || unique.len() != items.len() {
        Err(format!(
            "{what}: a list over its bound or with a repeat was accepted"
        ))
    } else {
        Ok(())
    }
}

/// Parse `raw` as operation `op`: `Ok(Some(debug))` when it is accepted and holds every invariant, `Ok(None)` when it is refused.
fn run_operation(op: usize, raw: Option<&str>) -> Result<Option<String>, String> {
    use domain::ContentSource;
    macro_rules! parsed {
        ($call:expr, |$v:ident| $check:block) => {
            match $call {
                Ok($v) => {
                    $check
                    Ok(Some(format!("{:?}", $v)))
                }
                Err(_) => Ok(None),
            }
        };
    }
    match op {
        0 => parsed!(query::page_only(raw), |p| {
            page_ok(&p)?;
            let again = query::page_only(Some(&page_text(&p)))
                .map_err(|_| "a page did not parse again".to_owned())?;
            if again != p {
                return Err("a page changed when it was written and read again".to_owned());
            }
        }),
        1 => match query::none(raw) {
            // No parameters: only a query of empty pairs (`&&`) is the same as none.
            Ok(()) if plain_names(raw.unwrap_or("")).is_some_and(|names| !names.is_empty()) => {
                Err("a parameter was accepted where there are none".to_owned())
            }
            Ok(()) => Ok(Some(String::new())),
            Err(_) => Ok(None),
        },
        2 => parsed!(query::tree(raw), |t| {
            page_ok(&t.page)?;
            path_ok(&t.prefix, "tree prefix")?;
            let mut again = format!("prefix={}&{}", enc(t.prefix.as_str()), page_text(&t.page));
            if let Some(d) = t.drift {
                again.push_str(&format!("&drift={}", d.as_str()));
            }
            let again =
                query::tree(Some(&again)).map_err(|_| "a tree request did not parse again".to_owned());
            if t.prefix.is_root() {
                // The root is written as no prefix at all.
            } else if again? != t {
                return Err("a tree request changed when it was written and read again".to_owned());
            }
        }),
        3 => parsed!(query::drift(raw), |d| {
            page_ok(&d.page)?;
            path_ok(&d.prefix, "drift prefix")?;
        }),
        4 => parsed!(query::content(raw), |c| {
            path_ok(&c.path, "content path")?;
            // A source has exactly its own parameters: the query is written back with them, and is refused with any other.
            let s = raw.unwrap_or("");
            let has = |name: &str| s.split('&').any(|p| p.split('=').next() == Some(name));
            // (A name written with an escape is the decoder's business; the oracle reads plain names only.)
            let plain = !s.contains(['%', '+']);
            match &c.source {
                _ if !plain => {}
                ContentSource::Nfs | ContentSource::Baseline => {
                    if has("hash") || has("repo") || has("ref") {
                        return Err("a parameter of another source was accepted".to_owned());
                    }
                }
                ContentSource::Blob { .. } => {
                    if !has("hash") || has("repo") || has("ref") {
                        return Err("a blob source with the wrong parameters was accepted".to_owned());
                    }
                }
                ContentSource::Git { .. } => {
                    if has("hash") || !has("repo") || !has("ref") {
                        return Err("a git source with the wrong parameters was accepted".to_owned());
                    }
                }
            }
        }),
        5 => parsed!(query::history(raw), |h| {
            page_ok(&h.page)?;
            path_ok(&h.path, "history path")?;
        }),
        6 => parsed!(query::compare(raw), |c| {
            path_ok(c.file.as_path(), "compare file")?;
            let again = format!(
                "left={}&right={}&file={}",
                enc(&c.left.to_string()),
                enc(&c.right.to_string()),
                enc(c.file.as_str())
            );
            let again = match &c.channel {
                Some(ch) => format!("{again}&channel={}", enc(ch.as_str())),
                None => again,
            };
            if query::compare(Some(&again)).map_err(|_| "a compare did not parse again".to_owned())? != c {
                return Err("a compare changed when it was written and read again".to_owned());
            }
        }),
        7 => parsed!(query::compare_tree(raw), |c| {
            page_ok(&c.page)?;
            path_ok(&c.prefix, "compare prefix")?;
        }),
        8 => parsed!(query::grid(raw), |g| {
            page_ok(&g.page)?;
            path_ok(g.file.as_path(), "grid file")?;
            if g.swimlanes.is_empty() {
                return Err("a grid of no swimlanes was accepted".to_owned());
            }
            list_ok(&g.swimlanes, 64, "grid swimlanes")?;
            let lanes: Vec<&str> = g.swimlanes.iter().map(|s| s.as_str()).collect();
            let again = format!(
                "file={}&swimlanes={}&only_differing={}&{}",
                enc(g.file.as_str()),
                lanes.join(","),
                g.only_differing,
                page_text(&g.page)
            );
            let again = match &g.channel {
                Some(ch) => format!("{again}&channel={}", enc(ch.as_str())),
                None => again,
            };
            if query::grid(Some(&again)).map_err(|_| "a grid did not parse again".to_owned())? != g {
                return Err("a grid changed when it was written and read again".to_owned());
            }
        }),
        9 => parsed!(query::search_settings(raw), |q| {
            page_ok(&q.page)?;
            let needle = q.path_contains.as_str();
            if needle.is_empty() || needle.len() > 256 || needle.chars().any(char::is_control) {
                return Err("a settings needle outside its bounds was accepted".to_owned());
            }
            list_ok(&q.swimlanes, 64, "settings swimlanes")?;
            if let Some(f) = &q.file {
                path_ok(f.as_path(), "settings file")?;
            }
        }),
        10 => parsed!(query::search_text(raw), |t| {
            if t.pattern.is_empty()
                || t.pattern.len() > 512
                || t.pattern.chars().any(|c| c.is_control() && c != '\t')
            {
                return Err("a text pattern outside its bounds was accepted".to_owned());
            }
            if !(1..=500).contains(&t.max_total) || !(1..=100).contains(&t.max_per_file) {
                return Err("a cap outside its range was accepted".to_owned());
            }
            list_ok(&t.swimlanes, 64, "text swimlanes")?;
            if t.path_glob.as_ref().is_some_and(|g| g.as_str().is_empty()) {
                return Err("an empty glob was accepted".to_owned());
            }
            let lanes: Vec<&str> = t.swimlanes.iter().map(|s| s.as_str()).collect();
            let mut again = format!(
                "pattern={}&regex={}&case_sensitive={}&max_total={}&max_per_file={}",
                enc(&t.pattern),
                t.regex,
                t.case_sensitive,
                t.max_total,
                t.max_per_file
            );
            if !lanes.is_empty() {
                again.push_str(&format!("&swimlanes={}", lanes.join(",")));
            }
            if let Some(g) = &t.path_glob {
                again.push_str(&format!("&path_glob={}", enc(g.as_str())));
            }
            if query::search_text(Some(&again)).map_err(|_| "a text search did not parse again".to_owned())?
                != t
            {
                return Err("a text search changed when it was written and read again".to_owned());
            }
        }),
        11 => parsed!(query::findings(raw), |f| {
            page_ok(&f.page)?;
            list_ok(&f.kinds, 12, "finding kinds")?;
        }),
        12 => parsed!(query::effective(raw), |e| {
            path_ok(e.file.as_path(), "effective file")?;
        }),
        _ => parsed!(query::served(raw), |s| {
            path_ok(&s.file, "served file")?;
        }),
    }
}

/// The names of the pairs of a raw query that the oracle can read without decoding: `None` when the text has an escape, a plus, a
/// fragment mark or a space (decoding them is the code's business, and the oracle does not repeat it).
fn plain_names(raw: &str) -> Option<Vec<String>> {
    if raw.is_empty() {
        return Some(Vec::new());
    }
    if raw.contains(['%', '+', '#', ' ']) || raw.chars().any(char::is_control) {
        return None;
    }
    // An empty pair (`a=1&&b=2`) is skipped by the decoder, so it has no name.
    Some(
        raw.split('&')
            .filter(|p| !p.is_empty())
            .map(|p| p.split('=').next().unwrap_or_default().to_owned())
            .collect(),
    )
}

pub fn registry_query(data: &[u8]) -> Result<(), String> {
    let (op, raw) = query_program(data);
    let (name, allowed) = OPERATIONS[op];
    let query = (!raw.is_empty()).then_some(raw.as_str());
    let first = run_operation(op, query).map_err(|why| format!("{name}: {why} ({raw:?})"))?;
    // The same answer twice.
    let second = run_operation(op, query).map_err(|why| format!("{name}: {why}"))?;
    if first != second {
        return Err(format!("{name}: two readings of {raw:?} differ"));
    }
    // A query over 8 KiB is refused whatever it holds.
    if raw.len() > 8 * 1024 && first.is_some() {
        return Err(format!("{name}: a query of {} bytes was accepted", raw.len()));
    }
    // A name the operation does not have, and a name twice, are refused: the oracle splits the text itself.
    if let Some(names) = plain_names(&raw) {
        let unknown = names.iter().any(|n| !allowed.contains(&n.as_str()));
        let mut seen = std::collections::BTreeSet::new();
        let repeated = names.iter().any(|n| !seen.insert(n.as_str()));
        if (unknown || repeated) && first.is_some() {
            return Err(format!(
                "{name}: {raw:?} has a name that is not the operation's or is given twice, and was accepted"
            ));
        }
    }
    Ok(())
}

/// One line saying what a program is and what became of it, for the test that the seeds reach the cases they are named for.
pub fn describe_query(data: &[u8]) -> String {
    let (op, raw) = query_program(data);
    let query = (!raw.is_empty()).then_some(raw.as_str());
    let verdict = match run_operation(op, query) {
        Ok(Some(_)) => "accepted",
        Ok(None) => "refused",
        Err(_) => "VIOLATION",
    };
    format!("{} {verdict}", OPERATIONS[op].0)
}

/// What a cursor program makes: `refused` when the key does not fit, else the length of the cursor text.
pub fn describe_cursor(data: &[u8]) -> String {
    let text = String::from_utf8_lossy(&data[..data.len().min(700)]).into_owned();
    if rc::parse(&text).is_ok() {
        return "text parses".to_owned();
    }
    match cursor_program(data) {
        Ok(what) => format!("program {what}"),
        Err(why) => format!("VIOLATION {why}"),
    }
}
