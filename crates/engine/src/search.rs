//! Text search over stored file versions (T7, S11, P7).
//!
//! The search is ripgrep's own, over blobs that are already in memory: `grep-regex` builds the matcher, `grep-searcher` runs it
//! over one slice and reports lines. It never touches NFS and never reads a file (the engine has no I/O); the caller gives it a
//! [`SearchCorpus`], the current stored versions, each blob once however many swimlanes hold it.
//!
//! # What is searched
//!
//! A place (swimlane and path) is searched when it passes the query's swimlane list and path glob and its class is
//! `structured` or `text` (a binary extension and a denied name are never opened, D79). A blob is searched when at least one of
//! its places is. It is read as text the way `decode_text` reads it (UTF-8, UTF-16 by its mark, Latin-1) and is skipped as
//! binary when that fails or the text holds a NUL, and skipped as oversized above `Limits::input_bytes`. The counts are in
//! [`SearchStats`].
//!
//! # How a query is read (S11)
//!
//! The pattern is literal text unless `regex` is set. A regex is the Rust `regex` engine's, which runs in time linear in the
//! text, and it is compiled under the size, DFA and nesting limits of [`Limits`]; a pattern over `regex_pattern_bytes`, an empty
//! one, one that does not compile, and one that holds a line break (a search reads one line at a time) are refused with an error
//! that does not repeat the pattern. The case option applies to both. `^` and `$` are the start and end of a line, and a line
//! may end in `\n`, `\r\n` or `\r`: the text is searched with all three turned into `\n`.
//!
//! # What comes back
//!
//! Hits are ordered by swimlane, path and line, so the answer does not depend on how many threads ran or in which order they
//! finished. A blob is searched once and its hits are reported at each of its places. Each place gets at most `max_per_file`
//! hits and the result at most `max_total`, both held to the limits of [`Limits`]; `truncated` says that a cap cut something
//! off. The line text has no `\r` or `\n` and is cut at `Limits::hit_line_bytes` on a character boundary; the ranges are byte
//! offsets into that text, in order, at most 64 of them, and a match that crosses the cut is cut with the line.
//!
//! # Memory
//!
//! While the blobs are searched on the pool, a hit is only a line number and two offsets. The text and the ranges of a hit
//! are made afterwards, for the hits that are returned, which is what keeps the memory of a frequent pattern over tens of
//! thousands of blobs small.
//!
//! # Cancelling
//!
//! The caller sets a [`CancelFlag`] to stop a search whose answer nobody wants any more. It is read before each blob and at
//! each match; a cancelled search returns an error, never a partial answer.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use domain::{ContentHash, FileClass, NfsPath, SwimlaneId, TextHit, TextQuery, TextResults};
use globset::{Candidate, GlobBuilder, GlobMatcher};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use rayon::prelude::*;

use crate::bytes::{DenyMatcher, classify, decode_text};
use crate::error::{DecodeError, SearchError};
use crate::limits::Limits;

/// The most ranges one hit lists. The line is cut at `Limits::hit_line_bytes` first, so this only matters for a limit set high.
const MAX_RANGES_PER_HIT: usize = 64;

/// A flag that stops a search. It is set from any thread and read by the searching threads.
#[derive(Debug, Default)]
pub struct CancelFlag(AtomicBool);

impl CancelFlag {
    /// A flag that is not set.
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// Set the flag. A search that sees it stops and returns [`SearchError::Cancelled`].
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// True once `cancel` has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// A place where a blob is stored: a swimlane and a path in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchLocation {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
}

/// One stored version and every place that holds it.
#[derive(Clone)]
pub struct SearchBlob {
    /// The hash of the content, the identity the caller deduplicates by. The search does not check it against the bytes.
    pub hash: ContentHash,
    pub content: Bytes,
    pub locations: Vec<SearchLocation>,
}

impl fmt::Debug for SearchBlob {
    /// The content is not printed (S21): a blob is a config file, and a log line must not hold one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchBlob")
            .field("hash", &self.hash)
            .field("bytes", &self.content.len())
            .field("locations", &self.locations.len())
            .finish()
    }
}

/// The blobs a search runs over, with their places in the order the results come in. Build it once and search it as often as
/// needed: the ordering work is done here, not per query.
#[derive(Clone)]
pub struct SearchCorpus {
    blobs: Vec<SearchBlob>,
    /// (blob, place) of every place, sorted by swimlane, then path.
    order: Vec<(u32, u32)>,
}

impl fmt::Debug for SearchCorpus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchCorpus")
            .field("blobs", &self.blobs.len())
            .field("locations", &self.order.len())
            .finish()
    }
}

impl SearchCorpus {
    /// A corpus of the given blobs.
    ///
    /// # Errors
    ///
    /// `CorpusTooLarge` for more than `u32::MAX` blobs, or places in one blob.
    pub fn new(blobs: Vec<SearchBlob>) -> Result<Self, SearchError> {
        let mut order = Vec::new();
        for (b, blob) in blobs.iter().enumerate() {
            let b = u32::try_from(b).map_err(|_| SearchError::CorpusTooLarge)?;
            for l in 0..blob.locations.len() {
                order.push((b, u32::try_from(l).map_err(|_| SearchError::CorpusTooLarge)?));
            }
        }
        order.sort_by(|x, y| {
            let place = |&(b, l): &(u32, u32)| &blobs[b as usize].locations[l as usize];
            let (px, py) = (place(x), place(y));
            px.swimlane
                .cmp(&py.swimlane)
                .then_with(|| px.path.cmp(&py.path))
                .then_with(|| x.cmp(y))
        });
        Ok(Self { blobs, order })
    }

    /// A corpus of files, grouped by content hash: the first file with a hash brings the bytes, the others only their
    /// places. The caller vouches that equal hashes mean equal bytes.
    ///
    /// # Errors
    ///
    /// `CorpusTooLarge`, as for `new`.
    pub fn from_files(
        files: impl IntoIterator<Item = (SwimlaneId, NfsPath, ContentHash, Bytes)>,
    ) -> Result<Self, SearchError> {
        let mut index: BTreeMap<ContentHash, usize> = BTreeMap::new();
        let mut blobs: Vec<SearchBlob> = Vec::new();
        for (swimlane, path, hash, content) in files {
            let location = SearchLocation { swimlane, path };
            if let Some(&i) = index.get(&hash) {
                blobs[i].locations.push(location);
            } else {
                index.insert(hash, blobs.len());
                blobs.push(SearchBlob {
                    hash,
                    content,
                    locations: vec![location],
                });
            }
        }
        Self::new(blobs)
    }

    /// The number of distinct blobs.
    pub fn blob_count(&self) -> usize {
        self.blobs.len()
    }

    /// The number of places.
    pub fn location_count(&self) -> usize {
        self.order.len()
    }
}

/// What a search did besides finding hits. The counts are of blobs that had a place to search, and they do not depend on the
/// pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchStats {
    /// Blobs that were searched.
    pub blobs_searched: usize,
    /// Blobs skipped as binary: a NUL in the text, or bytes that are not the text their byte order mark declares.
    pub blobs_binary: usize,
    /// Blobs skipped for being over `Limits::input_bytes`.
    pub blobs_oversized: usize,
}

/// The answer of `search_text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    pub results: TextResults,
    pub stats: SearchStats,
}

/// A hit before its text is made: where its line is in the decoded text of its blob.
#[derive(Debug, Clone, Copy)]
struct RawHit {
    line: u32,
    start: usize,
    end: usize,
}

enum Outcome {
    /// No place of the blob was eligible (or the search was cancelled before it).
    Unsearched,
    Binary,
    Oversized,
    Searched {
        hits: Vec<RawHit>,
        overflow: bool,
    },
}

struct BlobResult {
    /// The places of the blob that passed the filters, ascending.
    eligible: Vec<u32>,
    outcome: Outcome,
}

/// A path glob. One without a slash also matches the file name alone, as the deny globs do.
struct PathFilter {
    matcher: GlobMatcher,
    name_too: bool,
}

impl PathFilter {
    fn new(glob: &str) -> Result<Self, SearchError> {
        let matcher = GlobBuilder::new(glob)
            .build()
            .map_err(|_| SearchError::InvalidGlob)?
            .compile_matcher();
        Ok(Self {
            matcher,
            name_too: !glob.contains('/'),
        })
    }

    fn matches(&self, path: &NfsPath) -> bool {
        if self.matcher.is_match_candidate(&Candidate::new(path.as_str())) {
            return true;
        }
        let name = path.file_name();
        self.name_too
            && name.len() != path.as_str().len()
            && self.matcher.is_match_candidate(&Candidate::new(name))
    }
}

/// A query, checked and compiled.
struct Plan {
    matcher: RegexMatcher,
    /// Ascending, without repeats. Empty means every swimlane of the corpus.
    swimlanes: Vec<SwimlaneId>,
    glob: Option<PathFilter>,
    total: usize,
    per_file: usize,
}

impl Plan {
    fn new(query: &TextQuery, limits: &Limits) -> Result<Self, SearchError> {
        if query.pattern.is_empty() {
            return Err(SearchError::EmptyPattern);
        }
        if query.pattern.len() > limits.regex_pattern_bytes {
            return Err(SearchError::PatternTooLong);
        }
        // Literal unless asked for. The text is searched with every line ending turned into `\n` (see `lines_as_lf`), so the
        // line terminator is `\n` and a pattern that could match a line break is refused here instead of silently never
        // matching. The matcher wraps the pattern in one group of its own, which the nesting limit would count.
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!query.case_sensitive)
            .fixed_strings(!query.regex)
            .line_terminator(Some(b'\n'))
            .size_limit(limits.regex_size_limit)
            .dfa_size_limit(limits.regex_dfa_size_limit)
            .nest_limit(limits.regex_nest_limit.saturating_add(1))
            .build(&query.pattern)
            .map_err(|_| SearchError::InvalidPattern)?;
        let glob = match query.path_glob.as_ref().map(domain::ShortText::as_str) {
            None | Some("") => None,
            Some(text) => Some(PathFilter::new(text)?),
        };
        let mut swimlanes = query.swimlanes.clone();
        swimlanes.sort();
        swimlanes.dedup();
        let cap = |asked: u32, limit: usize| usize::try_from(asked).map_or(limit, |a| a.min(limit));
        Ok(Self {
            matcher,
            swimlanes,
            glob,
            total: cap(query.max_total, limits.hits_total),
            per_file: cap(query.max_per_file, limits.hits_per_file),
        })
    }

    fn wants(&self, place: &SearchLocation) -> bool {
        (self.swimlanes.is_empty() || self.swimlanes.binary_search(&place.swimlane).is_ok())
            && self.glob.as_ref().is_none_or(|g| g.matches(&place.path))
    }
}

/// Collects the lines that match, as offsets, and stops at one more than the per-file cap.
struct Collector<'a> {
    cancel: &'a CancelFlag,
    per_file: usize,
    hits: Vec<RawHit>,
    overflow: bool,
}

impl Sink for Collector<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, found: &SinkMatch<'_>) -> Result<bool, io::Error> {
        if self.cancel.is_cancelled() {
            return Ok(false);
        }
        if self.hits.len() >= self.per_file {
            self.overflow = true;
            return Ok(false);
        }
        let line = found.bytes().strip_suffix(b"\n").unwrap_or(found.bytes());
        let start = usize::try_from(found.absolute_byte_offset()).unwrap_or(usize::MAX);
        self.hits.push(RawHit {
            line: found
                .line_number()
                .map_or(u32::MAX, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            start,
            end: start.saturating_add(line.len()),
        });
        Ok(true)
    }
}

/// The text with every line ending (`\r\n`, and a lone `\r`) turned into `\n`, borrowed when it has no `\r`. The matcher's `$`
/// stops before `\n` only, so a file with `\r\n` endings is searched as the text its lines are: and the line text of a hit has
/// no `\r`. Lines are counted the way the parsers count them.
fn lines_as_lf(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.contains('\r') {
        return text;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            out.push(b'\n');
            i += if bytes.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // `\r` and `\n` are ASCII, so replacing or removing them leaves valid UTF-8.
    Cow::Owned(String::from_utf8(out).unwrap_or_default())
}

/// A blob read as text for searching: decoded, and with its line endings turned into `\n`.
fn blob_text<'a>(content: &'a [u8], limits: &Limits) -> Result<Cow<'a, str>, DecodeError> {
    decode_text(content, limits).map(|decoded| lines_as_lf(decoded.text))
}

fn searcher() -> Searcher {
    SearcherBuilder::new()
        .line_number(true)
        // The blob is decoded and checked for NUL before it is searched; the searcher sees text.
        .binary_detection(BinaryDetection::none())
        .bom_sniffing(false)
        .build()
}

fn search_blob(
    blob: &SearchBlob,
    plan: &Plan,
    deny: &DenyMatcher,
    limits: &Limits,
    cancel: &CancelFlag,
    searcher: &mut Searcher,
) -> BlobResult {
    if cancel.is_cancelled() {
        return with(Vec::new(), Outcome::Unsearched);
    }
    let mut eligible = Vec::new();
    for (i, place) in blob.locations.iter().enumerate() {
        let Ok(i) = u32::try_from(i) else { break };
        if plan.wants(place)
            && matches!(
                classify(&place.path, Some(&blob.content), deny),
                FileClass::Structured | FileClass::Text
            )
        {
            eligible.push(i);
        }
    }
    if eligible.is_empty() {
        return with(eligible, Outcome::Unsearched);
    }
    let text = match blob_text(&blob.content, limits) {
        Ok(text) => text,
        Err(DecodeError::TooLarge) => return with(eligible, Outcome::Oversized),
        Err(DecodeError::InvalidUtf8 | DecodeError::InvalidUtf16) => {
            return with(eligible, Outcome::Binary);
        }
    };
    let bytes = text.as_bytes();
    if bytes.contains(&0) {
        return with(eligible, Outcome::Binary);
    }
    let mut sink = Collector {
        cancel,
        per_file: plan.per_file,
        hits: Vec::new(),
        overflow: false,
    };
    // The only error a slice search can give is the sink's, and the sink gives none; a stopped search is a short one.
    let _ = searcher.search_slice(&plan.matcher, bytes, &mut sink);
    with(
        eligible,
        Outcome::Searched {
            hits: sink.hits,
            overflow: sink.overflow,
        },
    )
}

fn with(eligible: Vec<u32>, outcome: Outcome) -> BlobResult {
    BlobResult { eligible, outcome }
}

/// The hit for one line: its text, cut, and the ranges of the matches in it.
fn materialise(
    text: &str,
    raw: RawHit,
    plan: &Plan,
    limits: &Limits,
    place: &SearchLocation,
) -> Option<TextHit> {
    let line = text.get(raw.start..raw.end)?;
    let mut cut = line.len().min(limits.hit_line_bytes);
    while !line.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    // A matcher that cannot fail (`NoError`) gives nothing to handle here.
    let _ = plan.matcher.find_iter(line.as_bytes(), |m| {
        if m.start() >= cut || ranges.len() == MAX_RANGES_PER_HIT {
            return false;
        }
        if m.end() > m.start() {
            let to = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
            ranges.push((to(m.start()), to(m.end().min(cut))));
        }
        true
    });
    Some(TextHit {
        swimlane: place.swimlane.clone(),
        path: place.path.clone(),
        line: raw.line,
        text: line[..cut].to_owned(),
        ranges,
        logical_file: None,
        tenant: None,
    })
}

/// Search the corpus for a query.
///
/// The work runs on `pool` (never the global pool), so the caller calls this from a blocking context. `deny` decides which
/// names are never opened, and `limits` bounds the pattern, the compiled regex and the result. The hits come in swimlane, path
/// and line order whatever the pool does.
///
/// # Errors
///
/// `EmptyPattern`, `PatternTooLong`, `InvalidPattern` and `InvalidGlob` for a query that cannot be run, `Cancelled` when the
/// flag is set before the search ends. No error contains the pattern or the glob.
pub fn search_text(
    corpus: &SearchCorpus,
    query: &TextQuery,
    deny: &DenyMatcher,
    limits: &Limits,
    pool: &rayon::ThreadPool,
    cancel: &CancelFlag,
) -> Result<SearchOutcome, SearchError> {
    let plan = Plan::new(query, limits)?;
    if cancel.is_cancelled() {
        return Err(SearchError::Cancelled);
    }
    let found: Vec<BlobResult> = pool.install(|| {
        corpus
            .blobs
            .par_iter()
            .map_init(searcher, |searcher, blob| {
                search_blob(blob, &plan, deny, limits, cancel, searcher)
            })
            .collect()
    });
    if cancel.is_cancelled() {
        return Err(SearchError::Cancelled);
    }

    let mut stats = SearchStats::default();
    for result in &found {
        match result.outcome {
            Outcome::Searched { .. } => stats.blobs_searched += 1,
            Outcome::Binary => stats.blobs_binary += 1,
            Outcome::Oversized => stats.blobs_oversized += 1,
            Outcome::Unsearched => {}
        }
    }

    let mut hits: Vec<TextHit> = Vec::new();
    let mut truncated = false;
    // The decoded text of the blobs that got a hit: at most one per returned hit, and the hits are capped.
    let mut texts: BTreeMap<u32, Cow<'_, str>> = BTreeMap::new();
    'places: for &(b, l) in &corpus.order {
        let result = &found[b as usize];
        let Outcome::Searched { hits: raw, overflow } = &result.outcome else {
            continue;
        };
        if (raw.is_empty() && !overflow) || result.eligible.binary_search(&l).is_err() {
            continue;
        }
        if hits.len() >= plan.total {
            truncated = true;
            break;
        }
        let blob = &corpus.blobs[b as usize];
        let place = &blob.locations[l as usize];
        let text = match texts.entry(b) {
            Entry::Occupied(known) => known.into_mut(),
            Entry::Vacant(slot) => match blob_text(&blob.content, limits) {
                Ok(text) => slot.insert(text),
                Err(_) => continue,
            },
        };
        for &hit in raw {
            if hits.len() >= plan.total {
                truncated = true;
                break 'places;
            }
            if let Some(hit) = materialise(text, hit, &plan, limits, place) {
                hits.push(hit);
            }
        }
        if *overflow {
            truncated = true;
        }
    }
    Ok(SearchOutcome {
        results: TextResults { hits, truncated },
        stats,
    })
}
