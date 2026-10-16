use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{
    DocGrepHit, DocHeading, DocHit, DocId, DocPath, DocText, FeatureQuery, FeatureStatus, LineRange,
    LogicalFile, RenderedDoc, Role, SettingPath, ShortText, User,
};

use super::util::lock;
use crate::conformance::DocScenario;
use crate::{
    DocError, DocSearch, HEADING_ID_PREFIX, MAX_DOC_HEADINGS, MAX_DOC_HITS, MAX_DOC_PATTERN_BYTES,
    MAX_GREP_CONTEXT, MAX_GREP_HITS, MAX_RENDER_SOURCE_BYTES, MAX_RENDERED_HTML_BYTES, content_hash,
};

const MAX_DOCS: usize = 10_000;
/// The limit of a `ShortText` in bytes (`domain::SHORT_TEXT_MAX`, which the domain does not export).
const SHORT_TEXT_BYTES: usize = 256;
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
/// by match count, and `grep` with `regex = true` answers [`DocError::BadRequest`]. `render` has no Markdown engine either: it
/// escapes every character of the text, turns `#` lines into headings with ids, and wraps the rest in paragraphs, so its
/// page is inside the allowlist whatever the text holds. Hits carry no `anchor`.
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
        anchor: None,
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
        self.grep_where(u, pattern, regex, context, None, MAX_GREP_HITS)
    }

    async fn render(&self, u: &User, path: &DocPath) -> Result<RenderedDoc, DocError> {
        check(u)?;
        let st = lock(&self.state);
        let doc = st.docs.get(path).ok_or(DocError::NotFound)?;
        render_text(path, &doc.text)
    }

    async fn feature_status(&self, u: &User, _q: FeatureQuery) -> Result<FeatureStatus, DocError> {
        check(u)?;
        Ok(lock(&self.state)
            .features
            .clone()
            .unwrap_or(FeatureStatus { flags: Vec::new() }))
    }

    async fn related_to(
        &self,
        u: &User,
        f: &LogicalFile,
        setting: Option<&SettingPath>,
        limit: usize,
    ) -> Result<Vec<DocHit>, DocError> {
        check(u)?;
        if limit == 0 || limit > MAX_DOC_HITS {
            return Err(DocError::BadRequest);
        }
        let st = lock(&self.state);
        st.docs
            .iter()
            .filter(|(_, d)| d.text.contains(f.as_str()))
            .filter(|(_, d)| setting.is_none_or(|s| d.text.contains(s.as_str())))
            .take(limit)
            .map(|(p, d)| {
                let wanted = setting.map_or_else(|| f.as_str(), SettingPath::as_str);
                let line = d.text.lines().find(|l| l.contains(wanted)).unwrap_or("");
                hit(p, d, 1.0, line, vec![f.clone()])
            })
            .collect()
    }

    async fn grep_in(
        &self,
        u: &User,
        pattern: &str,
        regex: bool,
        context: u8,
        prefix: Option<&DocPath>,
        limit: usize,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        if limit == 0 || limit > MAX_GREP_HITS {
            check(u)?;
            return Err(DocError::BadRequest);
        }
        self.grep_where(u, pattern, regex, context, prefix, limit)
    }
}

/// Escape the five characters that matter in HTML text and in a double-quoted value, and drop control characters other than
/// line feed and tab.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
}

/// An ATX heading line: one to six `#`, a space (or the end), the title without its closing `#`s.
fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let title = rest.trim();
    let without_closing = title.trim_end_matches('#');
    let title = if without_closing.len() < title.len()
        && (without_closing.is_empty() || without_closing.ends_with(' '))
    {
        without_closing.trim_end()
    } else {
        title
    };
    Some((u8::try_from(hashes).unwrap_or(6), title))
}

/// `doc-` and the title as lower case ASCII letters and digits separated by single hyphens, numbered apart from the ids in `used`.
fn heading_id(title: &str, used: &mut BTreeSet<String>) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.truncate(64);
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "section" } else { slug };
    let mut id = format!("{HEADING_ID_PREFIX}{slug}");
    let mut n = 2_u32;
    while used.contains(&id) {
        id = format!("{HEADING_ID_PREFIX}{slug}-{n}");
        n += 1;
    }
    used.insert(id.clone());
    id
}

/// The page of `text`: see [`FakeDocSearch`]. Refuses a text or a page over the bounds, and stops as soon as the page is over.
fn render_text(path: &DocPath, text: &str) -> Result<RenderedDoc, DocError> {
    if text.len() > MAX_RENDER_SOURCE_BYTES {
        return Err(DocError::BadRequest);
    }
    let mut html = String::new();
    let mut paragraph = String::new();
    let mut headings = Vec::new();
    let mut used = BTreeSet::new();
    let flush = |html: &mut String, paragraph: &mut String| {
        if !paragraph.is_empty() {
            html.push_str("<p>");
            html.push_str(paragraph);
            html.push_str("</p>\n");
            paragraph.clear();
        }
    };
    for (n, line) in text.lines().enumerate() {
        if html.len() + paragraph.len() > MAX_RENDERED_HTML_BYTES {
            return Err(DocError::BadRequest);
        }
        if let Some((level, title)) = atx_heading(line) {
            flush(&mut html, &mut paragraph);
            let id = heading_id(title, &mut used);
            let _ = write!(html, "<h{level} id=\"{id}\">");
            escape(title, &mut html);
            let _ = writeln!(html, "</h{level}>");
            if headings.len() < MAX_DOC_HEADINGS {
                let mut plain = String::new();
                escape_plain(title, &mut plain);
                headings.push(DocHeading {
                    id: ShortText::parse(&id).map_err(|_| DocError::Unavailable)?,
                    level,
                    text: ShortText::parse(&plain).map_err(|_| DocError::Unavailable)?,
                    line: u32::try_from(n + 1).unwrap_or(u32::MAX),
                });
            }
        } else if line.trim().is_empty() {
            flush(&mut html, &mut paragraph);
        } else {
            if !paragraph.is_empty() {
                paragraph.push('\n');
            }
            escape(line, &mut paragraph);
        }
    }
    flush(&mut html, &mut paragraph);
    if html.len() > MAX_RENDERED_HTML_BYTES {
        return Err(DocError::BadRequest);
    }
    Ok(RenderedDoc {
        path: path.clone(),
        html,
        headings,
        content_hash: content_hash(text.as_bytes()),
    })
}

/// The title as plain text for a [`ShortText`]: no control characters, at most [`SHORT_TEXT_BYTES`].
fn escape_plain(title: &str, out: &mut String) {
    for c in title.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > SHORT_TEXT_BYTES {
            break;
        }
        out.push(c);
    }
}

/// Whether `path` is `prefix` or below it as a folder: the prefix ends at a whole path component.
fn is_below(path: &DocPath, prefix: &DocPath) -> bool {
    let (path, prefix) = (path.as_str(), prefix.as_str());
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

impl FakeDocSearch {
    /// `grep` and `grep_in`: the lines that contain `pattern`, in the docs under `prefix`, at most `limit` hits.
    fn grep_where(
        &self,
        u: &User,
        pattern: &str,
        regex: bool,
        context: u8,
        prefix: Option<&DocPath>,
        limit: usize,
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
            if prefix.is_some_and(|p| !is_below(path, p)) {
                continue;
            }
            let lines: Vec<&str> = doc.text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.contains(pattern) {
                    continue;
                }
                if out.len() >= limit {
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
}

#[async_trait]
impl DocScenario for FakeDocSearch {
    async fn seed(&self, path: DocPath, title: &str, text: &str) {
        Self::seed(self, path, title, text);
    }
}
