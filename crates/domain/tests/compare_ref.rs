//! S11: `CompareRef` is parsed from a string at the edge of REST and MCP.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{CompareRef, GitRef, SwimlaneId};
use proptest::prelude::*;

fn lane(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

#[test]
fn parses_all_five_forms() {
    assert_eq!(
        "nfs:sitb".parse::<CompareRef>().unwrap(),
        CompareRef::Nfs(lane("sitb"))
    );
    assert_eq!(
        "baseline:sitb".parse::<CompareRef>().unwrap(),
        CompareRef::Baseline(lane("sitb"))
    );
    assert_eq!(
        "effective:sit-2".parse::<CompareRef>().unwrap(),
        CompareRef::Effective(lane("sit-2"))
    );
    assert_eq!(
        "git:base@main".parse::<CompareRef>().unwrap(),
        CompareRef::GitBase(GitRef::parse("main").unwrap())
    );
    assert_eq!(
        "git:tenant@templates/common-docs".parse::<CompareRef>().unwrap(),
        CompareRef::GitTenant(GitRef::parse("templates/common-docs").unwrap())
    );
}

#[test]
fn git_refs_accept_branches_tags_and_commit_ids() {
    let sha1 = "0123456789abcdef0123456789abcdef01234567";
    let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    for ok in ["sit1", "v2.3.1", "templates/x", "feature/a-b_c", sha1, sha256] {
        assert!(GitRef::parse(ok).is_ok(), "{ok:?}");
    }
}

#[test]
fn rejects_malformed_refs() {
    let bad = [
        "",
        "nfs:",
        "nfs",
        "nfs:Sit B",
        "nfs:UPPER",
        "nfs:a/b",
        "baseline:",
        "git:base",
        "git:base@",
        "git:other@main",
        "git:tenant@..",
        "git:tenant@a..b",
        "git:tenant@-rf",
        "git:tenant@a@{1}",
        "git:tenant@a b",
        "git:tenant@a\\b",
        "git:tenant@a:b",
        "git:tenant@a^",
        "git:tenant@a~1",
        "git:tenant@a?",
        "git:tenant@a*",
        "git:tenant@a[",
        "git:tenant@/a",
        "git:tenant@a/",
        "git:tenant@a//b",
        "git:tenant@a.lock",
        "git:tenant@a/.b",
        "git:tenant@a.",
        "git:tenant@@",
        "git:tenant@a\0b",
        "unknown:sitb",
    ];
    for input in bad {
        assert!(input.parse::<CompareRef>().is_err(), "{input:?} must be rejected");
    }
    let long = format!("git:base@{}", "a".repeat(256));
    assert!(long.parse::<CompareRef>().is_err());
}

#[test]
fn display_matches_the_input_grammar() {
    for s in [
        "nfs:sitb",
        "baseline:sit-2",
        "effective:a1",
        "git:base@main",
        "git:tenant@templates/common-docs",
    ] {
        assert_eq!(s.parse::<CompareRef>().unwrap().to_string(), s);
    }
}

#[test]
fn serde_uses_the_string_form() {
    let r: CompareRef = "git:base@v1.0".parse().unwrap();
    assert_eq!(serde_json::to_string(&r).unwrap(), "\"git:base@v1.0\"");
    assert_eq!(
        serde_json::from_str::<CompareRef>("\"git:base@v1.0\"").unwrap(),
        r
    );
    assert!(serde_json::from_str::<CompareRef>("\"git:base@..\"").is_err());
}

#[test]
fn errors_do_not_leak_inputs() {
    let err = "nfs:Hunter2SecretValue".parse::<CompareRef>().unwrap_err();
    assert!(!err.to_string().contains("Hunter2"));
    assert!(!format!("{err:?}").contains("Hunter2"));
}

proptest! {
    #[test]
    fn roundtrip_prop(input in ".{0,80}") {
        if let Ok(r) = input.parse::<CompareRef>() {
            prop_assert_eq!(r.to_string().parse::<CompareRef>().unwrap(), r);
        }
    }

    #[test]
    fn roundtrip_for_generated_valid_refs(
        lane in "[a-z0-9][a-z0-9-]{0,20}",
        branch in "[a-z0-9][a-z0-9_-]{0,10}(/[a-z0-9][a-z0-9_-]{0,10}){0,2}",
        kind in 0usize..5
    ) {
        let text = match kind {
            0 => format!("nfs:{lane}"),
            1 => format!("baseline:{lane}"),
            2 => format!("effective:{lane}"),
            3 => format!("git:base@{branch}"),
            _ => format!("git:tenant@{branch}"),
        };
        let r: CompareRef = text.parse().unwrap();
        prop_assert_eq!(r.to_string(), text);
    }
}
