//! S11, S14: the `Authorization` header parser. Strict, bounded, and the same answer for the same bytes. The fuzz target
//! `mcp_bearer_header` asserts the properties for arbitrary input; these are the cases a person would write.

use http::{HeaderMap, HeaderValue, header};
use hub_mcp::auth::header::{HeaderError, MAX_TOKEN_BYTES, bearer};
use proptest::prelude::*;

fn headers(values: &[&str]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for value in values {
        map.append(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
    }
    map
}

#[test]
fn a_bearer_token_is_returned_as_written() {
    assert_eq!(
        bearer(&headers(&["Bearer abc.DEF-123_~+/="])),
        Ok("abc.DEF-123_~+/=")
    );
}

#[test]
fn the_scheme_is_matched_without_regard_to_case() {
    for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
        assert_eq!(
            bearer(&headers(&[&format!("{scheme} tok")])),
            Ok("tok"),
            "{scheme}"
        );
    }
}

#[test]
fn no_header_is_missing_and_two_headers_are_refused() {
    assert_eq!(bearer(&HeaderMap::new()), Err(HeaderError::Missing));
    assert_eq!(
        bearer(&headers(&["Bearer a", "Bearer b"])),
        Err(HeaderError::Duplicate),
        "two credentials are ambiguous; neither is used"
    );
}

#[test]
fn another_scheme_is_not_bearer() {
    for value in ["Basic dXNlcjpwYXNz", "Token abc", "Bearerabc", "Digest x=y"] {
        assert_eq!(bearer(&headers(&[value])), Err(HeaderError::NotBearer), "{value}");
    }
}

#[test]
fn a_bearer_without_a_usable_token_is_malformed() {
    for value in [
        "Bearer",
        "Bearer ",
        "Bearer  two-spaces",
        "Bearer tok en",
        "Bearer tok\ten",
        "Bearer \"quoted\"",
        "Bearer a=b",
        "Bearer =",
        "Bearer a,b",
        "Bearer a;b",
    ] {
        assert_eq!(
            bearer(&headers(&[value])),
            Err(HeaderError::Malformed),
            "{value:?} should be malformed"
        );
    }
}

#[test]
fn a_value_that_is_not_visible_ascii_is_malformed() {
    let mut map = HeaderMap::new();
    map.insert(
        header::AUTHORIZATION,
        HeaderValue::from_bytes(b"Bearer caf\xc3\xa9").unwrap(),
    );
    assert_eq!(bearer(&map), Err(HeaderError::Malformed));
}

#[test]
fn the_token_limit_is_the_ports_limit_and_a_longer_token_is_refused_unread() {
    assert_eq!(MAX_TOKEN_BYTES, ports::MAX_TOKEN_BYTES);
    let at_limit = "a".repeat(MAX_TOKEN_BYTES);
    assert_eq!(
        bearer(&headers(&[&format!("Bearer {at_limit}")])).map(str::len),
        Ok(MAX_TOKEN_BYTES)
    );
    // One byte over, and made of characters that would be malformed: the length decides first, so no character is looked at.
    let over = format!("Bearer {}", ";".repeat(MAX_TOKEN_BYTES + 1));
    assert_eq!(bearer(&headers(&[&over])), Err(HeaderError::TooLong));
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic_and_an_accepted_token_is_a_slice_of_the_header(
        value in proptest::collection::vec(any::<u8>(), 0..200)
    ) {
        let mut map = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_bytes(&value) {
            map.insert(header::AUTHORIZATION, v);
            if let Ok(token) = bearer(&map) {
                prop_assert!(!token.is_empty());
                prop_assert!(token.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b)));
                prop_assert!(value.ends_with(token.as_bytes()));
            }
        }
    }
}
