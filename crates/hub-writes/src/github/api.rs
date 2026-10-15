//! What the hub reads from GitHub's answers: pure functions from the bytes of a response to a typed value, with no I/O. They are
//! the parsers at the trust boundary of this leg, so they have a fuzz target (`writes_github_responses`, S22) and they refuse
//! rather than guess.
//!
//! GitHub's answers are untrusted data (rule 10): a body is read up to a cap, only the fields below are looked at, and a login
//! or a flag that is not exactly what was asked for is `Unexpected`, never a default.

use base64::Engine as _;
use bytes::Bytes;
use serde::Deserialize;

use super::types::{BranchName, GitSha, GithubLogin, parse_expiry};
use domain::Timestamp;

/// The name of the header that carries the expiry of a fine-grained token (`2026-11-20 12:00:00 UTC`).
pub const EXPIRY_HEADER: &str = "github-authentication-token-expiration";

/// The most bytes of a response body the client reads. `GET /user` and `GET /repos/{o}/{r}` are a few KiB.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// The most bytes of a response body that carries file content (T6): a file of up to 3 MiB as base64 with its line breaks, in JSON.
pub const MAX_CONTENT_BODY_BYTES: usize = 5 * 1024 * 1024;

/// The highest pull request number kept (`pr_links.pr_number` is a 32-bit integer).
pub const MAX_PULL_NUMBER: u32 = i32::MAX as u32;

/// The longest address of a pull request kept (`PrJob.pr_url`, `maxLength: 512`).
pub const MAX_PULL_URL_LEN: usize = 512;

/// Why a call to GitHub gave nothing usable. A fixed word each: no URL, no header, no body and no token (S21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GithubError {
    /// GitHub does not accept the token (401): it is wrong, expired or revoked.
    #[error("github does not accept the token")]
    Unauthorized,
    /// GitHub refuses what was asked (403 that is not a rate limit).
    #[error("github refuses the request")]
    Forbidden,
    /// GitHub does not know the resource (404), which for a fine-grained token also means it cannot see it.
    #[error("github does not know the resource")]
    NotFound,
    /// The rate limit, primary or secondary, and the wait is longer than this call will wait.
    #[error("github is rate limiting this client")]
    RateLimited,
    /// GitHub understood the request and will not do it: the data is not acceptable or conflicts with what is there (422, 409),
    /// for example a branch that already exists.
    #[error("github refused the data")]
    Unprocessable,
    /// A network failure, a timeout or a 5xx, after the bounded retries.
    #[error("github is unavailable")]
    Unavailable,
    /// An answer that is not what the API documents: another status, a body that does not read, or one over the cap.
    #[error("github answered something unexpected")]
    Unexpected,
    /// The client is not set up to talk to GitHub (the address is not an allowed one).
    #[error("the github client is not configured")]
    Config,
}

impl GithubError {
    /// Whether another try may help: GitHub or the network failed, not the request.
    #[must_use]
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

/// Is the first thing in `body`, after JSON whitespace, an array?
pub(crate) fn is_array(body: &[u8]) -> bool {
    body.iter().find(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n')) == Some(&b'[')
}

/// Is the first thing in `body`, after JSON whitespace, an object? serde reads a struct from an array too (`["octocat"]` would be a
/// login), and an answer or a request that is an array is not one this code asked for.
pub(crate) fn is_object(body: &[u8]) -> bool {
    body.iter().find(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n')) == Some(&b'{')
}

/// The body of an answer as text, if it is one this code reads: within the cap, valid UTF-8 throughout (`serde_json` does not check
/// the text of a field it skips, so `from_slice` alone would accept an answer with bytes in it that are not text), and an object.
fn object_text(body: &[u8]) -> Result<&str, GithubError> {
    object_text_within(body, MAX_BODY_BYTES)
}

fn object_text_within(body: &[u8], cap: usize) -> Result<&str, GithubError> {
    if body.len() > cap || !is_object(body) {
        return Err(GithubError::Unexpected);
    }
    std::str::from_utf8(body).map_err(|_| GithubError::Unexpected)
}

#[derive(Deserialize)]
struct UserBody {
    login: String,
}

/// The login of `GET /user`.
pub fn parse_user(body: &[u8]) -> Result<GithubLogin, GithubError> {
    let user: UserBody = serde_json::from_str(object_text(body)?).map_err(|_| GithubError::Unexpected)?;
    GithubLogin::parse(&user.login).map_err(|_| GithubError::Unexpected)
}

#[derive(Deserialize)]
struct RepoBody {
    #[serde(default)]
    permissions: Option<Permissions>,
}

#[derive(Deserialize)]
struct Permissions {
    #[serde(default)]
    push: Option<bool>,
}

/// Whether `GET /repos/{o}/{r}` says the caller can push. A body without the flag is "cannot": the answer fails closed.
pub fn parse_push_permission(body: &[u8]) -> Result<bool, GithubError> {
    let repo: RepoBody = serde_json::from_str(object_text(body)?).map_err(|_| GithubError::Unexpected)?;
    Ok(repo.permissions.and_then(|p| p.push).unwrap_or(false))
}

/// The expiry of the token, from the header's value: `Ok(None)` only when GitHub sent no header (a token that does not expire).
/// A header that is there and cannot be read (not text, not `YYYY-MM-DD HH:MM:SS UTC`, a date that is not one) is
/// [`GithubError::Unexpected`], not `None`: reading it as "never expires" would leave a token that does expire without the guard
/// that stops a job from using it after it has.
///
/// # Errors
///
/// [`GithubError::Unexpected`] for a header whose value is unreadable.
pub fn token_expiry(header: Option<&[u8]>) -> Result<Option<Timestamp>, GithubError> {
    let Some(raw) = header else {
        return Ok(None);
    };
    std::str::from_utf8(raw)
        .ok()
        .and_then(|v| parse_expiry(v.trim()))
        .map(Some)
        .ok_or(GithubError::Unexpected)
}

/// The meaning of an HTTP status for these calls, before the body is looked at.
#[must_use]
pub fn status_error(status: u16, rate_limited: bool) -> Option<GithubError> {
    match status {
        200 => None,
        401 => Some(GithubError::Unauthorized),
        403 | 429 if rate_limited || status == 429 => Some(GithubError::RateLimited),
        403 => Some(GithubError::Forbidden),
        404 => Some(GithubError::NotFound),
        409 | 422 => Some(GithubError::Unprocessable),
        500..=599 => Some(GithubError::Unavailable),
        _ => Some(GithubError::Unexpected),
    }
}

/// [`status_error`] for a call whose success is one of `ok` (a create answers 201).
#[must_use]
pub fn status_error_for(status: u16, rate_limited: bool, ok: &[u16]) -> Option<GithubError> {
    if ok.contains(&status) {
        None
    } else {
        status_error(status, rate_limited).or(Some(GithubError::Unexpected))
    }
}

// ------------------------------------------------------------------------------------------------------ the Git Data API (T6)

#[derive(Deserialize)]
struct ObjectBody {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct RefBody {
    object: ObjectBody,
}

/// The commit a ref points at, from `GET /git/ref/heads/{branch}` or the answer of creating one. A ref that points at anything
/// but a commit (an annotated tag) is not a branch head.
pub fn parse_ref_commit(body: &[u8]) -> Result<GitSha, GithubError> {
    let r: RefBody = serde_json::from_str(object_text(body)?).map_err(|_| GithubError::Unexpected)?;
    if r.object.kind != "commit" {
        return Err(GithubError::Unexpected);
    }
    GitSha::parse(&r.object.sha).map_err(|_| GithubError::Unexpected)
}

#[derive(Deserialize)]
struct ShaBody {
    sha: String,
}

#[derive(Deserialize)]
struct CommitBody {
    tree: ShaBody,
}

/// The tree of a commit, from `GET /git/commits/{sha}`.
pub fn parse_commit_tree(body: &[u8]) -> Result<GitSha, GithubError> {
    let c: CommitBody = serde_json::from_str(object_text(body)?).map_err(|_| GithubError::Unexpected)?;
    GitSha::parse(&c.tree.sha).map_err(|_| GithubError::Unexpected)
}

/// The id of the blob, tree or commit that a create call made.
pub fn parse_created_sha(body: &[u8]) -> Result<GitSha, GithubError> {
    let c: ShaBody = serde_json::from_str(object_text(body)?).map_err(|_| GithubError::Unexpected)?;
    GitSha::parse(&c.sha).map_err(|_| GithubError::Unexpected)
}

/// The bytes of base64 text with the line breaks GitHub puts in it. Nothing else is skipped: a space or any other character that is
/// not base64 is an error, and so is a length that does not match `size` when GitHub gave one.
fn decode_content(text: &str, size: Option<u64>) -> Result<Bytes, GithubError> {
    let clean: String = text.chars().filter(|c| *c != '\n').collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(clean.as_bytes())
        .map_err(|_| GithubError::Unexpected)?;
    if size.is_some_and(|s| s != bytes.len() as u64) {
        return Err(GithubError::Unexpected);
    }
    Ok(Bytes::from(bytes))
}

#[derive(Deserialize)]
struct BlobBody {
    content: String,
    encoding: String,
    #[serde(default)]
    size: Option<u64>,
}

/// The bytes of `GET /git/blobs/{sha}`. Only base64 is read, and the decoded length must be the size GitHub states.
pub fn parse_blob(body: &[u8]) -> Result<Bytes, GithubError> {
    let b: BlobBody = serde_json::from_str(object_text_within(body, MAX_CONTENT_BODY_BYTES)?)
        .map_err(|_| GithubError::Unexpected)?;
    if b.encoding != "base64" {
        return Err(GithubError::Unexpected);
    }
    decode_content(&b.content, b.size)
}

/// A file as `GET /contents/{path}` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contents {
    /// The blob the file is.
    pub blob: GitSha,
    /// The bytes, when the answer carries them. GitHub leaves them out for a file over 1 MB (`encoding: "none"`): the blob is
    /// then fetched by its id.
    pub bytes: Option<Bytes>,
}

#[derive(Deserialize)]
struct ContentsBody {
    #[serde(rename = "type")]
    kind: String,
    sha: String,
    #[serde(default)]
    encoding: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

/// A regular file from `GET /contents/{path}`. A directory (an array), a symbolic link and a submodule are not files this code
/// puts a change into, so they are not read.
pub fn parse_contents(body: &[u8]) -> Result<Contents, GithubError> {
    let c: ContentsBody = serde_json::from_str(object_text_within(body, MAX_CONTENT_BODY_BYTES)?)
        .map_err(|_| GithubError::Unexpected)?;
    if c.kind != "file" {
        return Err(GithubError::Unexpected);
    }
    let blob = GitSha::parse(&c.sha).map_err(|_| GithubError::Unexpected)?;
    let bytes = match (c.encoding.as_deref(), c.content) {
        (Some("base64"), Some(text)) => Some(decode_content(&text, c.size)?),
        (Some("none"), _) => None,
        _ => return Err(GithubError::Unexpected),
    };
    Ok(Contents { blob, bytes })
}

/// A pull request, as far as this code needs to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullInfo {
    pub number: u32,
    /// The address of the page of the pull request, `https`, within [`MAX_PULL_URL_LEN`].
    pub url: String,
}

#[derive(Deserialize)]
struct PullBody {
    number: u64,
    html_url: String,
}

fn pull_info(p: PullBody) -> Result<PullInfo, GithubError> {
    let number = u32::try_from(p.number)
        .ok()
        .filter(|n| (1..=MAX_PULL_NUMBER).contains(n))
        .ok_or(GithubError::Unexpected)?;
    let url_ok = p.html_url.starts_with("https://")
        && p.html_url.len() <= MAX_PULL_URL_LEN
        && !p.html_url.chars().any(|c| c.is_control() || c.is_whitespace());
    if !url_ok {
        return Err(GithubError::Unexpected);
    }
    Ok(PullInfo {
        number,
        url: p.html_url,
    })
}

/// The pull request that `POST /pulls`, `PATCH /pulls/{n}` answer with.
pub fn parse_pull(body: &[u8]) -> Result<PullInfo, GithubError> {
    let p: PullBody = serde_json::from_str(object_text(body)?).map_err(|_| GithubError::Unexpected)?;
    pull_info(p)
}

#[derive(Deserialize)]
struct PullHead {
    #[serde(rename = "ref")]
    name: String,
}

#[derive(Deserialize)]
struct ListedPull {
    number: u64,
    html_url: String,
    head: PullHead,
}

/// The pull request of `GET /pulls?head=owner:branch` whose head is `branch`: the answer is an array, and only an entry that
/// names the branch asked for counts.
pub fn parse_pull_list(body: &[u8], branch: &BranchName) -> Result<Option<PullInfo>, GithubError> {
    if body.len() > MAX_BODY_BYTES || !is_array(body) {
        return Err(GithubError::Unexpected);
    }
    let text = std::str::from_utf8(body).map_err(|_| GithubError::Unexpected)?;
    let all: Vec<ListedPull> = serde_json::from_str(text).map_err(|_| GithubError::Unexpected)?;
    all.into_iter()
        .find(|p| p.head.name == branch.as_str())
        .map(|p| {
            pull_info(PullBody {
                number: p.number,
                html_url: p.html_url,
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_is_read_and_checked() {
        assert_eq!(
            parse_user(br#"{"login":"octocat","id":1,"plan":{"name":"x"}}"#)
                .unwrap()
                .as_str(),
            "octocat"
        );
        for bad in [
            &br#"{"login":""}"#[..],
            br#"{"login":"a b"}"#,
            br#"{"login":"-a"}"#,
            br#"{"login":1}"#,
            br#"{"id":1}"#,
            b"[]",
            // Bytes that are not text, inside a field that is skipped.
            b"{\"id\":1,\"login\":\"octocat\",\"plan\":{\"name\":\"fr\x9b\xa2\"}}",
            // An array is read as a struct by serde; it is not an answer of this API.
            br#"["octocat"]"#,
            br#""octocat""#,
            b"",
            b"{",
            b"not json",
        ] {
            assert_eq!(
                parse_user(bad).unwrap_err(),
                GithubError::Unexpected,
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn a_body_over_the_cap_is_refused_unread() {
        let mut big = br#"{"login":"octocat","pad":""#.to_vec();
        big.extend(std::iter::repeat_n(b'a', MAX_BODY_BYTES));
        big.extend_from_slice(b"\"}");
        assert_eq!(parse_user(&big).unwrap_err(), GithubError::Unexpected);
        assert_eq!(parse_push_permission(&big).unwrap_err(), GithubError::Unexpected);
    }

    #[test]
    fn push_permission_fails_closed() {
        assert_eq!(
            parse_push_permission(br#"{"permissions":{"push":true,"pull":true}}"#),
            Ok(true)
        );
        assert_eq!(
            parse_push_permission(br#"{"permissions":{"push":false}}"#),
            Ok(false)
        );
        assert_eq!(parse_push_permission(br#"{"permissions":{}}"#), Ok(false));
        assert_eq!(parse_push_permission(br#"{"permissions":null}"#), Ok(false));
        assert_eq!(parse_push_permission(br#"{"name":"x"}"#), Ok(false));
        // A flag that is not a boolean is not a yes.
        assert_eq!(
            parse_push_permission(br#"{"permissions":{"push":"true"}}"#),
            Err(GithubError::Unexpected)
        );
        assert_eq!(
            parse_push_permission(br#"{"permissions":{"push":1}}"#),
            Err(GithubError::Unexpected)
        );
        assert_eq!(parse_push_permission(b"nope"), Err(GithubError::Unexpected));
        // Bytes that are not text anywhere in the answer make it not an answer, whatever field they are in.
        assert_eq!(
            parse_push_permission(b"{\"permissions\":{\"push\":true},\"name\":\"\x9b\"}"),
            Err(GithubError::Unexpected)
        );
        // Only an object is an answer; an array is read as a struct by serde and would pass for an empty one.
        assert_eq!(parse_push_permission(b"[]"), Err(GithubError::Unexpected));
        assert_eq!(
            parse_push_permission(br#"[{"push":true}]"#),
            Err(GithubError::Unexpected)
        );
        assert_eq!(
            parse_push_permission(b" \n{\"permissions\":{\"push\":true}}"),
            Ok(true)
        );
    }

    #[test]
    fn statuses_map_to_fixed_words() {
        assert_eq!(status_error(200, false), None);
        assert_eq!(status_error(401, false), Some(GithubError::Unauthorized));
        assert_eq!(status_error(403, false), Some(GithubError::Forbidden));
        assert_eq!(status_error(403, true), Some(GithubError::RateLimited));
        assert_eq!(status_error(429, false), Some(GithubError::RateLimited));
        assert_eq!(status_error(404, false), Some(GithubError::NotFound));
        assert_eq!(status_error(500, false), Some(GithubError::Unavailable));
        assert_eq!(status_error(503, false), Some(GithubError::Unavailable));
        assert_eq!(status_error(204, false), Some(GithubError::Unexpected));
        assert_eq!(status_error(302, false), Some(GithubError::Unexpected));
        assert!(GithubError::Unavailable.is_transient() && !GithubError::Unauthorized.is_transient());
    }

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn a_branch_head_is_a_commit() {
        let ok =
            format!(r#"{{"ref":"refs/heads/x","object":{{"sha":"{SHA_A}","type":"commit","url":"u"}}}}"#);
        assert_eq!(parse_ref_commit(ok.as_bytes()).unwrap().as_str(), SHA_A);
        for bad in [
            format!(r#"{{"object":{{"sha":"{SHA_A}","type":"tag"}}}}"#),
            format!(r#"{{"object":{{"sha":"{SHA_A}"}}}}"#),
            r#"{"object":{"sha":"short","type":"commit"}}"#.to_owned(),
            r#"{"object":null}"#.to_owned(),
            format!(r#"[{{"object":{{"sha":"{SHA_A}","type":"commit"}}}}]"#),
            String::new(),
        ] {
            assert_eq!(
                parse_ref_commit(bad.as_bytes()),
                Err(GithubError::Unexpected),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_commit_gives_its_tree_and_a_create_gives_the_new_id() {
        let c = format!(r#"{{"sha":"{SHA_A}","tree":{{"sha":"{SHA_B}"}},"parents":[]}}"#);
        assert_eq!(parse_commit_tree(c.as_bytes()).unwrap().as_str(), SHA_B);
        assert_eq!(parse_created_sha(c.as_bytes()).unwrap().as_str(), SHA_A);
        assert_eq!(
            parse_commit_tree(br#"{"tree":{"sha":"x"}}"#),
            Err(GithubError::Unexpected)
        );
        assert_eq!(
            parse_created_sha(br#"{"sha":"../etc"}"#),
            Err(GithubError::Unexpected)
        );
        assert_eq!(parse_created_sha(br#"{"sha":1}"#), Err(GithubError::Unexpected));
    }

    #[test]
    fn a_blob_is_decoded_byte_for_byte_with_github_line_breaks() {
        let content = b"a: 1\r\nb: \xEF\xBB\xBF\x00\xff\r\n".repeat(20);
        let mut wrapped = String::new();
        for c in b64(&content).as_bytes().chunks(60) {
            wrapped.push_str(std::str::from_utf8(c).unwrap());
            wrapped.push('\n');
        }
        let body = serde_json::json!({ "sha": SHA_A, "size": content.len(), "encoding": "base64", "content": wrapped });
        assert_eq!(
            &parse_blob(body.to_string().as_bytes()).unwrap()[..],
            &content[..]
        );
    }

    #[test]
    fn a_blob_that_is_not_what_it_says_is_refused() {
        let ok = b64(b"abc");
        for body in [
            serde_json::json!({ "size": 3, "encoding": "utf-8", "content": "abc" }),
            serde_json::json!({ "size": 4, "encoding": "base64", "content": ok }),
            serde_json::json!({ "size": 3, "encoding": "base64", "content": "not base64 !!" }),
            serde_json::json!({ "size": 3, "encoding": "base64", "content": format!(" {ok}") }),
            serde_json::json!({ "encoding": "base64" }),
            serde_json::json!({ "size": 3, "content": ok }),
        ] {
            assert_eq!(
                parse_blob(body.to_string().as_bytes()),
                Err(GithubError::Unexpected),
                "{body}"
            );
        }
        // The size is optional, and checked when it is there.
        let body = serde_json::json!({ "encoding": "base64", "content": ok });
        assert_eq!(&parse_blob(body.to_string().as_bytes()).unwrap()[..], b"abc");
        // The bigger cap applies to content, and no further.
        let big = serde_json::json!({ "encoding": "base64", "content": "A".repeat(MAX_CONTENT_BODY_BYTES) });
        assert_eq!(
            parse_blob(big.to_string().as_bytes()),
            Err(GithubError::Unexpected)
        );
    }

    #[test]
    fn contents_are_a_regular_file_with_or_without_bytes() {
        let with = serde_json::json!({ "type": "file", "encoding": "base64", "size": 3, "sha": SHA_A, "content": format!("{}\n", b64(b"abc")) });
        let got = parse_contents(with.to_string().as_bytes()).unwrap();
        assert_eq!(
            (got.blob.as_str(), got.bytes.as_deref()),
            (SHA_A, Some(&b"abc"[..]))
        );
        // An empty file has an empty content.
        let empty = serde_json::json!({ "type": "file", "encoding": "base64", "size": 0, "sha": SHA_A, "content": "" });
        assert_eq!(
            parse_contents(empty.to_string().as_bytes())
                .unwrap()
                .bytes
                .as_deref(),
            Some(&b""[..])
        );
        // A file over 1 MB comes without bytes.
        let none = serde_json::json!({ "type": "file", "encoding": "none", "size": 2_000_000, "sha": SHA_A, "content": "" });
        assert_eq!(parse_contents(none.to_string().as_bytes()).unwrap().bytes, None);
        for kind in ["dir", "symlink", "submodule"] {
            let body = serde_json::json!({ "type": kind, "encoding": "base64", "sha": SHA_A, "content": "" });
            assert_eq!(
                parse_contents(body.to_string().as_bytes()),
                Err(GithubError::Unexpected),
                "{kind}"
            );
        }
        // A directory is an array.
        assert_eq!(
            parse_contents(format!(r#"[{{"type":"file","sha":"{SHA_A}"}}]"#).as_bytes()),
            Err(GithubError::Unexpected)
        );
        let lying = serde_json::json!({ "type": "file", "encoding": "base64", "size": 9, "sha": SHA_A, "content": b64(b"abc") });
        assert_eq!(
            parse_contents(lying.to_string().as_bytes()),
            Err(GithubError::Unexpected)
        );
    }

    #[test]
    fn a_pull_request_has_a_number_and_an_https_address() {
        let ok = br#"{"number":17,"html_url":"https://github.com/acme/r/pull/17","state":"open"}"#;
        let got = parse_pull(ok).unwrap();
        assert_eq!(
            (got.number, got.url.as_str()),
            (17, "https://github.com/acme/r/pull/17")
        );
        for bad in [
            &br#"{"number":0,"html_url":"https://github.com/x"}"#[..],
            br#"{"number":-1,"html_url":"https://github.com/x"}"#,
            br#"{"number":2147483648,"html_url":"https://github.com/x"}"#,
            br#"{"number":1,"html_url":"http://github.com/x"}"#,
            br#"{"number":1,"html_url":"javascript:alert(1)"}"#,
            br#"{"number":1,"html_url":"https://github.com/a b"}"#,
            b"{\"number\":1,\"html_url\":\"https://github.com/a\\nb\"}",
            br#"{"number":1}"#,
        ] {
            assert_eq!(
                parse_pull(bad),
                Err(GithubError::Unexpected),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        let long = format!(
            r#"{{"number":1,"html_url":"https://github.com/{}"}}"#,
            "a".repeat(MAX_PULL_URL_LEN)
        );
        assert_eq!(parse_pull(long.as_bytes()), Err(GithubError::Unexpected));
    }

    #[test]
    fn a_list_of_pull_requests_is_searched_for_the_branch() {
        let branch = BranchName::parse("lanekeeper/sit1/abc").unwrap();
        let list = serde_json::json!([
            { "number": 3, "html_url": "https://github.com/a/r/pull/3", "head": { "ref": "other" } },
            { "number": 4, "html_url": "https://github.com/a/r/pull/4", "head": { "ref": "lanekeeper/sit1/abc" } },
        ]);
        assert_eq!(
            parse_pull_list(list.to_string().as_bytes(), &branch)
                .unwrap()
                .unwrap()
                .number,
            4
        );
        assert_eq!(parse_pull_list(b"[]", &branch), Ok(None));
        let other = serde_json::json!([{ "number": 3, "html_url": "https://github.com/a/r/pull/3", "head": { "ref": "other" } }]);
        assert_eq!(parse_pull_list(other.to_string().as_bytes(), &branch), Ok(None));
        for bad in [&b"{}"[..], b"not json", br#"[{"number":1}]"#, br"[1]"] {
            assert_eq!(parse_pull_list(bad, &branch), Err(GithubError::Unexpected));
        }
    }

    #[test]
    fn create_statuses_are_successes_and_the_rest_keep_their_words() {
        assert_eq!(status_error_for(201, false, &[201]), None);
        assert_eq!(
            status_error_for(200, false, &[201]),
            Some(GithubError::Unexpected)
        );
        assert_eq!(
            status_error_for(422, false, &[201]),
            Some(GithubError::Unprocessable)
        );
        assert_eq!(
            status_error_for(409, false, &[201]),
            Some(GithubError::Unprocessable)
        );
        assert_eq!(
            status_error_for(403, true, &[201]),
            Some(GithubError::RateLimited)
        );
        assert_eq!(
            status_error_for(502, false, &[201]),
            Some(GithubError::Unavailable)
        );
        assert_eq!(status_error_for(200, false, &[200, 201]), None);
    }

    #[test]
    fn the_expiry_header_is_optional_and_never_misread() {
        assert!(matches!(
            token_expiry(Some(b"2026-11-20 12:00:00 UTC")),
            Ok(Some(_))
        ));
        assert!(matches!(
            token_expiry(Some(b" 2026-11-20 12:00:00 UTC ")),
            Ok(Some(_))
        ));
        // No header is a token that does not expire; a header that cannot be read is not.
        assert_eq!(token_expiry(None), Ok(None));
        for bad in [
            &b"soon"[..],
            &[0xff, 0xfe],
            b"",
            b"   ",
            b"2026-11-20 12:00:00 -0500",
            b"2026-02-30 12:00:00 UTC",
        ] {
            assert_eq!(token_expiry(Some(bad)), Err(GithubError::Unexpected), "{bad:?}");
        }
    }
}
