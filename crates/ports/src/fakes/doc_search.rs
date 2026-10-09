use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{
    DocGrepHit, DocHit, DocId, DocPath, DocText, FeatureQuery, FeatureStatus, LineRange, LogicalFile, Role,
    ShortText, User,
};

use super::util::lock;
use crate::conformance::DocScenario;
use crate::{DocError, DocSearch, MAX_DOC_HITS, MAX_DOC_PATTERN_BYTES, MAX_GREP_CONTEXT};

const MAX_DOCS: usize = 10_000;
const MAX_GREP_HITS: usize = 500;
/// `read` returns at most this many bytes of text and sets `truncated` beyond it (MCP output cap, P9).
const MAX_READ_BYTES: usize = 16 * 1024;

struct Doc {
    id: DocId,
    title: String,
    text: String,
}

#[derive(Default)]
struct State {
    docs: BTreeMap<DocPath, Doc>,
    next_id: i64,
    features: Option<FeatureStatus>,
}

/// Substring search over seeded docs. There is no embedding, ranking model or regex engine: `search` ranks
/// by match count, and `grep` with `regex = true` answers [`DocError::BadRequest`].
#[derive(Clone, Default)]
pub struct FakeDocSearch {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for FakeDocSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeDocSearch").finish_non_exhaustive()
    }
}

fn check(u: &User) -> Result<(), DocError> {
    if u.allows(Role::Viewer) {
        Ok(())
    } else {
        Err(DocError::Forbidden)
    }
}

fn hit(
    path: &DocPath,
    doc: &Doc,
    score: f32,
    snippet: &str,
    related: Vec<LogicalFile>,
) -> Result<DocHit, DocError> {
    Ok(DocHit {
        doc: doc.id,
        title: ShortText::parse(&doc.title).map_err(|_| DocError::Unavailable)?,
        path: path.clone(),
        heading_path: Vec::new(),
        snippet: snippet.chars().take(200).collect(),
        score,
        related_files: related,
    })
}

impl FakeDocSearch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a doc. Returns false when the fake is full.
    pub fn seed(&self, path: DocPath, title: &str, text: &str) -> bool {
        let mut st = lock(&self.state);
        if st.docs.len() >= MAX_DOCS && !st.docs.contains_key(&path) {
            return false;
        }
        let id = if let Some(d) = st.docs.get(&path) {
            d.id
        } else {
            st.next_id += 1;
            DocId::from(st.next_id)
        };
        st.docs.insert(
            path,
            Doc {
                id,
                title: title.to_owned(),
                text: text.to_owned(),
            },
        );
        true
    }

    pub fn set_feature_status(&self, status: FeatureStatus) {
        lock(&self.state).features = Some(status);
    }
}

#[async_trait]
impl DocSearch for FakeDocSearch {
    async fn search(&self, u: &User, q: &str, limit: usize) -> Result<Vec<DocHit>, DocError> {
        check(u)?;
        if limit == 0 || limit > MAX_DOC_HITS || q.is_empty() || q.len() > MAX_DOC_PATTERN_BYTES {
            return Err(DocError::BadRequest);
        }
        let needle = q.to_lowercase();
        let st = lock(&self.state);
        let mut scored: Vec<(usize, &DocPath, &Doc)> = st
            .docs
            .iter()
            .filter_map(|(p, d)| {
                let n = d.text.to_lowercase().matches(&needle).count()
                    + d.title.to_lowercase().matches(&needle).count();
                (n > 0).then_some((n, p, d))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored
            .into_iter()
            .take(limit)
            .map(|(n, p, d)| {
                let line = d
                    .text
                    .lines()
                    .find(|l| l.to_lowercase().contains(&needle))
                    .unwrap_or("");
                #[allow(clippy::cast_precision_loss)]
                hit(p, d, n as f32, line, Vec::new())
            })
            .collect()
    }

    async fn related(&self, u: &User, f: &LogicalFile) -> Result<Vec<DocHit>, DocError> {
        check(u)?;
        let st = lock(&self.state);
        st.docs
            .iter()
            .filter(|(_, d)| d.text.contains(f.as_str()))
            .map(|(p, d)| {
                let line = d.text.lines().find(|l| l.contains(f.as_str())).unwrap_or("");
                hit(p, d, 1.0, line, vec![f.clone()])
            })
            .collect()
    }

    async fn read(&self, u: &User, path: &DocPath, lines: Option<LineRange>) -> Result<DocText, DocError> {
        check(u)?;
        let st = lock(&self.state);
        let doc = st.docs.get(path).ok_or(DocError::NotFound)?;
        let all: Vec<&str> = doc.text.lines().collect();
        let total = u32::try_from(all.len()).unwrap_or(u32::MAX);
        let selected: Vec<&str> = match lines {
            None => all,
            Some(r) => {
                if r.start() > total {
                    return Err(DocError::BadRequest);
                }
                all[(r.start() - 1) as usize..(r.end().min(total)) as usize].to_vec()
            }
        };
        let mut text = selected.join("\n");
        let truncated = text.len() > MAX_READ_BYTES;
        if truncated {
            let mut cut = MAX_READ_BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        Ok(DocText {
            path: path.clone(),
            lines,
            total_lines: total,
            text,
            truncated,
        })
    }

    async fn grep(
        &self,
        u: &User,
        pattern: &str,
        regex: bool,
        context: u8,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        check(u)?;
        if regex || pattern.is_empty() || pattern.len() > MAX_DOC_PATTERN_BYTES || context > MAX_GREP_CONTEXT
        {
            return Err(DocError::BadRequest);
        }
        let st = lock(&self.state);
        let ctx = usize::from(context);
        let mut out = Vec::new();
        'docs: for (path, doc) in &st.docs {
            let lines: Vec<&str> = doc.text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.contains(pattern) {
                    continue;
                }
                if out.len() >= MAX_GREP_HITS {
                    break 'docs;
                }
                out.push(DocGrepHit {
                    path: path.clone(),
                    line: u32::try_from(i + 1).unwrap_or(u32::MAX),
                    text: (*line).to_owned(),
                    before: lines[i.saturating_sub(ctx)..i]
                        .iter()
                        .map(|s| (*s).to_owned())
                        .collect(),
                    after: lines[(i + 1).min(lines.len())..(i + 1 + ctx).min(lines.len())]
                        .iter()
                        .map(|s| (*s).to_owned())
                        .collect(),
                });
            }
        }
        Ok(out)
    }

    async fn feature_status(&self, u: &User, _q: FeatureQuery) -> Result<FeatureStatus, DocError> {
        check(u)?;
        Ok(lock(&self.state)
            .features
            .clone()
            .unwrap_or(FeatureStatus { flags: Vec::new() }))
    }
}

#[async_trait]
impl DocScenario for FakeDocSearch {
    async fn seed(&self, path: DocPath, title: &str, text: &str) {
        Self::seed(self, path, title, text);
    }
}
