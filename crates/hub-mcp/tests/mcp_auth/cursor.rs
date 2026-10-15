//! S11: the cursors this server hands out (`src/output/cursor.rs`). One spelling for each value; another tool's cursor, a padded one,
//! a re-cased one and garbage are all refused; one that would be too long is not made.

use hub_mcp::output::cursor::{MAX_LEN, MAX_SKIP, PREFIX, decode, encode};
use proptest::prelude::*;

fn matches_the_contract(cursor: &str) -> bool {
    (1..=512).contains(&cursor.len())
        && cursor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'=' | b'.' | b'-'))
}

#[test]
fn a_cursor_round_trips() {
    for (tag, inner, skip) in [
        ("list_swimlanes", None, 0),
        ("list_swimlanes", Some("sit1"), 0),
        ("find_inconsistencies", Some("o17"), 12),
        ("get_pending_restarts", Some("apps/tx-core"), 0),
        ("compare_grid", Some("~!@#$%^&*()"), MAX_SKIP),
    ] {
        let cursor = encode(tag, inner, skip).unwrap();
        assert!(cursor.starts_with(PREFIX));
        assert!(matches_the_contract(&cursor), "{cursor}");
        let back = decode(&cursor, tag).unwrap();
        assert_eq!(
            (back.tag.as_str(), back.inner.as_deref(), back.skip),
            (tag, inner, skip)
        );
    }
}

#[test]
fn another_tools_cursor_is_refused() {
    let cursor = encode("list_files", Some("o5"), 1).unwrap();
    assert!(decode(&cursor, "list_files").is_ok());
    assert!(decode(&cursor, "get_drift").is_err());
    assert!(decode(&cursor, "list_file").is_err());
}

#[test]
fn only_the_one_spelling_is_accepted() {
    let cursor = encode("list_files", Some("o5"), 3).unwrap();
    let body = cursor.strip_prefix(PREFIX).unwrap();
    for bad in [
        format!("{cursor}="),
        format!("{cursor}=="),
        format!("{PREFIX}{}", body.to_uppercase()),
        format!("M1.{body}"),
        format!("m1:{body}"),
        format!(" {cursor}"),
        format!("{cursor} "),
        format!("{cursor}\n"),
        body.to_owned(),
        String::new(),
        PREFIX.to_owned(),
        // The same payload with a skip written with a leading zero.
        format!("{PREFIX}{}", base64url("list_files\n03\no5")),
        // A skip over the most a cursor may carry.
        format!(
            "{PREFIX}{}",
            base64url(&format!("list_files\n{}\no5", MAX_SKIP + 1))
        ),
        // A payload with only two lines, with a space in the key, with a control character.
        format!("{PREFIX}{}", base64url("list_files\n3")),
        format!("{PREFIX}{}", base64url("list_files\n3\no 5")),
        format!("{PREFIX}{}", base64url("list_files\n3\no\u{1}5")),
        format!("{PREFIX}{}", base64url("list_files\n+3\no5")),
    ] {
        assert!(decode(&bad, "list_files").is_err(), "{bad:?}");
    }
    // The empty key means the beginning, and is spelled by leaving it out.
    assert_eq!(encode("list_files", Some(""), 0), encode("list_files", None, 0));
    assert!(decode(&cursor, "list_files").is_ok());
}

fn base64url(text: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text)
}

#[test]
fn a_cursor_that_would_be_too_long_is_not_made() {
    assert!(encode("list_files", Some(&"k".repeat(300)), 0).is_some());
    assert!(
        encode("list_files", Some(&"k".repeat(400)), 0).is_none(),
        "the payload would make more than 512 characters"
    );
    assert!(encode("list_files", Some(&"k".repeat(600)), 0).is_none());
    assert!(encode("list_files", None, MAX_SKIP + 1).is_none());
    assert!(
        encode("List_Files", None, 0).is_none(),
        "a tool name is lower case with underscores"
    );
    assert!(encode("", None, 0).is_none());
    assert!(encode("list_files", Some("has space"), 0).is_none());
    let longest = encode("list_files", Some(&"k".repeat(370)), 200);
    if let Some(longest) = longest {
        assert!(longest.len() <= MAX_LEN);
    }
}

proptest! {
    #[test]
    fn encode_and_decode_are_inverse(
        tag in "[a-z_]{1,32}",
        inner in proptest::option::of("[!-~]{1,300}"),
        skip in 0..=MAX_SKIP,
    ) {
        if let Some(cursor) = encode(&tag, inner.as_deref(), skip) {
            prop_assert!(matches_the_contract(&cursor));
            let back = decode(&cursor, &tag).unwrap();
            prop_assert_eq!((back.inner.as_deref(), back.skip), (inner.as_deref(), skip));
        }
    }

    #[test]
    fn garbage_is_refused_and_an_accepted_cursor_is_canonical(text in ".{0,200}", tag in "[a-z_]{1,16}") {
        if let Ok(wrapped) = decode(&text, &tag) {
            let again = encode(&tag, wrapped.inner.as_deref(), wrapped.skip);
            prop_assert_eq!(again.as_deref(), Some(text.as_str()));
        }
    }

    #[test]
    fn a_payload_made_by_hand_is_accepted_only_if_it_is_what_encode_makes(
        tag in "[a-z_]{1,16}",
        skip in "[0-9]{1,4}",
        inner in "[!-~]{0,40}",
    ) {
        let cursor = format!("{PREFIX}{}", base64url(&format!("{tag}\n{skip}\n{inner}")));
        if let Ok(wrapped) = decode(&cursor, &tag) {
            prop_assert_eq!(wrapped.skip.to_string(), skip);
            prop_assert_eq!(wrapped.inner.unwrap_or_default(), inner);
        }
    }
}
