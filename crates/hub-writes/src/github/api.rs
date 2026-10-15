//! What the hub reads from GitHub's answers: pure functions from the bytes of a response to a typed value, with no I/O. They are
//! the parsers at the trust boundary of this leg, so they have a fuzz target (`writes_github_responses`, S22) and they refuse
//! rather than guess.
//!
//! GitHub's answers are untrusted data (rule 10): a body is read up to a cap, only the fields below are looked at, and a login
//! or a flag that is not exactly what was asked for is `Unexpected`, never a default.

use serde::Deserialize;

use super::types::{GithubLogin, parse_expiry};
use domain::Timestamp;

/// The name of the header that carries the expiry of a fine-grained token (`2026-11-20 12:00:00 UTC`).
pub const EXPIRY_HEADER: &str = "github-authentication-token-expiration";

/// The most bytes of a response body the client reads. `GET /user` and `GET /repos/{o}/{r}` are a few KiB.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

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

/// Is the first thing in `body`, after JSON whitespace, an object? serde reads a struct from an array too (`["octocat"]` would be a
/// login), and an answer or a request that is an array is not one this code asked for.
pub(crate) fn is_object(body: &[u8]) -> bool {
    body.iter().find(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n')) == Some(&b'{')
}

/// The body of an answer as text, if it is one this code reads: within the cap, valid UTF-8 throughout (`serde_json` does not check
/// the text of a field it skips, so `from_slice` alone would accept an answer with bytes in it that are not text), and an object.
fn object_text(body: &[u8]) -> Result<&str, GithubError> {
    if body.len() > MAX_BODY_BYTES || !is_object(body) {
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

/// The expiry of the token, from the header's value; `None` when GitHub sent none (a token that does not expire) or sent
/// something unreadable.
#[must_use]
pub fn token_expiry(header: Option<&[u8]>) -> Option<Timestamp> {
    header
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| parse_expiry(v.trim()))
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
        500..=599 => Some(GithubError::Unavailable),
        _ => Some(GithubError::Unexpected),
    }
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

    #[test]
    fn the_expiry_header_is_optional() {
        assert!(token_expiry(Some(b"2026-11-20 12:00:00 UTC")).is_some());
        assert!(token_expiry(Some(b" 2026-11-20 12:00:00 UTC ")).is_some());
        assert!(token_expiry(Some(b"soon")).is_none());
        assert!(token_expiry(Some(&[0xff, 0xfe])).is_none());
        assert!(token_expiry(None).is_none());
    }
}
