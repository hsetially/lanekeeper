//! T7 (S11, S22): the typed, bounded parse of GitHub's `push` and `pull_request` payloads. Only the fields the hub reads are
//! interpreted; each one is typed (`GitRef`, `CommitId`, a repository name) before any other code sees it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use hub_platform::git::payload::{
    PayloadError, PrAction, PullRequestEvent, PushEvent, PushedRef, RepoFullName,
};
use proptest::prelude::*;

const AFTER: &str = "0123456789abcdef0123456789abcdef01234567";
const BEFORE: &str = "89abcdef0123456789abcdef0123456789abcdef";
const ZERO: &str = "0000000000000000000000000000000000000000";

fn push(git_ref: &str, before: &str, after: &str, deleted: bool, repo: &str) -> String {
    format!(
        r#"{{"ref":"{git_ref}","before":"{before}","after":"{after}","deleted":{deleted},"created":false,"forced":false,
        "repository":{{"id":1,"full_name":"{repo}","private":true,"owner":{{"login":"acme"}}}},
        "pusher":{{"name":"someone"}},"commits":[{{"id":"{after}","message":"text","added":[],"removed":[],"modified":["a.yml"]}}],
        "sender":{{"login":"someone"}}}}"#
    )
}

#[test]
fn webhook_payload_fields_are_typed_push_branch() {
    let event =
        PushEvent::parse(push("refs/heads/sit1", BEFORE, AFTER, false, "acme/csp-tenant-data").as_bytes())
            .unwrap();
    assert_eq!(event.repository.as_str(), "acme/csp-tenant-data");
    match &event.target {
        PushedRef::Branch(name) => assert_eq!(name.as_str(), "sit1"),
        other @ PushedRef::Tag(_) => panic!("{other:?}"),
    }
    assert_eq!(event.before.as_ref().unwrap().as_str(), BEFORE);
    assert_eq!(event.after.as_str(), AFTER);
    assert!(!event.deleted);
}

#[test]
fn webhook_payload_branch_names_may_contain_slashes() {
    let event =
        PushEvent::parse(push("refs/heads/templates/common-docs", BEFORE, AFTER, false, "a/b").as_bytes())
            .unwrap();
    match event.target {
        PushedRef::Branch(name) => assert_eq!(name.as_str(), "templates/common-docs"),
        PushedRef::Tag(_) => panic!("a branch"),
    }
}

#[test]
fn webhook_payload_tag_push() {
    let event = PushEvent::parse(push("refs/tags/v1.2.3", ZERO, AFTER, false, "a/b").as_bytes()).unwrap();
    match event.target {
        PushedRef::Tag(name) => assert_eq!(name.as_str(), "v1.2.3"),
        PushedRef::Branch(_) => panic!("a tag"),
    }
    assert!(event.before.is_none(), "an all-zero before is a new ref");
}

#[test]
fn webhook_payload_deleted_ref_is_marked_by_the_flag_or_the_zero_id() {
    let by_flag = PushEvent::parse(push("refs/heads/x", BEFORE, AFTER, true, "a/b").as_bytes()).unwrap();
    assert!(by_flag.deleted);
    let by_zero = PushEvent::parse(push("refs/heads/x", BEFORE, ZERO, false, "a/b").as_bytes()).unwrap();
    assert!(by_zero.deleted);
}

#[test]
fn webhook_payload_refs_outside_heads_and_tags_are_refused() {
    for bad in [
        "refs/pull/1/head",
        "refs/notes/x",
        "heads/x",
        "x",
        "refs/heads/",
        "refs/tags/",
        "refs/heads/a..b",
        "refs/heads/-x",
    ] {
        let result = PushEvent::parse(push(bad, BEFORE, AFTER, false, "a/b").as_bytes());
        assert!(result.is_err(), "{bad} must be refused");
    }
}

#[test]
fn webhook_payload_commit_ids_must_be_hex_of_a_known_length() {
    for bad in ["", "xyz", "0123", &"g".repeat(40), &"a".repeat(41)] {
        let result = PushEvent::parse(push("refs/heads/x", BEFORE, bad, false, "a/b").as_bytes());
        assert_eq!(
            result.unwrap_err(),
            PayloadError::BadField("after"),
            "after = {bad:?}"
        );
    }
    let result = PushEvent::parse(push("refs/heads/x", "nothex", AFTER, false, "a/b").as_bytes());
    assert_eq!(result.unwrap_err(), PayloadError::BadField("before"));
}

#[test]
fn webhook_payload_repository_names_are_checked() {
    for bad in [
        "",
        "noslash",
        "a/b/c",
        "a b/c",
        "/c",
        "a/",
        "a/b\n",
        &format!("{}/b", "a".repeat(100)),
    ] {
        let result = PushEvent::parse(push("refs/heads/x", BEFORE, AFTER, false, bad).as_bytes());
        assert!(result.is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn webhook_payload_wrong_types_missing_fields_and_duplicates_are_errors_without_the_input() {
    let cases: [&[u8]; 8] = [
        b"",
        b"null",
        b"[]",
        b"{}",
        br#"{"ref": 1, "before": "x", "after": "y", "repository": {"full_name": "a/b"}}"#,
        br#"{"ref": "refs/heads/x", "before": "x", "after": "y"}"#,
        // A duplicated key: which one a parser keeps is a security question, so it is refused.
        br#"{"ref": "refs/heads/x", "ref": "refs/heads/y", "before": "x", "after": "y", "repository": {"full_name": "a/b"}}"#,
        &[0xff, 0xfe, 0x00],
    ];
    for body in cases {
        let error = PushEvent::parse(body).unwrap_err();
        let text = format!("{error} {error:?}");
        assert!(!text.contains("refs/heads"), "{text}");
    }
}

#[test]
fn webhook_payload_deep_nesting_is_bounded() {
    let mut body = String::from(r#"{"ref":"refs/heads/x","extra":"#);
    body.push_str(&"[".repeat(5_000));
    body.push_str(&"]".repeat(5_000));
    body.push('}');
    assert!(PushEvent::parse(body.as_bytes()).is_err());
}

#[test]
fn repo_full_name_compares_without_regard_to_case() {
    let a = RepoFullName::parse("Acme/Csp-Tenant-Data").unwrap();
    let b = RepoFullName::parse("acme/csp-tenant-data").unwrap();
    assert!(a.matches(&b));
    assert!(!a.matches(&RepoFullName::parse("acme/other").unwrap()));
}

fn pull_request(action: &str, number: i64, merged: bool, repo: &str) -> String {
    format!(
        r#"{{"action":"{action}","number":{number},"pull_request":{{"number":{number},"merged":{merged},"state":"closed",
        "html_url":"https://github.com/acme/x/pull/{number}","title":"text","body":"text","user":{{"login":"u"}}}},
        "repository":{{"full_name":"{repo}"}},"sender":{{"login":"u"}}}}"#
    )
}

#[test]
fn webhook_payload_pull_request_fields_are_typed() {
    let event =
        PullRequestEvent::parse(pull_request("closed", 42, true, "acme/csp-tenant-data").as_bytes()).unwrap();
    assert_eq!(event.number, 42);
    assert_eq!(event.action, PrAction::Closed);
    assert!(event.merged);
    assert_eq!(event.repository.as_str(), "acme/csp-tenant-data");

    for (text, action) in [
        ("opened", PrAction::Opened),
        ("reopened", PrAction::Reopened),
        ("edited", PrAction::Other),
    ] {
        let event = PullRequestEvent::parse(pull_request(text, 1, false, "a/b").as_bytes()).unwrap();
        assert_eq!(event.action, action, "{text}");
    }
}

#[test]
fn webhook_payload_pull_request_numbers_are_positive_and_fit_the_column() {
    for bad in [0_i64, -1, i64::from(i32::MAX) + 1] {
        let result = PullRequestEvent::parse(pull_request("opened", bad, false, "a/b").as_bytes());
        assert_eq!(result.unwrap_err(), PayloadError::BadField("number"), "{bad}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn webhook_payload_parse_never_panics_and_accepts_only_well_formed_values(body in proptest::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(event) = PushEvent::parse(&body) {
            prop_assert!(event.after.as_str().len() == 40 || event.after.as_str().len() == 64);
            match &event.target {
                PushedRef::Branch(n) | PushedRef::Tag(n) => prop_assert!(!n.as_str().is_empty() && !n.as_str().contains("..")),
            }
        }
        let _ = PullRequestEvent::parse(&body);
    }

    #[test]
    fn webhook_payload_round_trips_any_valid_branch_and_ids(
        branch in "[a-z][a-z0-9_-]{0,20}(/[a-z][a-z0-9_-]{0,20}){0,3}",
        before in "[0-9a-f]{40}",
        after in "[0-9a-f]{40}",
    ) {
        let body = push(&format!("refs/heads/{branch}"), &before, &after, false, "acme/x");
        let event = PushEvent::parse(body.as_bytes()).unwrap();
        match event.target {
            PushedRef::Branch(n) => prop_assert_eq!(n.as_str(), branch.as_str()),
            PushedRef::Tag(_) => prop_assert!(false),
        }
        prop_assert_eq!(event.after.as_str(), after.as_str());
    }
}
