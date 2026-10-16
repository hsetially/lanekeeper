//! What is worked out about a file once and kept (P9, P11): the secret scan of its text, and where its settings are.
//!
//! Both are pure functions of the file's bytes, so they are cached by the **SHA-256 of the bytes**, computed here and not taken from
//! the port: a port that returned a hash for other bytes would otherwise make one file's scan answer for another's, and a scan that
//! is wrong is a secret shown (S14b). A cached answer never goes stale and never needs invalidating.
//!
//! The caches are bounded by weight (`LimitSettings::cache_bytes`, 96 MiB for both together, P11) and by idle time. A cache of size
//! zero keeps nothing: every call works from the bytes, which is the cold path the benchmarks measure.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use domain::{ContentHash, LineRange, SettingLocation, SettingPath};
use engine::{
    Limits, ParseError, SecretMatch, Syntax, decode_setting_path, encode_setting_path, index_settings,
};
use moka::sync::Cache;

use crate::output::{RedactError, Redactor};

/// The scan of one text.
#[derive(Debug)]
pub struct ScanFacts {
    pub matches: Vec<SecretMatch>,
}

/// Where the settings of one structured file are.
#[derive(Debug)]
pub struct IndexFacts {
    /// Every leaf in file order, each path once: what the engine indexed, without the values.
    rows: Vec<(SettingPath, LineRange)>,
    outline: Vec<SettingLocation>,
}

/// The most entries of an outline: the contract's `maxItems`.
pub const MAX_OUTLINE: usize = 200;

/// Whether `path` is `top` or below it. A key that holds `.` or `[` is written in brackets and quotes, and a quote inside a key is
/// escaped, so the text of one top-level path followed by `.` or `[` is always a descendant of it and never another key.
fn is_under(path: &str, top: &str) -> bool {
    path.strip_prefix(top)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '[']))
}

impl IndexFacts {
    /// The lines of a setting, or `None` when the file has no such setting. A scan, as the engine's own lookup is: the cache keeps
    /// the paths once and not a second table of them, and a lookup is one per call.
    #[must_use]
    pub fn locate(&self, path: &SettingPath) -> Option<LineRange> {
        self.rows.iter().find(|(p, _)| p == path).map(|(_, lines)| *lines)
    }

    /// The top-level setting paths with the lines they span, in file order.
    #[must_use]
    pub fn outline(&self) -> &[SettingLocation] {
        &self.outline
    }

    /// Takes the paths out of the engine's index. The leaves of one top-level key are next to each other in file order, so a path
    /// is first compared with the top-level key of the row before it, and taken apart (the one costly step) only when it is not
    /// under it.
    fn from_index(index: engine::SettingsIndex) -> Self {
        // Top level: the first segment of each path, with the lines of everything below it.
        let mut tops: Vec<(String, u32, u32)> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut last: Option<usize> = None;
        let mut rows = Vec::with_capacity(index.rows.len());
        for row in index.rows {
            let (start, end) = (row.lines.start(), row.lines.end());
            let slot = match last {
                Some(i) if is_under(row.path.as_str(), &tops[i].0) => i,
                _ => {
                    let top = decode_setting_path(&row.path)
                        .and_then(|d| {
                            d.segments
                                .first()
                                .cloned()
                                .map(|first| encode_setting_path(d.document, &[first]))
                        })
                        .unwrap_or_else(|| row.path.as_str().to_owned());
                    let known = by_name.get(&top).copied();
                    let i = known.unwrap_or_else(|| {
                        tops.push((top.clone(), start, end));
                        by_name.insert(top, tops.len() - 1);
                        tops.len() - 1
                    });
                    last = Some(i);
                    i
                }
            };
            let entry = &mut tops[slot];
            entry.1 = entry.1.min(start);
            entry.2 = entry.2.max(end);
            rows.push((row.path, row.lines));
        }
        let outline = tops
            .into_iter()
            .take(MAX_OUTLINE)
            .filter_map(|(top, start, end)| {
                Some(SettingLocation {
                    path: SettingPath::parse(&top).ok()?,
                    lines: LineRange::new(start, end).ok()?,
                })
            })
            .collect();
        Self { rows, outline }
    }
}

fn weigh(units: usize) -> u32 {
    u32::try_from(units).unwrap_or(u32::MAX)
}

/// Both caches. Cloning shares them.
#[derive(Clone)]
pub struct FileCache {
    scans: Option<Cache<ContentHash, Arc<ScanFacts>>>,
    indexes: Option<Cache<(ContentHash, u8), Arc<IndexFacts>>>,
}

impl std::fmt::Debug for FileCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileCache").finish_non_exhaustive()
    }
}

const fn syntax_code(syntax: Syntax) -> u8 {
    match syntax {
        Syntax::Yaml => 0,
        Syntax::Json => 1,
        Syntax::Properties => 2,
    }
}

impl FileCache {
    /// `total_bytes` is shared between the two caches. Zero turns them off.
    #[must_use]
    pub fn new(total_bytes: u64) -> Self {
        if total_bytes == 0 {
            return Self {
                scans: None,
                indexes: None,
            };
        }
        let half = total_bytes / 2;
        let idle = Duration::from_secs(30 * 60);
        Self {
            scans: Some(
                Cache::builder()
                    .max_capacity(half)
                    .weigher(|_, v: &Arc<ScanFacts>| weigh(96 + v.matches.len() * 24))
                    .time_to_idle(idle)
                    .build(),
            ),
            indexes: Some(
                Cache::builder()
                    .max_capacity(half)
                    .weigher(|_, v: &Arc<IndexFacts>| {
                        weigh(
                            128 + v.rows.iter().map(|(k, _)| k.as_str().len() + 48).sum::<usize>()
                                + v.outline.len() * 96,
                        )
                    })
                    .time_to_idle(idle)
                    .build(),
            ),
        }
    }

    /// The scan of `text`, whose bytes hash to `key`. The second value is true when the answer was already there.
    ///
    /// Blocking work: call it from `spawn_blocking`.
    ///
    /// # Errors
    ///
    /// [`RedactError`] when the scan cannot finish. That is not cached.
    pub fn scan(
        &self,
        key: &ContentHash,
        text: &str,
        redactor: &Redactor,
    ) -> Result<(Arc<ScanFacts>, bool), RedactError> {
        let compute = || redactor.scan(text).map(|matches| Arc::new(ScanFacts { matches }));
        match &self.scans {
            None => compute().map(|facts| (facts, false)),
            Some(cache) => {
                if let Some(hit) = cache.get(key) {
                    return Ok((hit, true));
                }
                let facts = compute()?;
                cache.insert(*key, Arc::clone(&facts));
                Ok((facts, false))
            }
        }
    }

    /// The settings index of `bytes` read as `syntax`, which hash to `key`. As for [`FileCache::scan`].
    ///
    /// # Errors
    ///
    /// The engine's [`ParseError`]. That is not cached.
    pub fn index(
        &self,
        key: &ContentHash,
        syntax: Syntax,
        bytes: &[u8],
    ) -> Result<(Arc<IndexFacts>, bool), ParseError> {
        let compute = || {
            index_settings(bytes, syntax, &Limits::DEFAULT)
                .map(|index| Arc::new(IndexFacts::from_index(index)))
        };
        match &self.indexes {
            None => compute().map(|facts| (facts, false)),
            Some(cache) => {
                let k = (*key, syntax_code(syntax));
                if let Some(hit) = cache.get(&k) {
                    return Ok((hit, true));
                }
                let facts = compute()?;
                cache.insert(k, Arc::clone(&facts));
                Ok((facts, false))
            }
        }
    }
}
