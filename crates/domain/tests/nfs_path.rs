//! S11: `NfsPath` is the only way a path reaches the agent or the sentinel.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{NfsPath, PathError, RepoPath};
use proptest::prelude::*;

#[test]
fn rejects_every_traversal_form() {
    let forms = [
        "..",
        "../",
        "../etc/passwd",
        "a/../b",
        "a/b/..",
        "a/../../b",
        "./../a",
        "a/./../b",
        "/etc/passwd",
        "//etc/passwd",
        "a/b/../../..",
        "a\\..\\b",
        "..\\a",
        "a/..\\b",
    ];
    for form in forms {
        assert!(NfsPath::parse(form).is_err(), "{form:?} must be rejected");
    }
}

#[test]
fn rejects_nul_and_control_chars() {
    for bad in [
        "a\0b", "\0", "a/b\0", "a\nb", "a\rb", "a\tb", "a\u{1b}b", "a\u{7f}b", "a/\u{1}",
    ] {
        assert!(NfsPath::parse(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn rejects_over_1024_bytes() {
    let ok = format!("{}/{}", "a".repeat(255), "b".repeat(255));
    assert!(NfsPath::parse(&ok).is_ok());
    // Three 255-byte components, one of 254 and three separators: 1,022 bytes.
    let base = format!("{0}/{0}/{0}/{1}", "c".repeat(255), "c".repeat(254));
    assert_eq!(base.len(), 1022);
    assert!(NfsPath::parse(&base).is_ok());
    // 1,024 bytes exactly: accepted. 1,025: rejected.
    let exactly = format!("{base}/d");
    assert_eq!(exactly.len(), 1024);
    assert!(NfsPath::parse(&exactly).is_ok());
    let over = format!("{base}/dd");
    assert_eq!(over.len(), 1025);
    assert_eq!(NfsPath::parse(&over), Err(PathError::TooLong));
}

#[test]
fn rejects_a_component_over_255_bytes() {
    let long = "x".repeat(256);
    assert_eq!(NfsPath::parse(&long), Err(PathError::ComponentTooLong));
}

#[test]
fn rejects_empty_and_root_forms_for_a_file_path() {
    for bad in ["", ".", "./", "./.", "//", "/"] {
        assert!(NfsPath::parse(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn normalises_separators() {
    let cases = [
        ("a//b", "a/b"),
        ("a/./b", "a/b"),
        ("./a/b/", "a/b"),
        ("a/b///", "a/b"),
        ("a///b//c.yml", "a/b/c.yml"),
        (
            "tx-infinity-api/tx-infinity-core.yml",
            "tx-infinity-api/tx-infinity-core.yml",
        ),
    ];
    for (input, want) in cases {
        let parsed = NfsPath::parse(input).unwrap();
        assert_eq!(parsed.as_str(), want, "input {input:?}");
    }
}

#[test]
fn dots_inside_names_are_not_traversal() {
    for ok in [
        "...", "a/...", ".hidden", "a/.b/c", "a..b", "a/..b", "a/b..", "..a/b",
    ] {
        assert!(NfsPath::parse(ok).is_ok(), "{ok:?} is a legal file name");
    }
}

#[test]
fn prefix_parser_accepts_the_root() {
    for root in ["", ".", "./"] {
        let p = NfsPath::parse_prefix(root).unwrap();
        assert!(p.is_root());
        assert_eq!(p.as_str(), "");
    }
    assert!(NfsPath::root().is_root());
    assert!(NfsPath::parse_prefix("../x").is_err());
    assert!(NfsPath::parse_prefix("/x").is_err());
}

#[test]
fn accessors_split_the_path() {
    let p = NfsPath::parse("tx-infinity-api/tx-infinity-core.yml").unwrap();
    assert_eq!(p.file_name(), "tx-infinity-core.yml");
    assert_eq!(p.parent().as_str(), "tx-infinity-api");
    assert_eq!(
        p.components().collect::<Vec<_>>(),
        ["tx-infinity-api", "tx-infinity-core.yml"]
    );
    let top = NfsPath::parse("channels.yml").unwrap();
    assert!(top.parent().is_root());
}

#[test]
fn serde_round_trip_validates() {
    let p = NfsPath::parse("a/b.yml").unwrap();
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(json, "\"a/b.yml\"");
    assert_eq!(serde_json::from_str::<NfsPath>(&json).unwrap(), p);
    assert!(serde_json::from_str::<NfsPath>("\"../a\"").is_err());
    assert!(serde_json::from_str::<NfsPath>("\"\"").is_err());
}

#[test]
fn errors_do_not_leak_inputs() {
    let secret_looking = "../hunter2-secret-token";
    let err = NfsPath::parse(secret_looking).unwrap_err();
    assert!(!err.to_string().contains("hunter2"));
    assert!(!format!("{err:?}").contains("hunter2"));
}

#[test]
fn repo_path_rejects_git_metadata_and_traversal() {
    assert!(RepoPath::parse("config/a/b.yml").is_ok());
    assert!(RepoPath::parse("data/config/x/y-sit1.yml").is_ok());
    for bad in [
        "../x",
        "/x",
        ".git/config",
        "config/.git/HEAD",
        "config/.GIT/x",
        "a\0b",
        "",
    ] {
        assert!(RepoPath::parse(bad).is_err(), "{bad:?} must be rejected");
    }
}

proptest! {
    #[test]
    fn never_yields_dotdot_prop(input in ".{0,200}") {
        if let Ok(p) = NfsPath::parse(&input) {
            prop_assert!(p.components().all(|c| c != ".." && c != "." && !c.is_empty()));
            prop_assert!(!p.as_str().contains('\0'));
            prop_assert!(!p.as_str().starts_with('/'));
            prop_assert!(!p.as_str().contains('\\'));
            prop_assert!(p.as_str().len() <= 1024);
            // normalised output re-parses to itself
            prop_assert_eq!(NfsPath::parse(p.as_str()).unwrap(), p);
        }
    }

    #[test]
    fn structured_traversal_never_accepted(
        parts in proptest::collection::vec(prop_oneof![Just(".."), Just("."), Just("a"), Just("b.yml"), Just("")], 1..8)
    ) {
        let input = parts.join("/");
        let has_dotdot = parts.contains(&"..");
        let parsed = NfsPath::parse(&input);
        if has_dotdot {
            prop_assert!(parsed.is_err());
        }
    }
}
