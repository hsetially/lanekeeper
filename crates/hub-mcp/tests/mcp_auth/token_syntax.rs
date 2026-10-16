//! S7, S11: the syntax of a personal token and the keyed hash the store holds. The fuzz target `mcp_pat_token` asserts the
//! properties for arbitrary text; these are the cases a person would write, with one known answer from an independent HMAC.

use std::collections::HashSet;

use hub_mcp::auth::token::{
    DigestLength, HmacKey, KeyError, MAX_KEY_BYTES, MIN_KEY_BYTES, PREFIX, PersonalToken, TOKEN_LEN,
    TokenHmac, generate, looks_like_personal_token,
};
use proptest::prelude::*;

fn key() -> HmacKey {
    HmacKey::from_bytes(&(0..32).collect::<Vec<u8>>()).unwrap()
}

#[test]
fn a_generated_token_has_the_documented_shape_and_parses() {
    let text = generate().unwrap();
    assert_eq!(text.len(), TOKEN_LEN);
    assert_eq!(TOKEN_LEN, 49);
    assert!(text.starts_with(PREFIX));
    assert!(looks_like_personal_token(&text));
    assert_eq!(PersonalToken::parse(&text).unwrap().as_str(), text);
    assert!(
        text[PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
}

#[test]
fn generated_tokens_do_not_repeat() {
    let tokens: HashSet<String> = (0..200).map(|_| generate().unwrap()).collect();
    assert_eq!(tokens.len(), 200);
}

#[test]
fn the_hash_is_hmac_sha256_of_the_token_text() {
    // Computed with Python's `hmac` and `hashlib`, not with this crate's code: key = bytes 0..32, token = prefix + 43 x "A".
    let token = format!("{PREFIX}{}", "A".repeat(43));
    let parsed = PersonalToken::parse(&token).unwrap();
    assert_eq!(
        hex::encode(key().lookup(&parsed).unwrap().as_bytes()),
        "9d710e97e9ab568ac9a73ac23dba6834b6d5f69f50863f038076f061cfed07c1"
    );
    // The all-ones secret (a "_" in the last position), which exercises the other end of the alphabet.
    let token = format!("{PREFIX}{}8", "_".repeat(42));
    let parsed = PersonalToken::parse(&token).unwrap();
    assert_eq!(
        hex::encode(key().lookup(&parsed).unwrap().as_bytes()),
        "ad0b8f23b4db46a8fa45ac2d8cef40bcf1f94e77e4168a110a8854d63eddafe4"
    );
}

/// AGENTS.md rule 2: nothing here panics. HMAC-SHA-256 is 32 bytes, but the conversion does not rely on it: a digest of any other
/// length is an error, and that error reaches the caller (`lookup` returns it) and does not become a hash.
#[test]
fn a_digest_of_the_wrong_length_is_an_error_and_not_a_panic() {
    for len in [0_usize, 1, 16, 31, 33, 64, 1024] {
        assert_eq!(
            TokenHmac::from_digest(&vec![7_u8; len]),
            Err(DigestLength),
            "{len} bytes"
        );
    }
    assert_eq!(
        TokenHmac::from_digest(&[7_u8; 32]),
        Ok(TokenHmac::from_bytes([7_u8; 32]))
    );
    // The hash of a token goes through the same conversion, and comes back as the 32 bytes of the algorithm.
    let token = generate().unwrap();
    let parsed = PersonalToken::parse(&token).unwrap();
    let hash: Result<TokenHmac, DigestLength> = key().lookup(&parsed);
    assert_eq!(hash.unwrap().as_bytes().len(), 32);
}

#[test]
fn another_key_gives_another_hash_and_the_same_key_the_same_hash() {
    let token = generate().unwrap();
    let parsed = PersonalToken::parse(&token).unwrap();
    let other = HmacKey::from_bytes(&[7_u8; 32]).unwrap();
    assert_eq!(key().lookup(&parsed).unwrap(), key().lookup(&parsed).unwrap());
    assert_ne!(key().lookup(&parsed).unwrap(), other.lookup(&parsed).unwrap());
}

#[test]
fn a_token_with_any_other_shape_is_not_parsed() {
    let good = generate().unwrap();
    let body = &good[PREFIX.len()..];
    let candidates = [
        String::new(),
        PREFIX.to_owned(),
        format!("{PREFIX}{}", &body[..42]),
        format!("{PREFIX}{body}A"),
        format!("lkpat-{body}"),
        format!("LKPAT_{body}"),
        format!(" {good}"),
        format!("{good} "),
        format!("{good}\n"),
        format!("{PREFIX}{}=", &body[..42]),
        format!("{PREFIX}{}+", &body[..42]),
        format!("{PREFIX}{}/", &body[..42]),
        format!("{PREFIX}{}\u{e9}", &body[..41]),
        good.replace(&body[..1], "!"),
    ];
    for text in candidates {
        assert!(PersonalToken::parse(&text).is_none(), "{text:?}");
    }
}

#[test]
fn a_spelling_that_is_not_the_canonical_one_is_refused() {
    // 43 characters carry 258 bits; the last two must be zero. "B" in the last position sets one of them, so the same 256 bits
    // would have a second spelling (and a second hash) if it were accepted.
    let canonical = format!("{PREFIX}{}A", "A".repeat(42));
    let second_spelling = format!("{PREFIX}{}B", "A".repeat(42));
    assert!(PersonalToken::parse(&canonical).is_some());
    assert!(PersonalToken::parse(&second_spelling).is_none());
}

#[test]
fn a_jwt_does_not_look_like_a_personal_token() {
    assert!(!looks_like_personal_token("eyJhbGciOiJSUzI1NiJ9.e30.c2ln"));
    assert!(!looks_like_personal_token(""));
}

#[test]
fn keys_are_bounded_and_the_error_says_nothing_of_the_input() {
    assert_eq!(
        HmacKey::from_bytes(&[1; MIN_KEY_BYTES - 1]).unwrap_err(),
        KeyError::TooShort
    );
    assert!(HmacKey::from_bytes(&[1; MIN_KEY_BYTES]).is_ok());
    assert!(HmacKey::from_bytes(&[1; MAX_KEY_BYTES]).is_ok());
    assert_eq!(
        HmacKey::from_bytes(&[1; MAX_KEY_BYTES + 1]).unwrap_err(),
        KeyError::TooLong
    );

    let text = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"; // bytes 0..32, unpadded base64url
    assert!(HmacKey::from_secret_text(text).is_ok());
    assert!(
        HmacKey::from_secret_text(&format!("  {text}\n")).is_ok(),
        "a secret stored from a file ends in a newline"
    );
    assert_eq!(
        HmacKey::from_secret_text("not base64 !!").unwrap_err(),
        KeyError::Encoding
    );
    assert_eq!(HmacKey::from_secret_text("AAEC").unwrap_err(), KeyError::TooShort);
    assert_eq!(
        HmacKey::from_secret_text(&"A".repeat(5000)).unwrap_err(),
        KeyError::TooLong
    );
    for error in [KeyError::Encoding, KeyError::TooShort, KeyError::TooLong] {
        assert!(!error.to_string().contains("AAEC"));
    }
}

#[test]
fn nothing_secret_is_printed() {
    let token = generate().unwrap();
    let parsed = PersonalToken::parse(&token).unwrap();
    let hash = key().lookup(&parsed).unwrap();
    for printed in [
        format!("{parsed:?}"),
        format!("{key:?}", key = key()),
        format!("{hash:?}"),
    ] {
        assert!(printed.contains("redacted"), "{printed}");
        assert!(!printed.contains(&token[PREFIX.len()..]), "{printed}");
        assert!(!printed.contains(&hex::encode(hash.as_bytes())), "{printed}");
    }
}

proptest! {
    #[test]
    fn a_token_that_parses_has_one_spelling_and_a_stable_hash(text in "[A-Za-z0-9_=+/-]{0,60}") {
        let candidate = format!("{PREFIX}{text}");
        if let Some(token) = PersonalToken::parse(&candidate) {
            prop_assert_eq!(token.as_str(), candidate.as_str());
            prop_assert_eq!(candidate.len(), TOKEN_LEN);
            let again = PersonalToken::parse(&candidate).unwrap();
            prop_assert_eq!(key().lookup(&token).unwrap(), key().lookup(&again).unwrap());
        }
    }

    #[test]
    fn equal_hashes_compare_equal_and_one_flipped_bit_does_not(bytes in proptest::array::uniform32(any::<u8>()), bit in 0_usize..256) {
        let a = TokenHmac::from_bytes(bytes);
        let mut other = bytes;
        other[bit / 8] ^= 1 << (bit % 8);
        let b = TokenHmac::from_bytes(other);
        prop_assert_eq!(a, TokenHmac::from_bytes(bytes));
        prop_assert!(a.ct_matches(&TokenHmac::from_bytes(bytes)));
        prop_assert_ne!(a, b);
    }
}
