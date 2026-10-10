//! Behaviour of the shared read/write DTOs that the services in `docs/interfaces.md` exchange.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use domain::{
    AuditId, ContentHash, Cursor, Expected, NfsPath, Page, Paged, Role, ServiceRef, SwimlaneId, Timestamp,
    User, UserId, UserStatus, VersionLabel, WriteOutcome,
};

fn user(role: Option<Role>, status: UserStatus) -> User {
    User {
        id: "72f988bf-86f1-41af-91ab-2d7cd011db47:0b6c3e0e-5a1d-4a52-8f55-3f9d6f0a7f11"
            .parse::<UserId>()
            .unwrap(),
        email: "a@example.com".parse().unwrap(),
        display_name: "A".parse().unwrap(),
        role,
        status,
        requires_approval: false,
        github_login: None,
        last_seen_at: None,
    }
}

#[test]
fn page_limit_is_bounded_to_500() {
    assert!(Page::new(1, None).is_ok());
    assert!(Page::new(500, None).is_ok());
    assert!(Page::new(0, None).is_err());
    assert!(Page::new(501, None).is_err());
    assert_eq!(Page::clamped(0).limit(), 1);
    assert_eq!(Page::clamped(10_000).limit(), 500);
    assert_eq!(Page::default().limit(), Page::DEFAULT_LIMIT);
}

#[test]
fn cursor_is_opaque_and_bounded() {
    assert!(Cursor::parse("abc_DEF-123=").is_ok());
    for bad in ["", "a b", "a/b", "a\n", &"x".repeat(513)] {
        assert!(Cursor::parse(bad).is_err(), "{bad:?}");
    }
    let page = Page::new(10, Some(Cursor::parse("abc").unwrap())).unwrap();
    assert_eq!(page.cursor().map(Cursor::as_str), Some("abc"));
}

#[test]
fn paged_serialises_items_and_next() {
    let p = Paged {
        items: vec![1_u32, 2],
        next: Some(Cursor::parse("n1").unwrap()),
    };
    assert_eq!(
        serde_json::to_string(&p).unwrap(),
        "{\"items\":[1,2],\"next\":\"n1\"}"
    );
    let last: Paged<u32> = Paged::last(vec![]);
    assert!(last.next.is_none());
}

#[test]
fn only_active_users_pass_a_role_check() {
    assert!(user(Some(Role::Editor), UserStatus::Active).allows(Role::Viewer));
    assert!(user(Some(Role::Editor), UserStatus::Active).allows(Role::Editor));
    assert!(!user(Some(Role::Editor), UserStatus::Active).allows(Role::Operator));
    assert!(!user(Some(Role::Admin), UserStatus::Pending).allows(Role::Viewer));
    assert!(!user(Some(Role::Admin), UserStatus::Disabled).allows(Role::Viewer));
    assert!(!user(None, UserStatus::Active).allows(Role::Viewer));
}

#[test]
fn write_outcome_wire_shape() {
    let applied = WriteOutcome::Applied {
        hash: Some(ContentHash::from_bytes([1; 32])),
        audit_id: AuditId::from(5),
    };
    let v = serde_json::to_value(&applied).unwrap();
    assert_eq!(v["outcome"], "applied");
    assert_eq!(v["audit_id"], 5);
    let conflict = WriteOutcome::Conflict { current: None };
    assert_eq!(serde_json::to_value(&conflict).unwrap()["outcome"], "conflict");
    assert!(!conflict.is_success());
    assert!(applied.is_success());
    assert!(WriteOutcome::ProposalCreated { id: 3.into() }.is_success());
    assert!(!WriteOutcome::Locked { until: None }.is_success());
    for out in [
        applied,
        conflict,
        WriteOutcome::Locked {
            until: Some(Timestamp::from_unix_millis(1)),
        },
    ] {
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(serde_json::from_str::<WriteOutcome>(&json).unwrap(), out);
    }
}

#[test]
fn expected_state_has_no_blind_write_variant() {
    // `Expected` has exactly two shapes: the file must be absent, or it must have this hash.
    let absent = Expected::Absent;
    let hash = Expected::Hash {
        hash: ContentHash::from_bytes([2; 32]),
    };
    assert_ne!(absent, hash);
    assert!(absent.matches(None));
    assert!(!absent.matches(Some(&ContentHash::from_bytes([2; 32]))));
    assert!(hash.matches(Some(&ContentHash::from_bytes([2; 32]))));
    assert!(!hash.matches(None));
    assert!(!hash.matches(Some(&ContentHash::from_bytes([3; 32]))));
    let _unused: (Bytes, NfsPath, SwimlaneId, ServiceRef) = (
        Bytes::new(),
        NfsPath::parse("a").unwrap(),
        SwimlaneId::parse("a").unwrap(),
        ServiceRef::new("ns", "svc").unwrap(),
    );
}

#[test]
fn service_ref_validates_kubernetes_names() {
    assert!(ServiceRef::new("csp", "tx-infinity-api").is_ok());
    for (ns, name) in [
        ("", "a"),
        ("a", ""),
        ("A", "a"),
        ("a", "a b"),
        ("a", "-a"),
        ("a/b", "c"),
    ] {
        assert!(ServiceRef::new(ns, name).is_err(), "{ns:?}/{name:?}");
    }
}

#[test]
fn version_label_formats_as_tag_plus_distance() {
    let commit = domain::CommitId::parse(&"a".repeat(40)).unwrap();
    let tagged = VersionLabel {
        tag: Some(domain::GitRef::parse("v2.3.1").unwrap()),
        distance: 4,
        commit: commit.clone(),
    };
    assert_eq!(tagged.to_string(), "v2.3.1 + 4 commits");
    let exact = VersionLabel {
        tag: Some(domain::GitRef::parse("v2.3.1").unwrap()),
        distance: 0,
        commit: commit.clone(),
    };
    assert_eq!(exact.to_string(), "v2.3.1");
    let one = VersionLabel {
        tag: Some(domain::GitRef::parse("v2.3.1").unwrap()),
        distance: 1,
        commit: commit.clone(),
    };
    assert_eq!(one.to_string(), "v2.3.1 + 1 commit");
    let untagged = VersionLabel {
        tag: None,
        distance: 0,
        commit,
    };
    assert_eq!(untagged.to_string(), "aaaaaaa");
}

#[test]
fn tree_index_is_sorted_and_searchable() {
    use domain::{CommitId, RepoKind, RepoPath, TreeEntry, TreeIndex};
    let entry = |p: &str, n: u8| TreeEntry {
        path: RepoPath::parse(p).unwrap(),
        git_oid: CommitId::parse(&format!("{n:02x}").repeat(20)).unwrap(),
        sha256: ContentHash::from_bytes([n; 32]),
        size: 1,
    };
    let idx = TreeIndex::new(
        RepoKind::Base,
        CommitId::parse(&"a".repeat(40)).unwrap(),
        vec![
            entry("config/z.yml", 3),
            entry("config/a.yml", 1),
            entry("config/m/b.yml", 2),
            entry("config/a.yml", 9),
        ],
    );
    let paths: Vec<_> = idx.entries().iter().map(|e| e.path.as_str().to_owned()).collect();
    assert_eq!(paths, ["config/a.yml", "config/m/b.yml", "config/z.yml"]);
    assert!(idx.get(&RepoPath::parse("config/m/b.yml").unwrap()).is_some());
    assert!(idx.get(&RepoPath::parse("config/nope.yml").unwrap()).is_none());
}

#[test]
fn attribution_is_only_ever_upgraded() {
    use domain::{Attribution, AttributionSource, Confidence};
    let unknown = Attribution::unknown();
    let mut better = Attribution::unknown();
    better.source = AttributionSource::SentinelLogin;
    better.confidence = Confidence::High;
    assert!(unknown.may_upgrade_to(&better));
    assert!(!better.may_upgrade_to(&unknown));
    assert!(!better.may_upgrade_to(&better));
}

#[test]
fn serve_request_validates_every_field() {
    use domain::{AppName, ChannelName};
    assert!(AppName::parse("tx-infinity-api").is_ok());
    assert!(ChannelName::parse("remote-itm-teller").is_ok());
    for bad in ["", ".hidden", "a/b", "a b", "a\n", &"x".repeat(129)] {
        assert!(AppName::parse(bad).is_err(), "{bad:?}");
        assert!(ChannelName::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn id_errors_do_not_leak_inputs() {
    let probe = "Hunter2-SECRET";
    let errs = [
        format!("{:?}", domain::SwimlaneId::parse(probe).unwrap_err()),
        format!("{:?}", domain::TenantId::parse(probe).unwrap_err()),
        format!("{:?}", domain::ContentHash::parse(probe).unwrap_err()),
        format!("{:?}", domain::CommitId::parse(probe).unwrap_err()),
        format!("{:?}", domain::Guid::parse(probe).unwrap_err()),
        format!("{:?}", domain::GitRef::parse(&format!("{probe}..")).unwrap_err()),
        format!(
            "{:?}",
            domain::SettingPath::parse(&format!("{probe}\0")).unwrap_err()
        ),
        format!(
            "{:?}",
            domain::ShortText::parse(&format!("{probe}\n")).unwrap_err()
        ),
    ];
    for e in errs {
        assert!(!e.contains("Hunter2"), "{e}");
    }
}

#[test]
fn doc_path_lives_under_git_or_uploads() {
    use domain::DocPath;
    assert_eq!(
        DocPath::parse("/git/templates/common-docs/guide.md")
            .unwrap()
            .as_str(),
        "/git/templates/common-docs/guide.md"
    );
    assert_eq!(
        DocPath::parse("/uploads//a/./b.md").unwrap().as_str(),
        "/uploads/a/b.md"
    );
    for bad in [
        "",
        "/",
        "/git/",
        "/uploads/",
        "git/x",
        "/etc/passwd",
        "/git/../x",
        "/git/a/.git/x",
        "/uploads/a\0b",
        "/uploads/a\\b",
    ] {
        assert!(DocPath::parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn scan_delta_payload_is_the_sum_of_file_bytes_and_capped_at_3_mib() {
    use domain::{ScanDelta, ScanEntry};
    let entry = |path: &str, bytes: Option<&'static [u8]>| ScanEntry {
        path: NfsPath::parse(path).unwrap(),
        hash: ContentHash::from_bytes([1; 32]),
        size: 0,
        mtime: Timestamp::from_unix_millis(0),
        observed_at: Timestamp::from_unix_millis(0),
        denied: bytes.is_none(),
        bytes: bytes.map(Bytes::from_static),
    };
    let delta = ScanDelta {
        seq: 1,
        base_root: None,
        new_root: ContentHash::from_bytes([2; 32]),
        entries: vec![
            entry("a.yml", Some(b"abc")),
            entry("b.env", None),
            entry("c.yml", Some(b"de")),
        ],
        removed: vec![],
        skipped: vec![],
        during_job: None,
        more: false,
        part: 0,
        gap: None,
    };
    assert_eq!(delta.payload_bytes(), 5);
    assert_eq!(ScanDelta::MAX_BYTES, 3 * 1024 * 1024);
}
