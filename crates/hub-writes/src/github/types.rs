//! The small typed values of the GitHub leg: a login, a repository, the shape of a token and the expiry header. Each is parsed at
//! the edge, so nothing downstream handles a raw string (S11), and each error is a fixed word that holds none of the input (S21):
//! a rejected string may be a token.

use std::fmt;

use domain::Timestamp;

/// The prefix of a fine-grained personal access token (S8).
pub const FINE_GRAINED_PREFIX: &str = "github_pat_";

/// The longest token the API accepts (`GithubTokenRequest.token`, `maxLength: 512`).
pub const MAX_TOKEN_LEN: usize = 512;

/// A fine-grained token is the prefix and at least this many more characters. GitHub's are 82 characters in all; the floor only
/// refuses what cannot be a token, it does not try to know the format.
const MIN_BODY_LEN: usize = 20;

/// Why a string is not a token the vault takes. None of these carries the string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenShapeError {
    #[error("the token is empty")]
    Empty,
    #[error("the token is too long")]
    TooLong,
    /// A classic personal access token, an OAuth or app token: refused by policy (S8).
    #[error("classic tokens are not accepted")]
    Classic,
    /// Anything else that is not a fine-grained token.
    #[error("the token is not a fine-grained token")]
    NotFineGrained,
    #[error("the token holds a character a token cannot hold")]
    BadChars,
}

/// Checks that `token` has the shape of a fine-grained personal access token: the `github_pat_` prefix and then letters, digits
/// and underscores, within 512 characters. A classic token (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, or 40 hexadecimal
/// characters) is told apart, so the answer can cite the policy. This is the only judge of a token's shape; GitHub is asked
/// next whether the token works.
pub fn check_token_shape(token: &str) -> Result<(), TokenShapeError> {
    if token.is_empty() {
        return Err(TokenShapeError::Empty);
    }
    if token.len() > MAX_TOKEN_LEN {
        return Err(TokenShapeError::TooLong);
    }
    let Some(body) = token.strip_prefix(FINE_GRAINED_PREFIX) else {
        let classic = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"]
            .iter()
            .any(|p| token.starts_with(p))
            || (token.len() == 40 && token.bytes().all(|b| b.is_ascii_hexdigit()));
        return Err(if classic {
            TokenShapeError::Classic
        } else {
            TokenShapeError::NotFineGrained
        });
    };
    if !body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(TokenShapeError::BadChars);
    }
    if body.len() < MIN_BODY_LEN {
        return Err(TokenShapeError::NotFineGrained);
    }
    Ok(())
}

/// Why a login or a repository name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("the name is not valid")]
    Invalid,
}

/// A GitHub login: 1 to 39 letters, digits and single hyphens, not starting or ending with one. GitHub compares logins without
/// regard to case, so [`GithubLogin::same_as`] does too.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GithubLogin(String);

impl GithubLogin {
    pub fn parse(s: &str) -> Result<Self, NameError> {
        if valid_owner(s) {
            Ok(Self(s.to_owned()))
        } else {
            Err(NameError::Invalid)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Is this the same account as `other`? Logins differ in case only by how a person typed them.
    #[must_use]
    pub fn same_as(&self, other: &str) -> bool {
        self.0.eq_ignore_ascii_case(other)
    }
}

impl fmt::Debug for GithubLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A login names a person; it is an identifier, not a secret, and it appears in audit events. It is still not printed by
        // accident in a debug dump of a larger value.
        f.debug_tuple("GithubLogin").field(&self.0).finish()
    }
}

impl fmt::Display for GithubLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn valid_owner(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 39
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A repository, `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoRef {
    owner: String,
    name: String,
}

impl RepoRef {
    pub fn parse(s: &str) -> Result<Self, NameError> {
        let (owner, name) = s.split_once('/').ok_or(NameError::Invalid)?;
        let name_ok = !name.is_empty()
            && name.len() <= 100
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if valid_owner(owner) && name_ok {
            Ok(Self {
                owner: owner.to_owned(),
                name: name.to_owned(),
            })
        } else {
            Err(NameError::Invalid)
        }
    }

    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The API path of the repository: `/repos/{owner}/{name}`. Both parts are restricted to URL-safe characters.
    #[must_use]
    pub fn api_path(&self) -> String {
        format!("/repos/{}/{}", self.owner, self.name)
    }
}

impl fmt::Display for RepoRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// The days from 1970-01-01 to a civil date (proleptic Gregorian). Howard Hinnant's algorithm.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let day_of_year = (153 * mp + 2) / 5 + d - 1;
    let day_of_era = yoe * 365 + yoe / 4 - yoe / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// Reads the `github-authentication-token-expiration` header: `2026-11-20 12:00:00 UTC`. A value in any other shape is `None`,
/// which the vault stores as "expiry unknown". The year is held to 2000 to 2200, so a hostile header cannot make a time that
/// overflows anything downstream.
#[must_use]
pub fn parse_expiry(value: &str) -> Option<Timestamp> {
    let rest = value.strip_suffix(" UTC")?;
    let (date, time) = rest.split_once(' ')?;
    let mut d = date.split('-');
    let (y, m, day) = (d.next()?, d.next()?, d.next()?);
    if d.next().is_some() || y.len() != 4 || m.len() != 2 || day.len() != 2 {
        return None;
    }
    let mut t = time.split(':');
    let (hh, mm, ss) = (t.next()?, t.next()?, t.next()?);
    if t.next().is_some() || hh.len() != 2 || mm.len() != 2 || ss.len() != 2 {
        return None;
    }
    let num = |s: &str| -> Option<i64> {
        if s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    let (y, m, day) = (num(y)?, num(m)?, num(day)?);
    let (hh, mm, ss) = (num(hh)?, num(mm)?, num(ss)?);
    if !(2000..=2200).contains(&y) || !(1..=12).contains(&m) || day < 1 || day > days_in_month(y, m) {
        return None;
    }
    if hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    let secs = days_from_civil(y, m, day) * 86_400 + hh * 3_600 + mm * 60 + ss;
    Some(Timestamp::from_unix_millis(secs * 1_000))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// 82 characters, the length of a real fine-grained token. Not a real token.
    fn token() -> String {
        format!(
            "{FINE_GRAINED_PREFIX}{}",
            "A1b2C3d4E5_f6G7h8I9j0K1l2M3n4O5p6Q7r8S9t0U1v2W3x4Y5z6A7b8C9d0"
        )
    }

    #[test]
    fn a_fine_grained_token_is_taken() {
        assert_eq!(check_token_shape(&token()), Ok(()));
    }

    #[test]
    fn classic_and_other_tokens_are_refused_and_told_apart() {
        for classic in [
            "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
            "gho_0123456789abcdefghijklmnopqrstuvwxyz",
            "ghu_0123456789abcdefghijklmnopqrstuvwxyz",
            "ghs_0123456789abcdefghijklmnopqrstuvwxyz",
            "ghr_0123456789abcdefghijklmnopqrstuvwxyz",
            "0123456789abcdef0123456789abcdef01234567",
        ] {
            assert_eq!(
                check_token_shape(classic),
                Err(TokenShapeError::Classic),
                "{classic}"
            );
        }
        for other in ["hello", "Bearer abc", "github_pat", "xyz_0123456789"] {
            assert_eq!(
                check_token_shape(other),
                Err(TokenShapeError::NotFineGrained),
                "{other}"
            );
        }
        assert_eq!(check_token_shape(""), Err(TokenShapeError::Empty));
    }

    #[test]
    fn the_shape_has_a_length_and_a_charset() {
        assert_eq!(
            check_token_shape(&format!("{FINE_GRAINED_PREFIX}{}", "a".repeat(MAX_TOKEN_LEN))),
            Err(TokenShapeError::TooLong)
        );
        let at_the_limit = format!(
            "{FINE_GRAINED_PREFIX}{}",
            "a".repeat(MAX_TOKEN_LEN - FINE_GRAINED_PREFIX.len())
        );
        assert_eq!(check_token_shape(&at_the_limit), Ok(()));
        for bad in ["a b", "a\nb", "a-b", "a.b", "é", "a\0b"] {
            let t = format!("{FINE_GRAINED_PREFIX}{}{bad}", "x".repeat(30));
            assert_eq!(check_token_shape(&t), Err(TokenShapeError::BadChars), "{bad:?}");
        }
        assert_eq!(
            check_token_shape(&format!("{FINE_GRAINED_PREFIX}short")),
            Err(TokenShapeError::NotFineGrained)
        );
    }

    #[test]
    fn a_shape_error_never_holds_the_input() {
        let secret = "github_pat_SECRETSECRETSECRETSECRET-with-a-dash";
        let e = check_token_shape(secret).unwrap_err();
        assert!(!format!("{e} {e:?}").contains("SECRETSECRET"));
    }

    proptest! {
        /// Any string is judged without a panic, and what is accepted has the prefix, a bounded length and a plain charset.
        #[test]
        fn what_is_accepted_is_a_plain_prefixed_token(s in "\\PC{0,600}") {
            if check_token_shape(&s).is_ok() {
                prop_assert!(s.starts_with(FINE_GRAINED_PREFIX));
                prop_assert!(s.len() <= MAX_TOKEN_LEN);
                prop_assert!(s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
            }
        }
    }

    #[test]
    fn logins_follow_githubs_rules() {
        for ok in ["octocat", "a", "Mona-Lisa", "a1-b2", &"a".repeat(39)] {
            assert!(GithubLogin::parse(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-a", "a-", "a--b", "a_b", "a b", "a/b", &"a".repeat(40), "é"] {
            assert!(GithubLogin::parse(bad).is_err(), "{bad}");
        }
        let l = GithubLogin::parse("Octocat").unwrap();
        assert!(l.same_as("octocat") && l.same_as("OCTOCAT") && !l.same_as("octocat2"));
    }

    #[test]
    fn repositories_are_owner_and_name() {
        let r = RepoRef::parse("acme/configuration-base-saas").unwrap();
        assert_eq!((r.owner(), r.name()), ("acme", "configuration-base-saas"));
        assert_eq!(r.api_path(), "/repos/acme/configuration-base-saas");
        assert_eq!(r.to_string(), "acme/configuration-base-saas");
        for bad in [
            "", "acme", "acme/", "/x", "a/b/c", "acme/..", "acme/.", "acme/a b", "-a/b", "a/b?x=1", "a/b#c",
        ] {
            assert!(RepoRef::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_expiry_header_is_read() {
        // 2026-11-20 12:00:00 UTC.
        assert_eq!(
            parse_expiry("2026-11-20 12:00:00 UTC").map(Timestamp::unix_millis),
            Some(1_795_176_000_000)
        );
        assert_eq!(
            parse_expiry("2000-01-01 00:00:00 UTC").map(Timestamp::unix_millis),
            Some(946_684_800_000)
        );
        // A leap day is a day; the day after a month's last is not.
        assert!(parse_expiry("2028-02-29 00:00:00 UTC").is_some());
        assert!(parse_expiry("2027-02-29 00:00:00 UTC").is_none());
        for bad in [
            "",
            "2026-11-20",
            "2026-11-20 12:00:00",
            "2026-11-20 12:00:00 PST",
            "2026-13-20 12:00:00 UTC",
            "2026-00-20 12:00:00 UTC",
            "2026-11-31 12:00:00 UTC",
            "2026-11-20 24:00:00 UTC",
            "2026-11-20 12:60:00 UTC",
            "1999-11-20 12:00:00 UTC",
            "2201-11-20 12:00:00 UTC",
            "2026-1-20 12:00:00 UTC",
            "+026-11-20 12:00:00 UTC",
            "2026-11-20 12:00 UTC",
            "2026-11-20 12:00:00:00 UTC",
            "2026-11-20  12:00:00 UTC",
        ] {
            assert!(parse_expiry(bad).is_none(), "{bad}");
        }
    }

    proptest! {
        #[test]
        fn the_expiry_parser_never_panics_and_stays_in_range(s in "\\PC{0,40}") {
            if let Some(t) = parse_expiry(&s) {
                let ms = t.unix_millis();
                prop_assert!((946_684_800_000..=7_258_118_400_000).contains(&ms));
            }
        }

        /// A time written out and read back is the same time, for any second in the supported years.
        #[test]
        fn a_written_time_reads_back(days in 0_i64..73_000, secs in 0_i64..86_400) {
            let base = days_from_civil(2000, 1, 1);
            // Invert days_from_civil for the test by walking the civil calendar from 2000-01-01.
            let (mut y, mut m, mut d) = (2000_i64, 1_i64, 1_i64);
            for _ in 0..days {
                d += 1;
                if d > days_in_month(y, m) { d = 1; m += 1; if m > 12 { m = 1; y += 1; } }
            }
            prop_assume!(y <= 2200);
            let text = format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", secs / 3600, secs % 3600 / 60, secs % 60);
            let got = parse_expiry(&text).unwrap().unix_millis();
            prop_assert_eq!(got, (days_from_civil(y, m, d) * 86_400 + secs) * 1000);
            prop_assert_eq!(days_from_civil(y, m, d) - base, days);
        }
    }
}
