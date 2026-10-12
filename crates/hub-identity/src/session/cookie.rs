//! Cookies and the random values they carry (T3; S2, S3).
//!
//! - [`RandomToken`] is 256 random bits. The session id and the CSRF token are two of them. The cookie holds the base64url
//!   text; the database holds only the SHA-256 of the raw bytes ([`TokenHash`]), so a copy of the table is no session.
//! - [`RequestCookies::parse`] reads the `Cookie` header strictly: a size cap, cookie-octet values only, and a duplicate of
//!   one of our cookies is an error, not "the first one wins". Anything it refuses is treated as no cookie at all.
//! - The `Set-Cookie` builders write the exact attribute set S2 asks for: `__Host-` prefix, `Secure`, `Path=/`, no `Domain`,
//!   `SameSite=Lax`, and `HttpOnly` on everything the page's script has no need to read.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::Secret;
use sha2::{Digest, Sha256};

use crate::oidc::login::LOGIN_COOKIE_NAME;

/// The session cookie (S2): `HttpOnly`, `Secure`, `SameSite=Lax`, no `Domain`.
pub const SESSION_COOKIE: &str = "__Host-lk_session";
/// The CSRF cookie (S3): the same value the page sends back in [`CSRF_HEADER`]. Readable by script, by design.
pub const CSRF_COOKIE: &str = "__Host-lk_csrf";
/// The request header that carries the double-submit token.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// The longest `Cookie` header read, in bytes, summed over all `Cookie` headers of a request.
pub const MAX_COOKIE_HEADER_BYTES: usize = 8 * 1024;
/// The longest single cookie value read.
pub const MAX_COOKIE_VALUE_BYTES: usize = 2048;
/// At most this many `name=value` pairs are read from one request.
pub const MAX_COOKIE_PAIRS: usize = 64;

/// Why a `Cookie` header was refused. The edge treats every one of them as "no cookies".
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CookieError {
    #[error("cookie header is too large")]
    Oversize,
    #[error("cookie header is malformed")]
    Malformed,
    /// One of our cookies is present twice. Which one the browser meant is unknowable, so none is believed.
    #[error("a cookie is present twice")]
    Duplicate,
    #[error("too many cookies")]
    TooMany,
}

/// The cookies of a request that this crate cares about, as text. Nothing is decoded or trusted yet.
#[derive(Default)]
pub struct RequestCookies {
    pub session: Option<String>,
    pub csrf: Option<String>,
    pub login: Option<String>,
}

impl std::fmt::Debug for RequestCookies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestCookies").finish_non_exhaustive()
    }
}

impl RequestCookies {
    /// Read the values of every `Cookie` header of a request.
    ///
    /// Pairs are separated by `;` and optional spaces. A name is an HTTP token. A value is made of cookie octets
    /// (RFC 6265: printable ASCII without space, quote, comma, semicolon or backslash); a quoted value is refused. Only the
    /// cookies of this crate are kept, and each may appear once.
    pub fn parse<'a>(headers: impl IntoIterator<Item = &'a str>) -> Result<Self, CookieError> {
        let mut out = Self::default();
        let (mut total, mut pairs) = (0usize, 0usize);
        for header in headers {
            total = total.saturating_add(header.len());
            if total > MAX_COOKIE_HEADER_BYTES {
                return Err(CookieError::Oversize);
            }
            let mut segments = header.split(';').peekable();
            while let Some(pair) = segments.next() {
                let pair = pair.trim_start_matches(' ');
                if pair.is_empty() {
                    // Only a trailing separator may be followed by nothing; `a;;b` and a leading `;` are malformed.
                    if segments.peek().is_none() {
                        continue;
                    }
                    return Err(CookieError::Malformed);
                }
                pairs += 1;
                if pairs > MAX_COOKIE_PAIRS {
                    return Err(CookieError::TooMany);
                }
                let (name, value) = pair.split_once('=').ok_or(CookieError::Malformed)?;
                if !is_token(name) || !value.bytes().all(is_cookie_octet) {
                    return Err(CookieError::Malformed);
                }
                if value.len() > MAX_COOKIE_VALUE_BYTES {
                    return Err(CookieError::Oversize);
                }
                let slot = match name {
                    SESSION_COOKIE => &mut out.session,
                    CSRF_COOKIE => &mut out.csrf,
                    LOGIN_COOKIE_NAME => &mut out.login,
                    _ => continue,
                };
                if value.is_empty() {
                    return Err(CookieError::Malformed);
                }
                if slot.replace(value.to_owned()).is_some() {
                    return Err(CookieError::Duplicate);
                }
            }
        }
        Ok(out)
    }
}

/// An HTTP token (RFC 9110): the characters allowed in a cookie name.
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// A cookie-octet (RFC 6265, section 4.1.1).
const fn is_cookie_octet(b: u8) -> bool {
    matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
}

/// The SHA-256 of a [`RandomToken`]'s bytes: what the database stores and the cache is keyed by.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenHash(..)")
    }
}

/// The length of a token's text: 32 bytes in unpadded base64url.
const TOKEN_TEXT_LEN: usize = 43;

/// 256 random bits: a session id or a CSRF token.
pub struct RandomToken(Secret<[u8; 32]>);

impl std::fmt::Debug for RandomToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RandomToken([redacted])")
    }
}

impl RandomToken {
    /// Fresh bits from the system's secure generator. `None` only if the generator fails.
    pub fn generate() -> Option<Self> {
        let mut bytes = [0u8; 32];
        aws_lc_rs::rand::fill(&mut bytes).ok()?;
        Some(Self(Secret::new(bytes)))
    }

    /// The token a cookie or header value stands for: exactly 43 base64url characters, canonical, and nothing else.
    pub fn from_text(value: &str) -> Option<Self> {
        // 32 bytes are 43 characters. The decoder refuses padding, other alphabets and non-zero trailing bits, so one token
        // has exactly one text.
        if value.len() != TOKEN_TEXT_LEN {
            return None;
        }
        let mut bytes = [0u8; 32];
        let n = URL_SAFE_NO_PAD.decode_slice(value.as_bytes(), &mut bytes).ok()?;
        (n == 32).then(|| Self(Secret::new(bytes)))
    }

    /// The text for a cookie or header.
    pub fn to_text(&self) -> Secret<String> {
        Secret::new(URL_SAFE_NO_PAD.encode(self.0.expose()))
    }

    pub fn hash(&self) -> TokenHash {
        TokenHash(Sha256::digest(self.0.expose()).into())
    }
}

/// `Set-Cookie` for the session: lives `max_age_secs`, `HttpOnly`.
pub fn session_set_cookie(value: &Secret<String>, max_age_secs: i64) -> String {
    set_cookie(SESSION_COOKIE, value.expose(), max_age_secs, true)
}

/// `Set-Cookie` for the CSRF token: the page reads it, so no `HttpOnly`.
pub fn csrf_set_cookie(value: &Secret<String>, max_age_secs: i64) -> String {
    set_cookie(CSRF_COOKIE, value.expose(), max_age_secs, false)
}

/// `Set-Cookie` for the sealed login state: `HttpOnly`, and `Lax` so that it returns on Entra's redirect.
pub fn login_set_cookie(sealed: &str, max_age_secs: i64) -> String {
    set_cookie(LOGIN_COOKIE_NAME, sealed, max_age_secs, true)
}

/// `Set-Cookie` that removes a cookie of ours.
pub fn clear_cookie(name: &str) -> String {
    let http_only = name != CSRF_COOKIE;
    let mut line = format!("{name}=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; Secure");
    if http_only {
        line.push_str("; HttpOnly");
    }
    line.push_str("; SameSite=Lax");
    line
}

/// The one attribute set: `Path=/` and no `Domain`, which `__Host-` requires; `Secure`; `SameSite=Lax`, so that the cookie
/// comes back on Entra's top-level redirect and not on a cross-site subrequest.
fn set_cookie(name: &str, value: &str, max_age_secs: i64, http_only: bool) -> String {
    let mut line = format!("{name}={value}; Max-Age={max_age_secs}; Path=/; Secure");
    if http_only {
        line.push_str("; HttpOnly");
    }
    line.push_str("; SameSite=Lax");
    line
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn secret(s: &str) -> Secret<String> {
        Secret::new(s.to_owned())
    }

    fn attributes(set_cookie: &str) -> Vec<&str> {
        set_cookie.split("; ").skip(1).collect()
    }

    #[test]
    fn session_cookie_attributes_exact() {
        let line = session_set_cookie(&secret("VALUE"), 43_200);
        insta::assert_snapshot!(line, @"__Host-lk_session=VALUE; Max-Age=43200; Path=/; Secure; HttpOnly; SameSite=Lax");
        assert!(
            !line.to_ascii_lowercase().contains("domain"),
            "a __Host- cookie has no Domain"
        );
        assert_eq!(
            attributes(&line),
            ["Max-Age=43200", "Path=/", "Secure", "HttpOnly", "SameSite=Lax"]
        );
    }

    #[test]
    fn csrf_cookie_is_host_prefixed_secure_and_readable_by_script() {
        let line = csrf_set_cookie(&secret("VALUE"), 43_200);
        insta::assert_snapshot!(line, @"__Host-lk_csrf=VALUE; Max-Age=43200; Path=/; Secure; SameSite=Lax");
        assert!(line.starts_with("__Host-"));
        assert!(line.contains("; Secure"));
        assert!(
            !line.contains("HttpOnly"),
            "the page must be able to read the CSRF token"
        );
    }

    #[test]
    fn login_and_clearing_cookies_are_golden() {
        insta::assert_snapshot!(
            login_set_cookie("SEALED", 600),
            @"__Host-lk_login=SEALED; Max-Age=600; Path=/; Secure; HttpOnly; SameSite=Lax"
        );
        insta::assert_snapshot!(
            clear_cookie(SESSION_COOKIE),
            @"__Host-lk_session=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; Secure; HttpOnly; SameSite=Lax"
        );
        insta::assert_snapshot!(
            clear_cookie(CSRF_COOKIE),
            @"__Host-lk_csrf=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; Secure; SameSite=Lax"
        );
        insta::assert_snapshot!(
            clear_cookie(LOGIN_COOKIE_NAME),
            @"__Host-lk_login=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/; Secure; HttpOnly; SameSite=Lax"
        );
    }

    #[test]
    fn cookie_parser_reads_our_cookies_among_others() {
        let got = RequestCookies::parse(["theme=dark; __Host-lk_session=abc; other=1", "__Host-lk_csrf=def"])
            .unwrap();
        assert_eq!(got.session.as_deref(), Some("abc"));
        assert_eq!(got.csrf.as_deref(), Some("def"));
        assert_eq!(got.login, None);
        let none = RequestCookies::parse(["a=b"]).unwrap();
        assert!(none.session.is_none() && none.csrf.is_none() && none.login.is_none());
        let empty = RequestCookies::parse(std::iter::empty::<&str>()).unwrap();
        assert!(empty.session.is_none());
    }

    #[test]
    fn cookie_parser_rejects_duplicates_and_oversize() {
        for headers in [
            vec!["__Host-lk_session=a; __Host-lk_session=b"],
            vec!["__Host-lk_session=a", "__Host-lk_session=a"],
            vec!["__Host-lk_csrf=a; x=y; __Host-lk_csrf=b"],
            vec!["__Host-lk_login=a; __Host-lk_login=b"],
        ] {
            assert_eq!(
                RequestCookies::parse(headers.clone()).unwrap_err(),
                CookieError::Duplicate,
                "{headers:?}"
            );
        }
        let big = format!("x={}", "a".repeat(MAX_COOKIE_HEADER_BYTES));
        assert_eq!(
            RequestCookies::parse([big.as_str()]).unwrap_err(),
            CookieError::Oversize
        );
        // The cap is on the sum over all headers.
        let half = format!("x={}", "a".repeat(MAX_COOKIE_HEADER_BYTES / 2 + 1));
        assert_eq!(
            RequestCookies::parse([half.as_str(), half.as_str()]).unwrap_err(),
            CookieError::Oversize
        );
        let long_value = format!("__Host-lk_session={}", "a".repeat(MAX_COOKIE_VALUE_BYTES + 1));
        assert_eq!(
            RequestCookies::parse([long_value.as_str()]).unwrap_err(),
            CookieError::Oversize
        );
        let many = (0..=MAX_COOKIE_PAIRS)
            .map(|i| format!("c{i}=1"))
            .collect::<Vec<_>>()
            .join("; ");
        assert_eq!(
            RequestCookies::parse([many.as_str()]).unwrap_err(),
            CookieError::TooMany
        );
    }

    #[test]
    fn cookie_parser_rejects_malformed_pairs() {
        for bad in [
            "novalue",
            "=novalue",
            "__Host-lk_session=a b",
            "__Host-lk_session=\"quoted\"",
            "__Host-lk_session=a,b",
            "__Host-lk_session=a\\b",
            "__Host-lk_session=a;;b=c",
            "__Host-lk_session=a\u{7f}",
            "__Host-lk_session=\u{e9}",
            "a b=c",
            "__Host-lk_session=a\tb",
            ";",
        ] {
            assert!(RequestCookies::parse([bad]).is_err(), "{bad:?} was accepted");
        }
        // An empty value of ours is not a token; it is refused here and not treated as "no session".
        assert!(RequestCookies::parse(["__Host-lk_session="]).is_err());
        // Tolerated: the separator with extra spaces, a trailing separator, and a foreign cookie with an empty value.
        assert!(RequestCookies::parse(["a=1;  b=2"]).is_ok());
        assert!(RequestCookies::parse(["a=1; "]).is_ok());
        assert!(RequestCookies::parse(["a="]).is_ok());
    }

    #[test]
    fn random_tokens_are_fresh_and_round_trip_through_their_text() {
        let a = RandomToken::generate().unwrap();
        let b = RandomToken::generate().unwrap();
        let (ta, tb) = (a.to_text(), b.to_text());
        assert_ne!(ta.expose(), tb.expose());
        assert_eq!(ta.expose().len(), 43);
        let back = RandomToken::from_text(ta.expose()).unwrap();
        assert_eq!(back.hash(), a.hash());
        assert_ne!(a.hash(), b.hash());
        assert_eq!(back.to_text().expose(), ta.expose());
    }

    #[test]
    fn token_text_must_be_canonical_base64url_of_32_bytes() {
        const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let ok = RandomToken::generate().unwrap().to_text();
        let good = ok.expose();
        assert!(RandomToken::from_text(good).is_some());
        for bad in [
            String::new(),
            good[..42].to_owned(),
            format!("{good}A"),
            format!("{good}="),
            format!("{} ", &good[..42]),
            format!(" {}", &good[..42]),
            format!("{}+", &good[..42]),
            format!("{}/", &good[..42]),
            "\u{e9}".repeat(22),
        ] {
            assert!(RandomToken::from_text(&bad).is_none(), "{bad:?} was accepted");
        }
        // 43 characters carry 258 bits and a token has 256: the last character's low two bits are zero in the one canonical
        // spelling. Another spelling of the same bytes is refused, so one token has exactly one text.
        let last = good.chars().last().unwrap();
        let index = ALPHABET.find(last).unwrap();
        assert_eq!(index % 4, 0, "the generated token is canonical");
        let other = ALPHABET.chars().nth(index | 1).unwrap();
        let spelled = format!("{}{other}", &good[..42]);
        assert!(
            RandomToken::from_text(&spelled).is_none(),
            "a second spelling of a token was accepted"
        );
    }

    #[test]
    fn debug_output_never_shows_a_token() {
        let t = RandomToken::generate().unwrap();
        let text = t.to_text();
        let shown = format!(
            "{t:?} {:?} {:?}",
            t.hash(),
            RequestCookies::parse([format!("__Host-lk_session={}", text.expose()).as_str()]).unwrap()
        );
        assert!(!shown.contains(text.expose().as_str()));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn any_header_text_parses_or_errors_without_panicking(s in "\\PC{0,300}") {
            let _ = RequestCookies::parse([s.as_str()]);
        }

        #[test]
        fn an_accepted_token_text_encodes_back_to_itself(s in "[A-Za-z0-9_-]{43}") {
            if let Some(t) = RandomToken::from_text(&s) {
                let text = t.to_text();
                prop_assert_eq!(text.expose(), &s);
            }
        }

        #[test]
        fn a_parsed_cookie_value_has_only_cookie_octets(v in "[ -~]{0,64}") {
            let header = format!("__Host-lk_session={v}");
            if let Ok(c) = RequestCookies::parse([header.as_str()]) {
                if let Some(s) = c.session {
                    prop_assert!(s.bytes().all(|b| (0x21..=0x7e).contains(&b) && !matches!(b, b'"' | b',' | b';' | b'\\')));
                }
            }
        }
    }
}
