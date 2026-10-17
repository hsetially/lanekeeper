//! Typed, bounded parsing of the read API's query strings (T6, S4, S11, S12, P12; fuzzed by `registry_query`).
//!
//! Every operation has one function here that takes the raw query string and returns the typed request, or [`QueryError`], which is a
//! 400 and carries nothing of the input (the log has what is needed, the client gets the code). The rules are the same for all:
//!
//! - the raw query is at most [`MAX_QUERY_BYTES`] (8 KiB), and it must be a URI query that decodes to UTF-8;
//! - a name that is not one of the operation's parameters is refused, and so is a name given twice (an attacker's way of making two
//!   layers read different values);
//! - every value is parsed into its domain type, which is where length, alphabet and normalisation are enforced (paths are untrusted,
//!   rule 10), and a value that does not parse is refused whole, never repaired;
//! - a list is comma separated, has no empty and no repeated item and is at most its bound long (the contract's `maxItems` and
//!   `uniqueItems`);
//! - numbers are plain digits in their range, booleans are `true` or `false`, and a page is `limit` 1 to 500 (default 100) with an
//!   optional cursor of at most 512 characters of the cursor alphabet. Whether a cursor *means* anything is the service's decision
//!   (it is bound to its query, A31); here it is only well formed.
//!
//! Nothing here reads a header, touches the database or allocates in proportion to anything but the (bounded) query.

use std::collections::BTreeSet;
use std::str::FromStr;

use axum::extract::Query;
use domain::{
    AppName, ChannelName, CompareRef, ContentHash, ContentSource, Cursor, DriftState, FindingKind,
    FindingsQuery, GitRef, GridQuery, LogicalFile, NfsPath, Page, RepoKind, ServeRequest, SettingsQuery,
    Severity, ShortText, SwimlaneId, TenantId, TextQuery,
};
use http::Uri;
use hub_platform::ApiError;

/// The longest raw query string accepted. The longest legitimate one is a grid of 64 swimlanes (about 4 KiB) or a percent-encoded
/// 512-byte pattern (about 1.5 KiB).
pub const MAX_QUERY_BYTES: usize = 8 * 1024;

/// A query string that is not valid for its operation. It names nothing of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the query string is not valid")]
pub struct QueryError;

impl From<QueryError> for ApiError {
    fn from(_: QueryError) -> Self {
        ApiError::BadRequest
    }
}

type Result<T> = std::result::Result<T, QueryError>;

/// The parameters of one request: decoded, each name at most once and only from the operation's list.
#[derive(Debug)]
pub struct Params {
    items: Vec<(String, String)>,
}

impl Params {
    /// Decode `raw` (the part of the URI after `?`; `None` when there is none) against the names the operation accepts.
    ///
    /// # Errors
    ///
    /// [`QueryError`] for a query that is too long, does not decode, has a name outside `allowed` or has a name twice.
    pub fn new(raw: Option<&str>, allowed: &[&str]) -> Result<Self> {
        let raw = raw.unwrap_or("");
        if raw.len() > MAX_QUERY_BYTES {
            return Err(QueryError);
        }
        if raw.is_empty() {
            return Ok(Self { items: Vec::new() });
        }
        // The URI parser drops a fragment silently; a `#` in a query would be a different request than the one that was read.
        if raw.contains('#') {
            return Err(QueryError);
        }
        let uri: Uri = format!("/?{raw}").parse().map_err(|_| QueryError)?;
        let Query(items) = Query::<Vec<(String, String)>>::try_from_uri(&uri).map_err(|_| QueryError)?;
        // Duplicates and unknown names are refused, so there are never more pairs than names.
        if items.len() > allowed.len() {
            return Err(QueryError);
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (name, value) in &items {
            if !allowed.contains(&name.as_str()) || !seen.insert(name.as_str()) {
                return Err(QueryError);
            }
            // The decoder repairs bytes that are not UTF-8 by putting U+FFFD in their place. A value is refused whole, never repaired
            // (a path with a replacement character in it is not the path that was sent).
            if value.contains('\u{FFFD}') {
                return Err(QueryError);
            }
        }
        Ok(Self { items })
    }

    fn take(&mut self, name: &str) -> Option<String> {
        let at = self.items.iter().position(|(n, _)| n == name)?;
        Some(self.items.swap_remove(at).1)
    }

    fn present(&self, name: &str) -> bool {
        self.items.iter().any(|(n, _)| n == name)
    }

    /// A value of type `T`, absent or well formed.
    fn optional<T: FromStr>(&mut self, name: &str) -> Result<Option<T>> {
        self.take(name)
            .map(|v| v.parse::<T>().map_err(|_| QueryError))
            .transpose()
    }

    /// A value of type `T` that must be there.
    fn required<T: FromStr>(&mut self, name: &str) -> Result<T> {
        self.optional(name)?.ok_or(QueryError)
    }

    /// `true` or `false`, or `default` when absent.
    fn flag(&mut self, name: &str, default: bool) -> Result<bool> {
        match self.take(name).as_deref() {
            None => Ok(default),
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            Some(_) => Err(QueryError),
        }
    }

    /// A number in `min..=max`, written as digits only, or `default` when absent.
    fn number(&mut self, name: &str, min: u32, max: u32, default: u32) -> Result<u32> {
        let Some(text) = self.take(name) else {
            return Ok(default);
        };
        if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(QueryError);
        }
        let n: u32 = text.parse().map_err(|_| QueryError)?;
        if (min..=max).contains(&n) {
            Ok(n)
        } else {
            Err(QueryError)
        }
    }

    /// A comma separated list of `T`, at most `max` long, without empty or repeated items. Absent is empty.
    fn list<T: FromStr + Clone + Ord>(&mut self, name: &str, max: usize) -> Result<Vec<T>> {
        let Some(text) = self.take(name) else {
            return Ok(Vec::new());
        };
        let mut out: Vec<T> = Vec::new();
        let mut seen: BTreeSet<T> = BTreeSet::new();
        for item in text.split(',') {
            if out.len() >= max {
                return Err(QueryError);
            }
            let value: T = item.parse().map_err(|_| QueryError)?;
            if !seen.insert(value.clone()) {
                return Err(QueryError);
            }
            out.push(value);
        }
        Ok(out)
    }

    /// `limit` and `cursor`.
    fn page(&mut self) -> Result<Page> {
        let limit = self.number("limit", 1, Page::MAX_LIMIT, Page::DEFAULT_LIMIT)?;
        let cursor = self.optional::<CursorParam>("cursor")?.map(|c| c.0);
        Page::new(limit, cursor).map_err(|_| QueryError)
    }

    /// A folder to list: absent is the root, and a value is a real folder path (`.` and the empty string are not a way to say "root").
    fn prefix(&mut self) -> Result<NfsPath> {
        Ok(self.optional::<NfsPath>("prefix")?.unwrap_or_else(NfsPath::root))
    }
}

/// A cursor as a query value.
struct CursorParam(Cursor);

impl FromStr for CursorParam {
    type Err = QueryError;
    fn from_str(s: &str) -> Result<Self> {
        Cursor::parse(s).map(Self).map_err(|_| QueryError)
    }
}

// ------------------------------------------------------------------------------------------------ the operations

/// `listSwimlanes` and `listPendingRestarts`: a page.
///
/// # Errors
///
/// [`QueryError`] for anything but `limit` and `cursor`.
pub fn page_only(raw: Option<&str>) -> Result<Page> {
    let mut p = Params::new(raw, &["cursor", "limit"])?;
    p.page()
}

/// `getSwimlane`: no parameters.
///
/// # Errors
///
/// [`QueryError`] for any parameter at all.
pub fn none(raw: Option<&str>) -> Result<()> {
    Params::new(raw, &[]).map(|_| ())
}

/// `listSwimlaneTree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRequest {
    pub prefix: NfsPath,
    pub drift: Option<DriftState>,
    pub page: Page,
}

/// # Errors
///
/// [`QueryError`] for a parameter that is not `prefix`, `drift`, `cursor` or `limit`, or one that does not parse.
pub fn tree(raw: Option<&str>) -> Result<TreeRequest> {
    let mut p = Params::new(raw, &["prefix", "drift", "cursor", "limit"])?;
    Ok(TreeRequest {
        prefix: p.prefix()?,
        drift: p.optional("drift")?,
        page: p.page()?,
    })
}

/// `listDrift`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftRequest {
    pub state: Option<DriftState>,
    pub prefix: NfsPath,
    pub page: Page,
}

/// # Errors
///
/// [`QueryError`] for a parameter that is not `state`, `prefix`, `cursor` or `limit`, or one that does not parse.
pub fn drift(raw: Option<&str>) -> Result<DriftRequest> {
    let mut p = Params::new(raw, &["state", "prefix", "cursor", "limit"])?;
    Ok(DriftRequest {
        state: p.optional("state")?,
        prefix: p.prefix()?,
        page: p.page()?,
    })
}

/// `getFileContent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentRequest {
    pub path: NfsPath,
    pub source: ContentSource,
}

/// # Errors
///
/// [`QueryError`] for an unknown parameter or one that does not parse, a `source` that is not one of the four, and a parameter that
/// belongs to another source (`hash` is for `blob`, `repo` and `ref` for `git`, and each source needs its own).
pub fn content(raw: Option<&str>) -> Result<ContentRequest> {
    let mut p = Params::new(raw, &["path", "source", "hash", "repo", "ref"])?;
    let path: NfsPath = p.required("path")?;
    let source = match p.take("source").as_deref().unwrap_or("nfs") {
        "nfs" => ContentSource::Nfs,
        "baseline" => ContentSource::Baseline,
        "blob" => ContentSource::Blob {
            hash: p.required::<ContentHash>("hash")?,
        },
        "git" => ContentSource::Git {
            repo: p.required::<RepoKind>("repo")?,
            git_ref: p.required::<GitRef>("ref")?,
        },
        _ => return Err(QueryError),
    };
    // What is left belongs to another source.
    if p.present("hash") || p.present("repo") || p.present("ref") {
        return Err(QueryError);
    }
    Ok(ContentRequest { path, source })
}

/// `listFileHistory`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRequest {
    pub path: NfsPath,
    pub page: Page,
}

/// # Errors
///
/// [`QueryError`] for a parameter that is not `path`, `cursor` or `limit`, or one that does not parse.
pub fn history(raw: Option<&str>) -> Result<HistoryRequest> {
    let mut p = Params::new(raw, &["path", "cursor", "limit"])?;
    Ok(HistoryRequest {
        path: p.required("path")?,
        page: p.page()?,
    })
}

/// `compareFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareRequest {
    pub left: CompareRef,
    pub right: CompareRef,
    pub file: LogicalFile,
    pub channel: Option<ChannelName>,
}

/// # Errors
///
/// [`QueryError`] for a parameter that is not `left`, `right`, `file` or `channel`, or one that does not parse.
pub fn compare(raw: Option<&str>) -> Result<CompareRequest> {
    let mut p = Params::new(raw, &["left", "right", "file", "channel"])?;
    Ok(CompareRequest {
        left: p.required("left")?,
        right: p.required("right")?,
        file: p.required("file")?,
        channel: p.optional("channel")?,
    })
}

/// `compareTree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareTreeRequest {
    pub left: CompareRef,
    pub right: CompareRef,
    pub prefix: NfsPath,
    pub page: Page,
}

/// # Errors
///
/// [`QueryError`] for a parameter that is not `left`, `right`, `prefix`, `cursor` or `limit`, or one that does not parse.
pub fn compare_tree(raw: Option<&str>) -> Result<CompareTreeRequest> {
    let mut p = Params::new(raw, &["left", "right", "prefix", "cursor", "limit"])?;
    Ok(CompareTreeRequest {
        left: p.required("left")?,
        right: p.required("right")?,
        prefix: p.prefix()?,
        page: p.page()?,
    })
}

/// `getGrid`. Without `only_differing` every row is shown (the contract gives the parameter no default; the MCP tool's default is
/// `true`, a client that wants the same asks for it).
///
/// # Errors
///
/// [`QueryError`] for a parameter that is not `file`, `swimlanes`, `channel`, `only_differing`, `cursor` or `limit`, one that does
/// not parse, and a `swimlanes` list that is empty or longer than 64.
pub fn grid(raw: Option<&str>) -> Result<GridQuery> {
    let mut p = Params::new(
        raw,
        &[
            "file",
            "swimlanes",
            "channel",
            "only_differing",
            "cursor",
            "limit",
        ],
    )?;
    let file: LogicalFile = p.required("file")?;
    let swimlanes: Vec<SwimlaneId> = p.list("swimlanes", GridQuery::MAX_SWIMLANES)?;
    if swimlanes.is_empty() {
        return Err(QueryError);
    }
    Ok(GridQuery {
        file,
        swimlanes,
        channel: p.optional("channel")?,
        only_differing: p.flag("only_differing", false)?,
        page: p.page()?,
    })
}

/// An optional text that, when given, is not empty: `path_glob=` is a mistake, not "no glob".
fn non_empty(text: Option<ShortText>) -> Result<Option<ShortText>> {
    match text {
        Some(t) if t.as_str().is_empty() => Err(QueryError),
        other => Ok(other),
    }
}

/// The most swimlanes a search names.
pub const MAX_SEARCH_SWIMLANES: usize = 64;

/// `searchSettings`.
///
/// # Errors
///
/// [`QueryError`] for a parameter that is not `q`, `swimlanes`, `file`, `cursor` or `limit`, one that does not parse, and a `q` that
/// is empty or longer than 256 bytes.
pub fn search_settings(raw: Option<&str>) -> Result<SettingsQuery> {
    let mut p = Params::new(raw, &["q", "swimlanes", "file", "cursor", "limit"])?;
    let needle: ShortText = p.required("q")?;
    if needle.as_str().is_empty() {
        return Err(QueryError);
    }
    Ok(SettingsQuery {
        path_contains: needle,
        swimlanes: p.list("swimlanes", MAX_SEARCH_SWIMLANES)?,
        file: p.optional("file")?,
        page: p.page()?,
    })
}

/// `searchText`: a pattern of 1 to 512 bytes (no control characters but a tab), literal unless `regex=true`, case insensitive unless `case_sensitive=true`, at most
/// `max_total` (default 500, at most 500) and `max_per_file` (default 20, at most 100) hits. The service keeps those within its own
/// ceilings (A27).
///
/// # Errors
///
/// [`QueryError`] for a parameter outside the list, one that does not parse, and a pattern that is empty or longer than 512 bytes.
pub fn search_text(raw: Option<&str>) -> Result<TextQuery> {
    let mut p = Params::new(
        raw,
        &[
            "pattern",
            "regex",
            "case_sensitive",
            "swimlanes",
            "path_glob",
            "max_total",
            "max_per_file",
        ],
    )?;
    let pattern = p.take("pattern").ok_or(QueryError)?;
    // A tab is something to search for; no other control character is (a newline cannot be in a line-based search, a NUL is a binary
    // file's business), and refusing them here keeps them out of the engine and the logs.
    if pattern.is_empty()
        || pattern.len() > TextQuery::MAX_PATTERN_BYTES
        || pattern.chars().any(|c| c.is_control() && c != '\t')
    {
        return Err(QueryError);
    }
    Ok(TextQuery {
        pattern,
        regex: p.flag("regex", false)?,
        case_sensitive: p.flag("case_sensitive", false)?,
        swimlanes: p.list("swimlanes", MAX_SEARCH_SWIMLANES)?,
        path_glob: non_empty(p.optional::<ShortText>("path_glob")?)?,
        max_total: p.number("max_total", 1, 500, TextQuery::DEFAULT_MAX_TOTAL)?,
        max_per_file: p.number("max_per_file", 1, 100, TextQuery::DEFAULT_MAX_PER_FILE)?,
    })
}

/// The most kinds a findings filter names (the contract's `maxItems`).
pub const MAX_FINDING_KINDS: usize = 12;

/// `listFindings`.
///
/// # Errors
///
/// [`QueryError`] for a parameter that is not `swimlane`, `kinds`, `min_severity`, `cursor` or `limit`, and one that does not parse.
pub fn findings(raw: Option<&str>) -> Result<FindingsQuery> {
    let mut p = Params::new(raw, &["swimlane", "kinds", "min_severity", "cursor", "limit"])?;
    Ok(FindingsQuery {
        swimlane: p.optional("swimlane")?,
        kinds: p.list::<FindingKind>("kinds", MAX_FINDING_KINDS)?,
        min_severity: p.optional::<Severity>("min_severity")?,
        page: p.page()?,
    })
}

/// `getEffectiveConfig`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveRequest {
    pub file: LogicalFile,
    pub tenant: TenantId,
    pub channel: Option<ChannelName>,
}

/// # Errors
///
/// [`QueryError`] for a parameter that is not `file`, `tenant` or `channel`, and one that does not parse.
pub fn effective(raw: Option<&str>) -> Result<EffectiveRequest> {
    let mut p = Params::new(raw, &["file", "tenant", "channel"])?;
    Ok(EffectiveRequest {
        file: p.required("file")?,
        tenant: p.required("tenant")?,
        channel: p.optional("channel")?,
    })
}

/// `getServedFile`.
///
/// # Errors
///
/// [`QueryError`] for a parameter that is not `application`, `tenant`, `channel` or `file`, and one that does not parse.
pub fn served(raw: Option<&str>) -> Result<ServeRequest> {
    let mut p = Params::new(raw, &["application", "tenant", "channel", "file"])?;
    Ok(ServeRequest {
        application: p.required::<AppName>("application")?,
        tenant: p.required("tenant")?,
        channel: p.optional("channel")?,
        file: p.required("file")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long(n: usize) -> String {
        "a".repeat(n)
    }

    #[test]
    fn a_page_is_a_limit_of_one_to_five_hundred_and_a_well_formed_cursor() {
        assert_eq!(page_only(None).unwrap().limit(), 100);
        assert_eq!(page_only(Some("limit=1")).unwrap().limit(), 1);
        assert_eq!(page_only(Some("limit=500")).unwrap().limit(), 500);
        for bad in [
            "limit=0",
            "limit=501",
            "limit=-1",
            "limit=%2B5",
            "limit=",
            "limit=1.5",
            "limit=99999999999",
            "limit=abc",
            "limit=1&limit=2",
            "cursor=",
            "cursor=a%20b",
            "cursor=a%2Fb",
        ] {
            assert_eq!(page_only(Some(bad)), Err(QueryError), "{bad}");
        }
        let page = page_only(Some("cursor=abc_-=.&limit=7")).unwrap();
        assert_eq!(page.cursor().unwrap().as_str(), "abc_-=.");
        assert_eq!(page.limit(), 7);
        let too_long = format!("cursor={}", long(513));
        assert_eq!(page_only(Some(&too_long)), Err(QueryError));
        assert!(page_only(Some(&format!("cursor={}", long(512)))).is_ok());
    }

    #[test]
    fn an_unknown_or_repeated_name_is_refused_whatever_its_value() {
        assert_eq!(page_only(Some("limit=5&x=1")), Err(QueryError));
        assert_eq!(page_only(Some("x")), Err(QueryError));
        assert_eq!(page_only(Some("limit=5&limit=5")), Err(QueryError));
        assert_eq!(none(Some("limit=5")), Err(QueryError));
        assert_eq!(none(None), Ok(()));
        assert_eq!(none(Some("")), Ok(()));
    }

    #[test]
    fn the_whole_query_is_bounded_and_must_decode() {
        let big = format!("limit=5&{}", long(MAX_QUERY_BYTES));
        assert_eq!(page_only(Some(&big)), Err(QueryError));
        // `%FF` is not UTF-8, a space and a `#` are not in a query.
        assert_eq!(page_only(Some("cursor=%FF")), Err(QueryError));
        assert_eq!(
            tree(Some("prefix=%FF%FE")),
            Err(QueryError),
            "bytes that are not UTF-8 are refused, not repaired"
        );
        assert_eq!(
            tree(Some("prefix=a%EF%BF%BDb")),
            Err(QueryError),
            "a replacement character is refused too"
        );
        assert_eq!(page_only(Some("limit=5 ")), Err(QueryError));
        assert_eq!(page_only(Some("limit=5#x")), Err(QueryError));
    }

    #[test]
    fn the_tree_and_the_drift_list_take_a_folder_and_a_state() {
        let t = tree(Some("prefix=core%2Fconf&drift=git_ahead&limit=3")).unwrap();
        assert_eq!(t.prefix.as_str(), "core/conf");
        assert_eq!(t.drift, Some(DriftState::GitAhead));
        assert_eq!(t.page.limit(), 3);
        assert!(tree(None).unwrap().prefix.is_root(), "absent is the root");
        for bad in [
            "prefix=",
            "prefix=..%2Fetc",
            "prefix=%2Fabs",
            "prefix=a%00b",
            "prefix=a%5Cb",
            "drift=nope",
            "drift=",
            "state=git_ahead",
        ] {
            assert_eq!(tree(Some(bad)), Err(QueryError), "{bad}");
        }
        let d = drift(Some("state=git_ahead&prefix=core")).unwrap();
        assert_eq!(d.state, Some(DriftState::GitAhead));
        assert_eq!(d.prefix.as_str(), "core");
        assert_eq!(drift(Some("drift=git_ahead")), Err(QueryError));
    }

    #[test]
    fn a_content_source_takes_exactly_its_own_parameters() {
        let hash = "a".repeat(64);
        let c = content(Some("path=core.yml")).unwrap();
        assert_eq!(c.source, ContentSource::Nfs);
        assert_eq!(
            content(Some("path=core.yml&source=baseline")).unwrap().source,
            ContentSource::Baseline
        );
        let blob = content(Some(&format!("path=core.yml&source=blob&hash={hash}"))).unwrap();
        assert!(matches!(blob.source, ContentSource::Blob { .. }));
        let git = content(Some("path=core.yml&source=git&repo=tenant&ref=v1.2")).unwrap();
        assert!(matches!(
            git.source,
            ContentSource::Git {
                repo: RepoKind::Tenant,
                ..
            }
        ));
        for bad in [
            "source=nfs".to_owned(),
            "path=core.yml&source=other".to_owned(),
            "path=core.yml&source=blob".to_owned(),
            format!("path=core.yml&source=blob&hash={}", "z".repeat(64)),
            format!("path=core.yml&hash={hash}"),
            format!("path=core.yml&source=nfs&hash={hash}"),
            format!("path=core.yml&source=git&hash={hash}&repo=base&ref=main"),
            "path=core.yml&source=git&repo=base".to_owned(),
            "path=core.yml&source=git&ref=main".to_owned(),
            "path=core.yml&source=git&repo=other&ref=main".to_owned(),
            "path=core.yml&source=git&repo=base&ref=a..b".to_owned(),
            "path=core.yml&source=git&repo=base&ref=-x".to_owned(),
            "path=core.yml&repo=base".to_owned(),
            "path=core.yml&ref=main".to_owned(),
            "path=core.yml&source=nfs&source=nfs".to_owned(),
        ] {
            assert_eq!(content(Some(&bad)), Err(QueryError), "{bad}");
        }
    }

    #[test]
    fn a_compare_has_two_typed_sides_a_file_and_an_optional_channel() {
        let c = compare(Some(
            "left=nfs%3Asit1&right=git%3Abase%40v2.3.1&file=core.yml&channel=web",
        ))
        .unwrap();
        assert_eq!(c.left.to_string(), "nfs:sit1");
        assert_eq!(c.right.to_string(), "git:base@v2.3.1");
        assert_eq!(c.channel.unwrap().as_str(), "web");
        for bad in [
            "left=nfs%3Asit1&right=nfs%3Asit2",
            "left=nfs%3Asit1&file=core.yml",
            "left=nfs%3ASIT1&right=nfs%3Asit2&file=core.yml",
            "left=other%3Asit1&right=nfs%3Asit2&file=core.yml",
            "left=nfs%3Asit1&right=nfs%3Asit2&file=..%2Fcore.yml",
            "left=nfs%3Asit1&right=git%3Abase%40a..b&file=core.yml",
            "left=nfs%3Asit1&right=nfs%3Asit2&file=core.yml&channel=A%20B",
        ] {
            assert_eq!(compare(Some(bad)), Err(QueryError), "{bad}");
        }
        let c = compare_tree(Some("left=nfs%3Asit1&right=nfs%3Asit2")).unwrap();
        assert!(c.prefix.is_root());
        assert_eq!(c.page.limit(), 100);
        assert_eq!(compare_tree(Some("left=nfs%3Asit1")), Err(QueryError));
        assert_eq!(
            compare_tree(Some("left=nfs%3Asit1&right=nfs%3Asit2&file=a")),
            Err(QueryError),
            "`file` is the single-file compare's"
        );
    }

    #[test]
    fn a_grid_names_one_to_sixty_four_distinct_swimlanes() {
        let g = grid(Some(
            "file=core.yml&swimlanes=a1,b2,c3&only_differing=true&channel=web",
        ))
        .unwrap();
        assert_eq!(
            g.swimlanes.iter().map(SwimlaneId::as_str).collect::<Vec<_>>(),
            ["a1", "b2", "c3"],
            "the order asked for is the order of the columns"
        );
        assert!(g.only_differing);
        assert!(!grid(Some("file=core.yml&swimlanes=a1")).unwrap().only_differing);
        let names = |n: usize| (0..n).map(|i| format!("s{i}")).collect::<Vec<_>>().join(",");
        assert!(grid(Some(&format!("file=f&swimlanes={}", names(64)))).is_ok());
        for bad in [
            "file=f".to_owned(),
            "swimlanes=a1".to_owned(),
            "file=f&swimlanes=".to_owned(),
            "file=f&swimlanes=a1,,b2".to_owned(),
            "file=f&swimlanes=a1,a1".to_owned(),
            "file=f&swimlanes=a1,".to_owned(),
            "file=f&swimlanes=A1".to_owned(),
            format!("file=f&swimlanes={}", names(65)),
            "file=f&swimlanes=a1&only_differing=1".to_owned(),
            "file=f&swimlanes=a1&only_differing=True".to_owned(),
        ] {
            assert_eq!(grid(Some(&bad)), Err(QueryError), "{bad}");
        }
    }

    #[test]
    fn a_settings_search_has_a_needle_of_one_to_256_bytes() {
        let q = search_settings(Some("q=server.port&swimlanes=a1,b2&file=core.yml&limit=10")).unwrap();
        assert_eq!(q.path_contains.as_str(), "server.port");
        assert_eq!(q.swimlanes.len(), 2);
        assert_eq!(q.page.limit(), 10);
        assert!(search_settings(Some(&format!("q={}", long(256)))).is_ok());
        for bad in [
            String::new(),
            "q=".to_owned(),
            format!("q={}", long(257)),
            "q=a%00b".to_owned(),
            "q=a%0Ab".to_owned(),
            "q=a&swimlanes=a1,a1".to_owned(),
            format!(
                "q=a&swimlanes={}",
                (0..65).map(|i| format!("s{i}")).collect::<Vec<_>>().join(",")
            ),
            "q=a&file=..%2Fx".to_owned(),
            "q=a&pattern=b".to_owned(),
        ] {
            assert_eq!(search_settings(Some(&bad)), Err(QueryError), "{bad}");
        }
    }

    #[test]
    fn a_text_search_has_a_bounded_pattern_and_bounded_caps() {
        let q = search_text(Some("pattern=timeout")).unwrap();
        assert_eq!(
            (q.regex, q.case_sensitive, q.max_total, q.max_per_file),
            (false, false, 500, 20)
        );
        let q = search_text(Some(
            "pattern=a.%2B&regex=true&case_sensitive=true&swimlanes=a1&path_glob=%2A%2A%2F%2A.yml&max_total=7&max_per_file=3",
        ))
        .unwrap();
        assert!(q.regex && q.case_sensitive);
        assert_eq!((q.max_total, q.max_per_file), (7, 3));
        assert_eq!(q.path_glob.unwrap().as_str(), "**/*.yml");
        assert!(search_text(Some(&format!("pattern={}", long(512)))).is_ok());
        assert_eq!(
            search_text(Some("pattern=a%09b")).unwrap().pattern,
            "a\tb",
            "a tab can be searched for"
        );
        for bad in [
            "pattern=a&path_glob=".to_owned(),
            String::new(),
            "pattern=".to_owned(),
            format!("pattern={}", long(513)),
            "pattern=a&max_total=0".to_owned(),
            "pattern=a&max_total=501".to_owned(),
            "pattern=a&max_per_file=0".to_owned(),
            "pattern=a&max_per_file=101".to_owned(),
            "pattern=a&regex=yes".to_owned(),
            "pattern=a&path_glob=a%00".to_owned(),
            format!("pattern=a&path_glob={}", long(257)),
            "pattern=a&limit=5".to_owned(),
            "pattern=a&pattern=b".to_owned(),
            "pattern=a%00b".to_owned(),
            "pattern=a%0Ab".to_owned(),
            "pattern=a%1Bb".to_owned(),
            "pattern=a%7Fb".to_owned(),
        ] {
            assert_eq!(search_text(Some(&bad)), Err(QueryError), "{bad}");
        }
        // A multi-byte pattern is bounded in bytes, not characters.
        let wide = "é".repeat(257);
        assert_eq!(search_text(Some(&format!("pattern={wide}"))), Err(QueryError));
    }

    #[test]
    fn findings_filter_by_swimlane_kinds_and_severity() {
        let f = findings(Some("swimlane=a1&kinds=drift,orphan_file&min_severity=high")).unwrap();
        assert_eq!(f.swimlane.unwrap().as_str(), "a1");
        assert_eq!(f.kinds.len(), 2);
        assert_eq!(f.min_severity, Some(Severity::High));
        let all = FindingKind::ALL
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(",");
        assert!(findings(Some(&format!("kinds={all}"))).is_ok());
        for bad in [
            "kinds=nope",
            "kinds=drift,drift",
            "kinds=",
            "kinds=drift,",
            "min_severity=urgent",
            "swimlane=",
            "swimlane=A1",
        ] {
            assert_eq!(findings(Some(bad)), Err(QueryError), "{bad}");
        }
    }

    #[test]
    fn effective_and_served_need_their_own_required_parameters() {
        let e = effective(Some("file=core.yml&tenant=sit1&channel=web")).unwrap();
        assert_eq!(e.tenant.as_str(), "sit1");
        assert_eq!(
            effective(Some("file=core.yml")),
            Err(QueryError),
            "the tenant is required"
        );
        assert_eq!(effective(Some("tenant=sit1")), Err(QueryError));
        let s = served(Some("application=core&tenant=sit1&file=core.yml")).unwrap();
        assert_eq!(s.application.as_str(), "core");
        assert!(s.channel.is_none());
        for bad in [
            "tenant=sit1&file=core.yml",
            "application=core&file=core.yml",
            "application=core&tenant=sit1",
            "application=co%20re&tenant=sit1&file=core.yml",
            "application=core&tenant=sit1&file=%2Fetc%2Fpasswd",
            "application=core&tenant=sit1&file=core.yml&extra=1",
        ] {
            assert_eq!(served(Some(bad)), Err(QueryError), "{bad}");
        }
    }

    #[test]
    fn a_query_error_is_a_bad_request_and_says_nothing_of_the_input() {
        assert_eq!(ApiError::from(QueryError), ApiError::BadRequest);
        assert_eq!(QueryError.to_string(), "the query string is not valid");
    }
}
