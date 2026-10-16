use std::collections::BTreeSet;
use std::fmt::Write as _;

use async_trait::async_trait;
use domain::{DocPath, FeatureQuery, LineRange, LogicalFile, RenderedDoc, Role, SettingPath, UserStatus};

use super::sample::{active, user};
use crate::{
    DocError, DocSearch, HEADING_ID_PREFIX, MAX_DOC_HEADINGS, MAX_DOC_HITS, MAX_DOC_PATTERN_BYTES,
    MAX_GREP_CONTEXT, MAX_GREP_HITS, MAX_HEADING_ID_BYTES, MAX_HTML_NESTING_DEPTH, MAX_RENDER_SOURCE_BYTES,
    MAX_RENDERED_HTML_BYTES, MAX_URL_BYTES,
};

/// What the docs conformance suite needs from the test harness.
#[async_trait]
pub trait DocScenario: DocSearch {
    /// Add or replace a doc with this title and exact text.
    async fn seed(&self, path: DocPath, title: &str, text: &str);

    /// Seed a doc at `path` whose text holds a value that this implementation's secret scan flags, and return that value.
    /// Additive (`contract/08-web`, CG11). The default is `None`: the scenario cannot plant one, and
    /// [`render_output_stays_inside_the_allowlist`] skips the check that no page carries a flagged value (S14b). A real
    /// implementation overrides it, because the order "scan, then render" is a security property that only a planted value shows.
    async fn plant_secret(&self, _path: DocPath) -> Option<String> {
        None
    }
}

/// - Pending, disabled and role-less users are `Forbidden` everywhere (S4).
/// - Limits are enforced: `limit` 1..=100, patterns at most 512 bytes, grep context at most 10.
/// - `search` finds matching docs, `read` returns exact lines of a doc (`NotFound` for unknown paths,
///   `BadRequest` for a range past the end), `grep` returns 1-based line numbers with bounded context.
/// - `related_to` (`contract/07-mcp`) returns the docs that mention the file, and the setting too when one is given, at most
///   `limit` of them (1..=100). `grep_in` greps the docs under a path prefix only, whole path components, at most `limit` hits
///   (1..=500), with the limits of `grep`. An implementation that keeps the `Unavailable` default of these fails here.
/// - `render` (`contract/08-web`, CG11) passes [`render_output_stays_inside_the_allowlist`], and every `anchor` of a hit names
///   a heading of the rendered page of that hit.
pub async fn doc_search<D: DocScenario + ?Sized>(d: &D) {
    let viewer = active(1, Role::Viewer);
    let guide = DocPath::parse("/git/main/guides/alpha.md").unwrap();
    let other = DocPath::parse("/git/main/guides/beta.md").unwrap();
    d.seed(
        guide.clone(),
        "Alpha guide",
        "# Alpha\nline two mentions alpha twice: alpha\nline three\nsee tx-api/core.yml for the flag\n",
    )
    .await;
    d.seed(other, "Beta guide", "# Beta\nnothing relevant\n").await;
    // A doc of another folder that mentions the same file and a setting the first one does not, and the word `mentions`.
    let ports = DocPath::parse("/git/main/reference/ports.md").unwrap();
    d.seed(
        ports.clone(),
        "Ports reference",
        "# Ports\nthis page mentions tx-api/core.yml and the setting server.port\n",
    )
    .await;

    for bad in [
        user(2, Some(Role::Admin), UserStatus::Pending),
        user(3, Some(Role::Admin), UserStatus::Disabled),
        user(4, None, UserStatus::Active),
    ] {
        assert_eq!(
            d.search(&bad, "alpha", 10).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(d.read(&bad, &guide, None).await.unwrap_err(), DocError::Forbidden);
        assert_eq!(
            d.grep(&bad, "alpha", false, 0).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(
            d.related(&bad, &core_yml()).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(
            d.feature_status(&bad, empty_query()).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(
            d.related_to(&bad, &core_yml(), None, 10).await.unwrap_err(),
            DocError::Forbidden
        );
        assert_eq!(
            d.grep_in(&bad, "alpha", false, 0, None, 10).await.unwrap_err(),
            DocError::Forbidden
        );
    }

    let hits = d.search(&viewer, "alpha", 10).await.unwrap();
    assert_eq!(
        hits.first().map(|h| h.path.clone()),
        Some(guide.clone()),
        "best match first"
    );
    assert!(hits.len() <= 10);
    assert_eq!(d.search(&viewer, "alpha", 1).await.unwrap().len(), 1);
    assert_eq!(
        d.search(&viewer, "alpha", 0).await.unwrap_err(),
        DocError::BadRequest
    );
    assert_eq!(
        d.search(&viewer, "alpha", MAX_DOC_HITS + 1).await.unwrap_err(),
        DocError::BadRequest
    );
    let long = "a".repeat(MAX_DOC_PATTERN_BYTES + 1);
    assert_eq!(
        d.search(&viewer, &long, 10).await.unwrap_err(),
        DocError::BadRequest
    );

    let full = d.read(&viewer, &guide, None).await.unwrap();
    assert_eq!(full.total_lines, 4);
    assert!(full.text.starts_with("# Alpha"));
    let part = d
        .read(&viewer, &guide, Some(LineRange::new(2, 3).unwrap()))
        .await
        .unwrap();
    assert_eq!(part.text, "line two mentions alpha twice: alpha\nline three");
    assert_eq!(
        d.read(&viewer, &guide, Some(LineRange::new(50, 60).unwrap()))
            .await
            .unwrap_err(),
        DocError::BadRequest
    );
    let missing = DocPath::parse("/git/main/nope.md").unwrap();
    assert_eq!(
        d.read(&viewer, &missing, None).await.unwrap_err(),
        DocError::NotFound
    );

    let found = d.grep(&viewer, "line three", false, 1).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, 3, "grep line numbers are 1-based");
    assert_eq!(found[0].before, ["line two mentions alpha twice: alpha"]);
    assert_eq!(found[0].after, ["see tx-api/core.yml for the flag"]);
    assert_eq!(
        d.grep(&viewer, "alpha", false, MAX_GREP_CONTEXT + 1)
            .await
            .unwrap_err(),
        DocError::BadRequest
    );
    assert_eq!(
        d.grep(&viewer, &long, false, 0).await.unwrap_err(),
        DocError::BadRequest
    );

    let related = d.related(&viewer, &core_yml()).await.unwrap();
    assert!(
        related.iter().any(|h| h.path == guide),
        "docs that mention the file"
    );

    d.feature_status(&viewer, empty_query())
        .await
        .expect("feature status");

    related_to_and_grep_in(d, &viewer, &guide, &ports).await;

    render_output_stays_inside_the_allowlist(d).await;
    hit_anchors_name_a_heading(d, &viewer).await;
}

/// An `anchor` is optional, but when a hit or a flag has one it is a heading id, and for a hit it is the id of a heading of the
/// rendered page of the hit's own doc: the reader opens the page at that heading, so an id that names nothing is a dead link.
async fn hit_anchors_name_a_heading<D: DocScenario + ?Sized>(d: &D, viewer: &domain::User) {
    let mut hits = d.search(viewer, "alpha", 10).await.unwrap();
    hits.extend(d.related(viewer, &core_yml()).await.unwrap());
    hits.extend(d.related_to(viewer, &core_yml(), None, 10).await.unwrap());
    for hit in hits {
        let Some(anchor) = hit.anchor else { continue };
        assert!(
            is_heading_id(anchor.as_str()),
            "the anchor {anchor} of a hit in {} is not a heading id",
            hit.path
        );
        let page = d.render(viewer, &hit.path).await.unwrap();
        assert!(
            page.headings.iter().any(|h| h.id == anchor),
            "the anchor {anchor} of a hit names no heading of the rendered page of {}",
            hit.path
        );
    }
    for flag in d.feature_status(viewer, empty_query()).await.unwrap().flags {
        if let Some(anchor) = flag.anchor {
            assert!(
                is_heading_id(anchor.as_str()),
                "the anchor {anchor} of flag {} is not a heading id",
                flag.flag
            );
        }
    }
}

/// `related_to` and `grep_in`: the setting, the limit and the prefix narrow the answer, and the limits are enforced.
async fn related_to_and_grep_in<D: DocScenario + ?Sized>(
    d: &D,
    viewer: &domain::User,
    guide: &DocPath,
    ports: &DocPath,
) {
    let paths = |hits: &[domain::DocHit]| hits.iter().map(|h| h.path.clone()).collect::<Vec<_>>();
    let setting = SettingPath::parse("server.port").unwrap();

    // Docs that mention the file; with a setting, only those that mention it too; at most `limit`.
    let both = paths(&d.related_to(viewer, &core_yml(), None, 10).await.unwrap());
    assert!(
        both.contains(guide) && both.contains(ports),
        "both docs mention the file: {both:?}"
    );
    let narrowed = paths(
        &d.related_to(viewer, &core_yml(), Some(&setting), 10)
            .await
            .unwrap(),
    );
    assert_eq!(
        narrowed,
        [ports.clone()],
        "only the doc that mentions the setting as well"
    );
    let unknown = SettingPath::parse("no.such.setting").unwrap();
    assert!(
        d.related_to(viewer, &core_yml(), Some(&unknown), 10)
            .await
            .unwrap()
            .is_empty(),
        "a setting no doc mentions is an empty answer, not an error"
    );
    assert_eq!(
        d.related_to(viewer, &core_yml(), None, 1).await.unwrap().len(),
        1,
        "at most `limit`"
    );
    for limit in [0, MAX_DOC_HITS + 1] {
        assert_eq!(
            d.related_to(viewer, &core_yml(), None, limit).await.unwrap_err(),
            DocError::BadRequest,
            "limit {limit}"
        );
    }
    assert!(
        d.related_to(viewer, &core_yml(), None, MAX_DOC_HITS)
            .await
            .is_ok()
    );

    // Grep under a prefix: a whole path component, the prefix itself or below it.
    let all = d.grep_in(viewer, "mentions", false, 0, None, 20).await.unwrap();
    let hit_paths = paths_of_grep(&all);
    assert!(
        hit_paths.contains(guide) && hit_paths.contains(ports),
        "both docs say `mentions`: {hit_paths:?}"
    );
    let guides = DocPath::parse("/git/main/guides").unwrap();
    let under = d
        .grep_in(viewer, "mentions", false, 0, Some(&guides), 20)
        .await
        .unwrap();
    assert_eq!(
        paths_of_grep(&under),
        [guide.clone()],
        "only the docs below the prefix"
    );
    let itself = d
        .grep_in(viewer, "mentions", false, 0, Some(guide), 20)
        .await
        .unwrap();
    assert_eq!(
        paths_of_grep(&itself),
        [guide.clone()],
        "the prefix may be a doc itself"
    );
    let partial = DocPath::parse("/git/main/guide").unwrap();
    assert!(
        d.grep_in(viewer, "mentions", false, 0, Some(&partial), 20)
            .await
            .unwrap()
            .is_empty(),
        "`guide` is not the folder `guides`"
    );
    let uploads = DocPath::parse("/uploads/none").unwrap();
    assert!(
        d.grep_in(viewer, "mentions", false, 0, Some(&uploads), 20)
            .await
            .unwrap()
            .is_empty(),
        "a prefix with no docs is an empty answer, not an error"
    );
    // Context and line numbers are `grep`'s.
    let ctx = d
        .grep_in(viewer, "line three", false, 1, Some(&guides), 20)
        .await
        .unwrap();
    assert_eq!(ctx.len(), 1);
    assert_eq!(ctx[0].line, 3, "1-based, as for `grep`");
    assert_eq!(ctx[0].before, ["line two mentions alpha twice: alpha"]);

    // At most `limit` hits, and the limits of `grep`.
    assert_eq!(
        d.grep_in(viewer, "mentions", false, 0, None, 1)
            .await
            .unwrap()
            .len(),
        1
    );
    for limit in [0, MAX_GREP_HITS + 1] {
        assert_eq!(
            d.grep_in(viewer, "mentions", false, 0, None, limit)
                .await
                .unwrap_err(),
            DocError::BadRequest,
            "limit {limit}"
        );
    }
    assert!(
        d.grep_in(viewer, "mentions", false, 0, None, MAX_GREP_HITS)
            .await
            .is_ok()
    );
    let long = "a".repeat(MAX_DOC_PATTERN_BYTES + 1);
    assert_eq!(
        d.grep_in(viewer, &long, false, 0, None, 10).await.unwrap_err(),
        DocError::BadRequest
    );
    assert_eq!(
        d.grep_in(viewer, "mentions", false, MAX_GREP_CONTEXT + 1, None, 10)
            .await
            .unwrap_err(),
        DocError::BadRequest
    );
}

fn paths_of_grep(hits: &[domain::DocGrepHit]) -> Vec<DocPath> {
    let mut seen: Vec<DocPath> = Vec::new();
    for h in hits {
        if !seen.contains(&h.path) {
            seen.push(h.path.clone());
        }
    }
    seen.sort();
    seen
}

fn empty_query() -> FeatureQuery {
    FeatureQuery {
        flag: None,
        file: None,
        channel: None,
        swimlanes: Vec::new(),
    }
}

fn core_yml() -> LogicalFile {
    LogicalFile::parse("tx-api/core.yml").unwrap()
}

// ---------------------------------------------------------------------------------------------------------------
// Rendered docs (`contract/08-web`, CG11): the one HTML sink the CSP lets through (S12)

/// The elements a rendered page may contain, and nothing else. The same list is in the description of `RenderedDoc.html` in
/// `api/openapi.yaml`; `crates/ports/tests/web_contract_docs.rs` keeps the two equal. Attributes, per element: `id` on a
/// heading (`doc-` and a slug, unique), `href` and `rel` on `a`, `src` and `alt` on `img`; no other.
pub const ALLOWED_HTML_TAGS: &[&str] = &[
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "p",
    "blockquote",
    "pre",
    "code",
    "ul",
    "ol",
    "li",
    "hr",
    "br",
    "em",
    "strong",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "a",
    "img",
];

/// Elements with no closing tag.
const VOID_TAGS: [&str; 3] = ["br", "hr", "img"];

/// A checker reports at most this many violations: the first ones are the ones to read.
const MAX_VIOLATIONS: usize = 20;

/// Whether `s` is a heading id of a rendered page: `doc-` and a slug of lower case letters and digits separated by single
/// hyphens, at most [`MAX_HEADING_ID_BYTES`] (the pattern of `DocHeading.id`).
pub fn is_heading_id(s: &str) -> bool {
    s.len() <= MAX_HEADING_ID_BYTES
        && s.strip_prefix(HEADING_ID_PREFIX).is_some_and(|slug| {
            slug.split('-').all(|part| {
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
        })
}

/// A URL value of a rendered page, as it stands: printable ASCII of at most [`MAX_URL_BYTES`], no space, tab, line break or other
/// control character (a browser drops a tab and a line break from a URL before it reads it), no backslash (it reads `/\` as
/// `//`), none of `<>"'` or the backtick. This is the only check that refuses a tab or a line break inside a value: the
/// page-wide control check lets both through, because they are legal in text.
fn is_plain_url(u: &str) -> bool {
    u.len() <= MAX_URL_BYTES
        && u.is_ascii()
        && !u.bytes().any(|b| {
            b.is_ascii_control() || b == b' ' || matches!(b, b'\\' | b'<' | b'>' | b'"' | b'\'' | b'`')
        })
}

/// An absolute `https:` URL with a host and no credentials.
fn is_https_url(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    is_plain_url(u) && !authority.is_empty() && !authority.contains('@')
}

/// A path on this origin: one leading `/`, never `//`.
fn is_origin_path(u: &str) -> bool {
    u.starts_with('/') && !u.starts_with("//") && is_plain_url(u)
}

fn short(s: &str) -> String {
    s.chars().take(60).collect()
}

/// The five entities the checker lets an attribute value hold, decoded. Any other `&` is a violation: a browser decodes `&colon;`
/// and `&#106;`, and a check on the raw text would not see what it sees.
fn unescape_attr(raw: &str) -> Result<String, String> {
    const ENTITIES: [(&str, char); 5] = [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&#39;", '\''),
    ];
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some((entity, ch)) = ENTITIES.iter().find(|(e, _)| rest.starts_with(e)) else {
            return Err(format!(
                "an entity outside amp, lt, gt, quot and #39 in the attribute value {:?}",
                short(raw)
            ));
        };
        out.push(*ch);
        rest = &rest[entity.len()..];
    }
    out.push_str(rest);
    Ok(out)
}

fn attribute_allowed(tag: &str, name: &str) -> bool {
    matches!(
        (tag, name),
        ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", "id") | ("a", "href" | "rel") | ("img", "src" | "alt")
    )
}

fn check_attribute_value(tag: &str, name: &str, value: &str) -> Result<(), String> {
    let ok = match name {
        "id" => is_heading_id(value),
        "href" => {
            is_https_url(value) || is_origin_path(value) || value.strip_prefix('#').is_some_and(is_heading_id)
        }
        "rel" => value == "noopener noreferrer",
        "src" => is_origin_path(value),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "the value {:?} of {name} on <{tag}> is outside the allowlist",
            short(value)
        ))
    }
}

/// The name that starts at `from`: ASCII letters and digits.
fn alnum_name(html: &str, from: usize) -> &str {
    let end = from
        + html.as_bytes()[from..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
    &html[from..end]
}

fn scan_closing(html: &str, start: usize, stack: &mut Vec<&'static str>) -> Result<usize, String> {
    let name = alnum_name(html, start + 2);
    let end = start + 2 + name.len();
    let Some(tag) = ALLOWED_HTML_TAGS.iter().copied().find(|t| *t == name) else {
        return Err(format!("closing tag </{name}> is not in the allowlist"));
    };
    if VOID_TAGS.contains(&tag) {
        return Err(format!("</{tag}>: a void element has no closing tag"));
    }
    if html.as_bytes().get(end) != Some(&b'>') {
        return Err(format!(
            "the closing tag </{tag} at byte {start} is not `</{tag}>`"
        ));
    }
    match stack.pop() {
        Some(open) if open == tag => Ok(end + 1),
        Some(open) => Err(format!(
            "</{tag}> closes <{open}>: elements are not properly nested"
        )),
        None => Err(format!("</{tag}> closes nothing")),
    }
}

/// An attribute at `from`: its name, its decoded value and the byte after its closing quote.
fn scan_attribute<'a>(html: &'a str, from: usize, tag: &str) -> Result<(&'a str, String, usize), String> {
    let b = html.as_bytes();
    let end = from
        + b[from..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(**c, b'-' | b':' | b'_'))
            .count();
    let name = &html[from..end];
    if !attribute_allowed(tag, name) {
        return Err(format!("attribute {name} is not allowed on <{tag}>"));
    }
    if b.get(end) != Some(&b'=') || b.get(end + 1) != Some(&b'"') {
        return Err(format!("attribute {name} of <{tag}> has no double-quoted value"));
    }
    let value_start = end + 2;
    let Some(len) = b[value_start..].iter().position(|c| *c == b'"') else {
        return Err(format!("the value of {name} on <{tag}> is never closed"));
    };
    let raw = &html[value_start..value_start + len];
    if raw.contains(['<', '>']) {
        return Err(format!(
            "the value of {name} on <{tag}> holds a raw angle bracket"
        ));
    }
    let value = unescape_attr(raw)?;
    check_attribute_value(tag, name, &value)?;
    Ok((name, value, value_start + len + 1))
}

/// The tag that starts at `start` (a `<`): the byte after it, or why it is outside the allowlist.
fn scan_tag(
    html: &str,
    start: usize,
    stack: &mut Vec<&'static str>,
    ids: &mut BTreeSet<String>,
) -> Result<usize, String> {
    let b = html.as_bytes();
    match b.get(start + 1) {
        None => return Err("a `<` at the end of the page".to_owned()),
        Some(b'!' | b'?') => {
            return Err(format!(
                "a comment, doctype, CDATA section or processing instruction at byte {start}"
            ));
        }
        Some(b'/') => return scan_closing(html, start, stack),
        Some(c) if c.is_ascii_alphabetic() => {}
        Some(_) => {
            return Err(format!(
                "a `<` that starts no tag at byte {start}: in text it is `&lt;`"
            ));
        }
    }
    let name = alnum_name(html, start + 1);
    let Some(tag) = ALLOWED_HTML_TAGS.iter().copied().find(|t| *t == name) else {
        return Err(format!("element <{name}> is not in the allowlist"));
    };
    // The depth of an element is its open ancestors and itself, a void element included: a page is not a tower.
    if stack.len() >= MAX_HTML_NESTING_DEPTH {
        return Err(format!(
            "<{tag}> is nested deeper than {MAX_HTML_NESTING_DEPTH} levels"
        ));
    }
    let void = VOID_TAGS.contains(&tag);
    let mut i = start + 1 + name.len();
    let mut seen: Vec<(&str, String)> = Vec::new();
    loop {
        match b.get(i) {
            None => return Err(format!("the tag <{tag}> is never finished")),
            Some(b'>') => {
                i += 1;
                break;
            }
            Some(b'/') if b.get(i + 1) == Some(&b'>') => {
                if !void {
                    return Err(format!(
                        "<{tag}/> is self-closing, and only br, hr and img are void"
                    ));
                }
                i += 2;
                break;
            }
            Some(b' ') => {
                i += 1;
                if b.get(i).is_some_and(u8::is_ascii_alphabetic) {
                    let (name, value, next) = scan_attribute(html, i, tag)?;
                    if seen.iter().any(|(n, _)| *n == name) {
                        return Err(format!("attribute {name} twice on <{tag}>"));
                    }
                    seen.push((name, value));
                    i = next;
                }
            }
            Some(_) => return Err(format!("an unexpected character in the tag <{tag}> at byte {i}")),
        }
    }
    let has = |n: &str| seen.iter().any(|(name, _)| *name == n);
    match tag {
        "a" => {
            if !(has("href") && has("rel")) {
                return Err("an <a> needs an href and rel=\"noopener noreferrer\"".to_owned());
            }
            if stack.contains(&"a") {
                return Err("an <a> inside an <a>".to_owned());
            }
        }
        "img" if !has("src") => return Err("an <img> needs a src".to_owned()),
        _ => {}
    }
    if let Some((_, id)) = seen.iter().find(|(n, _)| *n == "id") {
        if !ids.insert(id.clone()) {
            return Err(format!("the id {id} twice in one page"));
        }
    }
    if !void {
        stack.push(tag);
    }
    Ok(i)
}

/// Every way in which `html` leaves the allowlist of a rendered page (S12), at most [`MAX_VIOLATIONS`] of them; empty when it
/// stays inside. The allowlist is the one stated in `api/openapi.yaml` (`RenderedDoc.html`):
/// - the elements of [`ALLOWED_HTML_TAGS`], in lower case, closed and properly nested at most [`MAX_HTML_NESTING_DEPTH`] deep (a
///   void element counts), an `a` never inside an `a`;
/// - `id` on `h1` to `h6` only, a [`is_heading_id`], unique in the page; `href` and `rel="noopener noreferrer"` on `a`, the
///   `href` an absolute `https:` URL without credentials, a path on this origin or a `#` fragment naming a heading id; `src` (a
///   path on this origin) and `alt` on `img`; every URL value a plain one (see `is_plain_url`: printable ASCII, no space, tab,
///   line break, backslash, `<`, `>`, quote or backtick, at most [`MAX_URL_BYTES`]); no other attribute, every value in double
///   quotes;
/// - every `&` of a value begins `&amp;`, `&lt;`, `&gt;`, `&quot;` or `&#39;`; text has no `<`;
/// - no comment, doctype, CDATA section or processing instruction, and no control character except line feed and tab.
///
/// A small scanner on purpose, with no dependency: it reads what a browser would read, strictly, so that a page the checker
/// accepts is one the browser parses into exactly these elements.
pub fn html_violations(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some((at, c)) = html
        .char_indices()
        .find(|(_, c)| c.is_control() && *c != '\n' && *c != '\t')
    {
        out.push(format!("control character U+{:04X} at byte {at}", u32::from(c)));
    }
    let b = html.as_bytes();
    let mut stack: Vec<&'static str> = Vec::new();
    let mut ids = BTreeSet::new();
    let mut i = 0;
    while i < b.len() && out.len() < MAX_VIOLATIONS {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        match scan_tag(html, i, &mut stack, &mut ids) {
            Ok(next) => i = next,
            Err(why) => {
                out.push(why);
                i += 1;
            }
        }
    }
    if out.len() < MAX_VIOLATIONS {
        if let Some(open) = stack.last() {
            out.push(format!("element <{open}> is never closed"));
        }
    }
    out
}

/// Everything wrong with a [`RenderedDoc`]: its page ([`html_violations`], at most [`MAX_RENDERED_HTML_BYTES`]) and its outline
/// against the page. Each heading is of level 1 to 6, has a [`is_heading_id`] that no other heading has, sits on a later line
/// than the one before, and is an element of that level with that id in the page; the outline has at most
/// [`MAX_DOC_HEADINGS`] entries.
pub fn rendered_doc_violations(doc: &RenderedDoc) -> Vec<String> {
    let mut out = Vec::new();
    if doc.html.len() > MAX_RENDERED_HTML_BYTES {
        out.push(format!(
            "the page is {} bytes, over the bound of {MAX_RENDERED_HTML_BYTES} bytes",
            doc.html.len()
        ));
    }
    out.extend(html_violations(&doc.html));
    if doc.headings.len() > MAX_DOC_HEADINGS {
        out.push(format!(
            "the outline has {} headings, over the bound of {MAX_DOC_HEADINGS}",
            doc.headings.len()
        ));
    }
    let mut ids = BTreeSet::new();
    let mut last_line = 0;
    for h in &doc.headings {
        let id = h.id.as_str();
        if !(1..=6).contains(&h.level) {
            out.push(format!("heading {id} has level {}", h.level));
        }
        if !is_heading_id(id) {
            out.push(format!("{id:?} is not a heading id"));
        }
        if !ids.insert(id) {
            out.push(format!("heading id {id} twice in the outline"));
        }
        if h.line <= last_line {
            out.push(format!(
                "heading {id} is on line {}, not after line {last_line} (lines are 1-based, in document order)",
                h.line
            ));
        }
        last_line = h.line;
        if !doc.html.contains(&format!("<h{} id=\"{id}\">", h.level)) {
            out.push(format!("heading id {id} (level {}) is not in the html", h.level));
        }
    }
    out.truncate(MAX_VIOLATIONS);
    out
}

/// Markdown that tries to get markup, a script, a URL or an element name past the renderer. Every doc below is small and
/// valid text; none may make the page leave the allowlist, whether the implementation escapes, removes or rewrites what it
/// does not allow. The entry named `marker-text` must also keep the marker of a masked value as text.
pub const XSS_CORPUS: &[(&str, &str)] = &[
    ("script-block", "# Title\n\n<script>alert(1)</script>\n\nafter\n"),
    ("script-inline", "text <script>alert(1)</script> text\n"),
    ("img-onerror", "<img src=x onerror=alert(1)>\n"),
    ("javascript-link", "[x](javascript:alert(1))\n"),
    ("javascript-link-mixed-case", "[x](JaVaScRiPt:alert(1))\n"),
    ("javascript-link-entity", "[x](&#106;avascript:alert(1))\n"),
    ("javascript-link-tab", "[x](<jav&#x09;ascript:alert(1)>)\n"),
    (
        "data-link",
        "[x](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)\n",
    ),
    ("vbscript-link", "[x](vbscript:msgbox(1))\n"),
    ("http-link", "[x](http://insecure.example/)\n"),
    ("protocol-relative-link", "[x](//evil.example/)\n"),
    ("backslash-link", "[x](/\\evil.example/)\n"),
    ("userinfo-link", "[x](https://docs.example@evil.example/)\n"),
    ("autolink-javascript", "<javascript:alert(1)>\n"),
    (
        "style-attribute",
        "<p style=\"background:url(javascript:alert(1))\">styled</p>\n",
    ),
    (
        "event-handlers",
        "<p onclick=\"alert(1)\" onmouseover=\"alert(2)\">hover</p>\n",
    ),
    ("iframe", "<iframe src=\"https://evil.example/\"></iframe>\n"),
    (
        "object-embed",
        "<object data=\"x.swf\"></object><embed src=\"x.swf\">\n",
    ),
    ("svg", "<svg onload=alert(1)><circle r=1></circle></svg>\n"),
    (
        "math",
        "<math><mi xlink:href=\"javascript:alert(1)\">x</mi></math>\n",
    ),
    (
        "form",
        "<form action=\"https://evil.example/\"><input name=password><button>go</button></form>\n",
    ),
    (
        "base-meta-link",
        "<base href=\"https://evil.example/\"><meta http-equiv=\"refresh\" content=\"0;url=https://evil.example/\">\
         <link rel=\"stylesheet\" href=\"https://evil.example/x.css\">\n",
    ),
    (
        "style-element",
        "<style>@import url(https://evil.example/x.css);</style>\n",
    ),
    ("data-image", "![x](data:image/png;base64,iVBORw0KGgo=)\n"),
    (
        "data-image-html",
        "<img src=\"data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=\">\n",
    ),
    ("external-image", "![x](https://evil.example/pixel.png)\n"),
    ("protocol-relative-image", "![x](//evil.example/pixel.png)\n"),
    // Raw anchors and images of a sanitiser that trusts a path-shaped URL (`/` and not `//`, so "a path on this origin"). Each
    // is inside the allowlist but for its URL value (`rel` and `alt` are right), so the URL value is the one rule that has to
    // stop it: a browser drops a tab and a line break from a URL and reads a backslash as a slash, so `/<tab>/evil.example/`
    // and `/\evil.example/` are `//evil.example/`. A renderer that escapes raw HTML, as the fake does, passes them trivially;
    // one that lets an `a` or an `img` through does not. `crates/ports/tests/web_contract_docs.rs` keeps the checker honest
    // on every one of them.
    (
        "raw-a-protocol-relative",
        "<a href=\"//evil.example/\" rel=\"noopener noreferrer\">x</a>\n",
    ),
    (
        "raw-a-backslash",
        "<a href=\"/\\evil.example/\" rel=\"noopener noreferrer\">x</a>\n",
    ),
    (
        "raw-a-tab-path",
        "<a href=\"/\t/evil.example/\" rel=\"noopener noreferrer\">x</a>\n",
    ),
    (
        "raw-a-line-feed-path",
        "<a href=\"/\n/evil.example/\" rel=\"noopener noreferrer\">x</a>\n",
    ),
    (
        "raw-img-protocol-relative",
        "<img src=\"//evil.example/p.png\" alt=\"x\">\n",
    ),
    (
        "raw-img-backslash",
        "<img src=\"/\\evil.example/p.png\" alt=\"x\">\n",
    ),
    (
        "raw-img-tab-path",
        "<img src=\"/\t/evil.example/p.png\" alt=\"x\">\n",
    ),
    (
        "raw-img-line-feed-path",
        "<img src=\"/\n/evil.example/p.png\" alt=\"x\">\n",
    ),
    // A destination in angle brackets may hold a tab: a renderer that passes the destination on as it is emits it.
    ("md-link-tab-path", "[x](</\t/evil.example/>)\n"),
    ("md-image-tab-path", "![x](</\t/evil.example/p.png>)\n"),
    // Relative links: the page rewrites one to `/docs/view?path=` or drops it, and never emits it as it stands.
    ("relative-md-link", "[guide](guide.md)\n"),
    ("relative-md-link-parent", "[up](../x.md)\n"),
    ("relative-md-link-fragment", "[usage](guide.md#usage)\n"),
    (
        "srcset-image",
        "<img src=\"/a.png\" srcset=\"https://evil.example/b.png 2x\">\n",
    ),
    (
        "html-comment",
        "<!-- a comment --><!--[if IE]><script>alert(1)</script><![endif]-->\n",
    ),
    (
        "cdata-doctype-pi",
        "<![CDATA[x]]><!DOCTYPE html><?php echo 1 ?>\n",
    ),
    (
        "id-clobbering",
        "<h1 id=\"admin\">x</h1><a id=\"root\" name=\"root\">y</a><p id=\"__proto__\">z</p>\n\n# admin\n\n# root\n",
    ),
    (
        "target-blank",
        "<a href=\"https://docs.example/\" target=\"_blank\">x</a>\n",
    ),
    (
        "details-ontoggle",
        "<details open ontoggle=alert(1)><summary>x</summary></details>\n",
    ),
    ("nested-malformed", "<scr<script>ipt>alert(1)</scr</script>ipt>\n"),
    (
        "template-noscript",
        "<template><script>alert(1)</script></template>\
         <noscript><p title=\"</noscript><img src=x onerror=alert(1)>\"></noscript>\n",
    ),
    (
        "heading-with-markup",
        "# <img src=x onerror=alert(1)> Title <script>x</script>\n",
    ),
    (
        "table-with-markup",
        "| a | b |\n|---|---|\n| <script>1</script> | [x](javascript:1) |\n",
    ),
    (
        "code-with-markup",
        "```html\n<script>alert(1)</script>\n```\n\n`<img src=x onerror=1>`\n",
    ),
    (
        "entities-in-text",
        "&lt;script&gt;alert(1)&lt;/script&gt; &amp; &#60;img src=x onerror=alert(1)&#62;\n",
    ),
    ("control-characters", "a\u{7}b\u{1b}[31mc\u{85}d\u{8}e\n"),
    (
        "deep-nesting",
        ">>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>> deep\n",
    ),
    // Nesting: a page is at most 32 deep, so a renderer drops the containers beyond it and keeps their content.
    (
        "deep-nesting-quotes",
        "> > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > > deep\n",
    ),
    (
        "deep-nesting-lists",
        "- item 0\n  - item 1\n    - item 2\n      - item 3\n        - item 4\n          - item 5\n            - item 6\n              - item 7\n                - item 8\n                  - item 9\n                    - item 10\n                      - item 11\n                        - item 12\n                          - item 13\n                            - item 14\n                              - item 15\n                                - item 16\n                                  - item 17\n                                    - item 18\n                                      - item 19\n                                        - item 20\n                                          - item 21\n                                            - item 22\n                                              - item 23\n                                                - item 24\n                                                  - item 25\n                                                    - item 26\n                                                      - item 27\n                                                        - item 28\n                                                          - item 29\n                                                            - item 30\n                                                              - item 31\n                                                                - item 32\n                                                                  - item 33\n                                                                    - item 34\n                                                                      - item 35\n                                                                        - item 36\n                                                                          - item 37\n                                                                            - item 38\n                                                                              - item 39\n",
    ),
    ("marker-text", "The value is [REDACTED: 12 chars, api_key] here\n"),
];

/// The outline document: CRLF line endings, a repeated heading and a heading on level 3. Headings: (1, Overview, line 1),
/// (2, Usage, 5), (2, Usage, 9), (3, Fine print, 11).
const OUTLINE: &str = "# Overview\r\n\r\nText with `code`.\r\n\r\n## Usage\r\n\r\nmore\r\n\r\n## Usage\r\n\r\n### Fine print\r\n";

/// `DocSearch::render`: the page of every doc stays inside the allowlist (S12), agrees with its outline, is bounded and is
/// addressed by content.
///
/// - Pending, disabled and role-less users are `Forbidden`, an unknown path is `NotFound`.
/// - The outline of a CRLF doc is right: levels, texts, 1-based lines, unique ids that the page carries.
/// - The content hash follows the text, not the path: the same text twice, even under two paths, is one hash.
/// - A doc over [`MAX_RENDER_SOURCE_BYTES`], and one whose page would pass [`MAX_RENDERED_HTML_BYTES`], is refused
///   (`BadRequest`), never truncated; an outline longer than [`MAX_DOC_HEADINGS`] is cut at that many.
/// - When the scenario can plant a value its secret scan flags ([`DocScenario::plant_secret`]), the value is not in the page.
/// - Every doc of [`XSS_CORPUS`] renders to a page for which [`rendered_doc_violations`] is empty, and the marker of a masked
///   value is text in the page.
pub async fn render_output_stays_inside_the_allowlist<D: DocScenario + ?Sized>(d: &D) {
    let viewer = active(1, Role::Viewer);
    let at = |name: &str| DocPath::parse(&format!("/git/main/render/{name}.md")).unwrap();

    // S4, and a path that is not there.
    let outline = at("outline");
    d.seed(outline.clone(), "Outline", OUTLINE).await;
    for bad in [
        user(2, Some(Role::Admin), UserStatus::Pending),
        user(3, Some(Role::Admin), UserStatus::Disabled),
        user(4, None, UserStatus::Active),
    ] {
        assert_eq!(
            d.render(&bad, &outline).await.unwrap_err(),
            DocError::Forbidden,
            "render: a user who may not read docs"
        );
    }
    assert_eq!(
        d.render(&viewer, &at("missing")).await.unwrap_err(),
        DocError::NotFound,
        "render: an unknown path"
    );

    // The outline, with CRLF line endings.
    let page = d.render(&viewer, &outline).await.unwrap();
    assert_eq!(page.path, outline, "the page says whose it is");
    let violations = rendered_doc_violations(&page);
    assert!(
        violations.is_empty(),
        "outline: the rendered page leaves the allowlist or disagrees with its outline: {violations:?}"
    );
    let got: Vec<(u8, &str, u32)> = page
        .headings
        .iter()
        .map(|h| (h.level, h.text.as_str(), h.line))
        .collect();
    assert_eq!(
        got,
        [
            (1, "Overview", 1),
            (2, "Usage", 5),
            (2, "Usage", 9),
            (3, "Fine print", 11)
        ],
        "the outline: levels, plain texts, 1-based lines, in document order"
    );
    for h in &page.headings {
        let element = format!("<h{0} id=\"{1}\">{2}</h{0}>", h.level, h.id, h.text);
        assert!(
            page.html.contains(&element),
            "a plain heading is the element {element}, with its text as the content"
        );
    }

    // The content hash follows the text.
    let twin = at("twin");
    d.seed(twin.clone(), "Twin", OUTLINE).await;
    let again = d.render(&viewer, &outline).await.unwrap();
    let twin_page = d.render(&viewer, &twin).await.unwrap();
    assert_eq!(
        again.content_hash, page.content_hash,
        "same text, same content hash: a render is repeatable"
    );
    assert_eq!(
        twin_page.content_hash, page.content_hash,
        "same text, same content hash, whatever the path"
    );
    d.seed(twin.clone(), "Twin", "# Changed\n").await;
    assert_ne!(
        d.render(&viewer, &twin).await.unwrap().content_hash,
        page.content_hash,
        "a changed text has another content hash"
    );

    // Bounds: refused, never truncated; the outline is cut.
    let too_long = at("source-over-the-bound");
    d.seed(too_long.clone(), "Long", &"a".repeat(MAX_RENDER_SOURCE_BYTES + 1))
        .await;
    assert!(
        matches!(d.render(&viewer, &too_long).await, Err(DocError::BadRequest)),
        "a doc of more than MAX_RENDER_SOURCE_BYTES must be refused (BadRequest, which REST answers with 413), never truncated"
    );
    let too_wide = at("page-over-the-bound");
    d.seed(
        too_wide.clone(),
        "Wide",
        &"&".repeat(MAX_RENDERED_HTML_BYTES / 5 + 1),
    )
    .await;
    assert!(
        matches!(d.render(&viewer, &too_wide).await, Err(DocError::BadRequest)),
        "a doc whose page would pass MAX_RENDERED_HTML_BYTES must be refused (BadRequest, which REST answers with 413), never cut"
    );
    let many = at("many-headings");
    let mut text = String::new();
    for n in 0..=MAX_DOC_HEADINGS {
        writeln!(text, "# Heading {n}\n").unwrap();
    }
    d.seed(many.clone(), "Many", &text).await;
    let page = d.render(&viewer, &many).await.unwrap();
    assert_eq!(
        page.headings.len(),
        MAX_DOC_HEADINGS,
        "the outline is cut at the bound, and the page keeps every heading"
    );
    assert_eq!(page.headings[0].text.as_str(), "Heading 0");
    assert_eq!(
        page.headings[MAX_DOC_HEADINGS - 1].text.as_str(),
        format!("Heading {}", MAX_DOC_HEADINGS - 1),
        "the first headings, in document order"
    );
    let violations = rendered_doc_violations(&page);
    assert!(
        violations.is_empty(),
        "many-headings: the rendered page leaves the allowlist or disagrees with its outline: {violations:?}"
    );

    // The scan runs before the render.
    let planted = at("planted");
    if let Some(secret) = d.plant_secret(planted.clone()).await {
        let page = d.render(&viewer, &planted).await.unwrap();
        assert!(
            !page.html.contains(&secret) && !page.headings.iter().any(|h| h.text.as_str().contains(&secret)),
            "a flagged value reached the rendered page (S14b): the secret scan runs before the render"
        );
        let violations = rendered_doc_violations(&page);
        assert!(
            violations.is_empty(),
            "planted: the rendered page leaves the allowlist or disagrees with its outline: {violations:?}"
        );
    }

    // The corpus.
    for (name, source) in XSS_CORPUS {
        let path = DocPath::parse(&format!("/git/main/xss/{name}.md")).unwrap();
        d.seed(path.clone(), name, source).await;
        let page = match d.render(&viewer, &path).await {
            Ok(page) => page,
            Err(e) => panic!("{name}: a small doc must render, and the answer is {e:?}"),
        };
        let violations = rendered_doc_violations(&page);
        assert!(
            violations.is_empty(),
            "{name}: the rendered page leaves the allowlist or disagrees with its outline: {violations:?}"
        );
        if *name == "marker-text" {
            assert!(
                page.html.contains("[REDACTED: 12 chars, api_key]"),
                "the marker of a masked value is text in the page"
            );
        }
    }
}
