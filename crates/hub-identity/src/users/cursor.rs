//! Keyset cursors of the user and access-request lists (S11, S22). A cursor is base64url (no padding) of the last key
//! of the previous page, so it fits the `OpenAPI` `Cursor` pattern `^[A-Za-z0-9_=.-]{1,512}$`.
//!
//! A cursor comes from the client, so it is untrusted: decoding is strict and never panics, and a bad one is
//! [`AccessError::BadCursor`] (400). It carries no authority: the list still runs as the caller, with the caller's role.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::UserId;

use super::AccessError;

/// The longest cursor accepted (the `OpenAPI` pattern's bound).
pub const MAX_CURSOR_LEN: usize = 512;

fn decode_text(cursor: &str) -> Result<String, AccessError> {
    if cursor.is_empty() || cursor.len() > MAX_CURSOR_LEN {
        return Err(AccessError::BadCursor);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| AccessError::BadCursor)?;
    String::from_utf8(bytes).map_err(|_| AccessError::BadCursor)
}

/// The cursor that resumes the user list after `id`.
pub fn encode_user(id: &UserId) -> String {
    URL_SAFE_NO_PAD.encode(id.to_string())
}

/// The user a cursor resumes after.
pub fn decode_user(cursor: &str) -> Result<UserId, AccessError> {
    decode_text(cursor)?.parse().map_err(|_| AccessError::BadCursor)
}

/// The cursor that resumes the request list (newest first) after request `id`.
pub fn encode_request(id: i64) -> String {
    URL_SAFE_NO_PAD.encode(id.to_string())
}

/// The request id a cursor resumes after: a canonical positive decimal.
pub fn decode_request(cursor: &str) -> Result<i64, AccessError> {
    let text = decode_text(cursor)?;
    match text.parse::<i64>() {
        Ok(n) if n >= 1 && n.to_string() == text => Ok(n),
        _ => Err(AccessError::BadCursor),
    }
}

#[cfg(test)]
mod tests {
    use domain::Guid;
    use proptest::prelude::*;

    use super::*;

    fn id(n: u64) -> UserId {
        UserId::new(
            Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap(),
            Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
        )
    }

    #[test]
    fn cursors_fit_the_openapi_pattern() {
        let c = encode_user(&id(5));
        assert!(
            c.len() <= MAX_CURSOR_LEN
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        );
        assert!(encode_request(i64::MAX).len() <= MAX_CURSOR_LEN);
    }

    #[test]
    fn rejects_padding_noise_and_the_wrong_kind() {
        let user = encode_user(&id(1));
        assert_eq!(
            decode_request(&user),
            Err(AccessError::BadCursor),
            "a user cursor is not a request cursor"
        );
        assert_eq!(decode_user(&encode_request(7)), Err(AccessError::BadCursor));
        for bad in [
            "",
            "=",
            "AAAA",
            "not base64!",
            "MQ==",
            &"A".repeat(MAX_CURSOR_LEN + 1),
        ] {
            assert_eq!(decode_user(bad), Err(AccessError::BadCursor), "{bad:?}");
            assert_eq!(decode_request(bad), Err(AccessError::BadCursor), "{bad:?}");
        }
        // Not canonical decimal: a sign, a leading zero, zero, a negative number, overflow.
        for bad in ["+5", "05", "0", "-3", "99999999999999999999"] {
            assert_eq!(
                decode_request(&URL_SAFE_NO_PAD.encode(bad)),
                Err(AccessError::BadCursor),
                "{bad}"
            );
        }
    }

    proptest! {
        #[test]
        fn a_user_cursor_round_trips(n in any::<u64>()) {
            let c = encode_user(&id(n % 0xffff_ffff_ffff));
            prop_assert_eq!(encode_user(&decode_user(&c).unwrap()), c);
        }

        #[test]
        fn a_request_cursor_round_trips(n in 1..=i64::MAX) {
            let c = encode_request(n);
            prop_assert_eq!(decode_request(&c), Ok(n));
        }

        #[test]
        fn any_text_is_decoded_or_refused_without_panic(s in ".{0,700}") {
            if let Ok(user) = decode_user(&s) {
                // Accepted input is normalised by the id parser (case), so check idempotence.
                prop_assert_eq!(decode_user(&encode_user(&user)), Ok(user));
            }
            if let Ok(n) = decode_request(&s) {
                prop_assert_eq!(encode_request(n), s);
            }
        }
    }
}
