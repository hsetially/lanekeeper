//! The write-path additions of the `contract/06-writes` change (G1 to G3, G6): wire shapes, bounds, and that JSON
//! written before the change still reads.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{
    AppName, AuditId, ChangeEol, ChangePreview, ContentHash, IdError, IssueKind, NfsPath, NotifyRequest,
    PrRequest, PrTarget, PreviewId, PreviewScope, PreviewWarning, RejectReason, ShortText, SwimlaneId,
    TenantId, Timestamp, ValidationIssue, WriteOutcome,
};

fn issue(kind: IssueKind, line: Option<u32>, rule: Option<&str>) -> ValidationIssue {
    ValidationIssue {
        kind,
        line: line.map(|l| std::num::NonZeroU32::new(l).unwrap()),
        rule: rule.map(|r| ShortText::parse(r).unwrap()),
    }
}

#[test]
fn change_eol_has_exactly_lf_and_crlf() {
    assert_eq!(serde_json::to_value(ChangeEol::Lf).unwrap(), "lf");
    assert_eq!(serde_json::to_value(ChangeEol::Crlf).unwrap(), "crlf");
    assert_eq!(
        serde_json::from_str::<ChangeEol>("\"crlf\"").unwrap(),
        ChangeEol::Crlf
    );
    // `mixed` is a state a file can be in, never a style to convert to.
    assert!(serde_json::from_str::<ChangeEol>("\"mixed\"").is_err());
    assert_eq!(ChangeEol::ALL.len(), 2);
}

#[test]
fn pr_target_wire_shape_is_tagged_by_kind() {
    let tenant = PrTarget::Tenant {
        branch: TenantId::parse("sit1").unwrap(),
    };
    assert_eq!(
        serde_json::to_value(&tenant).unwrap(),
        serde_json::json!({"kind": "tenant", "branch": "sit1"})
    );
    assert_eq!(
        serde_json::to_value(&PrTarget::Base).unwrap(),
        serde_json::json!({"kind": "base"})
    );
    for t in [tenant, PrTarget::Base] {
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<PrTarget>(&json).unwrap(), t);
    }
}

#[test]
fn pr_target_rejects_a_branch_that_is_not_a_tenant_id() {
    for bad in [
        r#"{"kind":"tenant","branch":"../main"}"#,
        r#"{"kind":"tenant","branch":"Sit1"}"#,
        r#"{"kind":"tenant"}"#,
        r#"{"kind":"tenant","branch":"sit1","extra":1}"#,
        r#"{"kind":"other"}"#,
    ] {
        assert!(serde_json::from_str::<PrTarget>(bad).is_err(), "{bad}");
    }
}

#[test]
fn preview_id_is_a_bounded_token() {
    assert!(PreviewId::parse("AbC-123_xyz.def").is_ok());
    assert_eq!(PreviewId::parse("").unwrap_err(), IdError::Empty);
    assert_eq!(PreviewId::parse(&"a".repeat(129)).unwrap_err(), IdError::TooLong);
    assert_eq!(PreviewId::parse("has space").unwrap_err(), IdError::BadChar);
    assert_eq!(PreviewId::parse("semi;colon").unwrap_err(), IdError::BadChar);
    let id = PreviewId::parse("tok.en").unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"tok.en\"");
    assert!(serde_json::from_str::<PreviewId>("\"bad id\"").is_err());
}

#[test]
fn notify_request_is_capped_like_a_pr() {
    assert_eq!(NotifyRequest::MAX_PATHS, 100);
    assert_eq!(NotifyRequest::MAX_PATHS, PrRequest::MAX_PATHS);
}

#[test]
fn new_reject_reasons_have_stable_wire_names() {
    assert_eq!(
        serde_json::to_value(RejectReason::SecretDetected).unwrap(),
        "secret_detected"
    );
    assert_eq!(
        serde_json::to_value(RejectReason::ParseError).unwrap(),
        "parse_error"
    );
    // The old names are unchanged.
    for (r, name) in [
        (RejectReason::Denied, "denied"),
        (RejectReason::TooLarge, "too_large"),
        (RejectReason::InvalidContent, "invalid_content"),
        (RejectReason::UnknownSwimlane, "unknown_swimlane"),
        (RejectReason::AgentUnavailable, "agent_unavailable"),
        (RejectReason::NotAllowed, "not_allowed"),
    ] {
        assert_eq!(serde_json::to_value(r).unwrap(), name);
    }
}

#[test]
fn validation_issue_carries_a_place_and_a_rule_but_never_text() {
    let secret = issue(IssueKind::SecretMatch, Some(12), Some("aws_access_key"));
    assert_eq!(
        serde_json::to_value(&secret).unwrap(),
        serde_json::json!({"kind": "secret_match", "line": 12, "rule": "aws_access_key"})
    );
    let dup = issue(IssueKind::DuplicateKey, Some(4), None);
    assert_eq!(
        serde_json::to_value(&dup).unwrap(),
        serde_json::json!({"kind": "duplicate_key", "line": 4})
    );
    let parse = issue(IssueKind::ParseError, None, None);
    assert_eq!(
        serde_json::to_value(&parse).unwrap(),
        serde_json::json!({"kind": "parse_error"})
    );
    for i in [secret, dup, parse] {
        let json = serde_json::to_string(&i).unwrap();
        assert_eq!(serde_json::from_str::<ValidationIssue>(&json).unwrap(), i);
    }
    assert!(serde_json::from_str::<ValidationIssue>(r#"{"kind":"secret_match","text":"AKIA..."}"#).is_err());
}

#[test]
fn a_line_is_one_based_and_zero_is_refused() {
    let one: ValidationIssue = serde_json::from_str(r#"{"kind":"duplicate_key","line":1}"#).unwrap();
    assert_eq!(one.line.map(std::num::NonZeroU32::get), Some(1));
    let largest: ValidationIssue =
        serde_json::from_str(r#"{"kind":"duplicate_key","line":4294967295}"#).unwrap();
    assert_eq!(largest.line.map(std::num::NonZeroU32::get), Some(u32::MAX));
    assert!(
        serde_json::from_str::<ValidationIssue>(r#"{"kind":"duplicate_key","line":0}"#).is_err(),
        "lines count from 1 (the OpenAPI minimum)"
    );
    assert!(serde_json::from_str::<ValidationIssue>(r#"{"kind":"duplicate_key","line":-1}"#).is_err());
}

#[test]
fn outcomes_written_before_the_change_still_read() {
    let applied: WriteOutcome =
        serde_json::from_str(r#"{"outcome":"applied","hash":null,"audit_id":5}"#).unwrap();
    assert_eq!(
        applied,
        WriteOutcome::Applied {
            hash: None,
            audit_id: AuditId::from(5),
            warnings: Vec::new()
        }
    );
    let rejected: WriteOutcome =
        serde_json::from_str(r#"{"outcome":"rejected","reason":"not_allowed"}"#).unwrap();
    assert_eq!(
        rejected,
        WriteOutcome::Rejected {
            reason: RejectReason::NotAllowed,
            issues: Vec::new()
        }
    );
}

#[test]
fn empty_issue_lists_are_not_written() {
    // So that every outcome that carries no detail keeps the bytes it had before.
    let applied = WriteOutcome::Applied {
        hash: Some(ContentHash::from_bytes([1; 32])),
        audit_id: AuditId::from(5),
        warnings: Vec::new(),
    };
    let v = serde_json::to_value(&applied).unwrap();
    assert!(v.get("warnings").is_none(), "{v}");
    let rejected = WriteOutcome::Rejected {
        reason: RejectReason::Denied,
        issues: Vec::new(),
    };
    let v = serde_json::to_value(&rejected).unwrap();
    assert!(v.get("issues").is_none(), "{v}");
}

#[test]
fn outcomes_with_detail_round_trip() {
    let applied = WriteOutcome::Applied {
        hash: None,
        audit_id: AuditId::from(9),
        warnings: vec![issue(IssueKind::DuplicateKey, Some(3), None)],
    };
    let rejected = WriteOutcome::Rejected {
        reason: RejectReason::SecretDetected,
        issues: vec![
            issue(IssueKind::SecretMatch, Some(2), Some("github_token")),
            issue(IssueKind::SecretMatch, Some(40), Some("high_entropy")),
        ],
    };
    for out in [applied, rejected] {
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(serde_json::from_str::<WriteOutcome>(&json).unwrap(), out);
    }
    // A rejection with issues is still not a success, and an applied write with warnings still is.
    assert!(
        WriteOutcome::Applied {
            hash: None,
            audit_id: AuditId::from(1),
            warnings: vec![issue(IssueKind::DuplicateKey, None, None)]
        }
        .is_success()
    );
}

#[test]
fn change_preview_debug_never_prints_values() {
    let preview = ChangePreview {
        preview_id: PreviewId::parse("tok").unwrap(),
        expires_at: Timestamp::from_unix_millis(1),
        scope: PreviewScope::Application {
            app: AppName::parse("tx-infinity-api").unwrap(),
        },
        warnings: vec![PreviewWarning::NeverRead {
            suffix: TenantId::parse("sit9").unwrap(),
        }],
        changes: vec![domain::PreviewedChange {
            change: domain::SettingChange {
                path: domain::SettingPath::parse("db.password").unwrap(),
                kind: domain::ChangeKind::Changed,
                left: Some(domain::SettingValue {
                    text: "hunter2-secret".to_owned(),
                    ty: domain::ValueType::String,
                    flag: None,
                }),
                right: None,
                left_location: None,
                right_location: None,
            },
            flagged: false,
        }],
        changes_total: 1,
        effective_changed: true,
    };
    let shown = format!("{preview:?}");
    assert!(!shown.contains("hunter2"), "{shown}");
    assert!(shown.contains("changes"), "{shown}");
}

#[test]
fn change_preview_wire_shape() {
    let preview = ChangePreview {
        preview_id: PreviewId::parse("tok").unwrap(),
        expires_at: Timestamp::from_unix_millis(600_000),
        scope: PreviewScope::Folder {
            path: NfsPath::parse("new-folder").unwrap(),
        },
        warnings: vec![
            PreviewWarning::ForksFromBase,
            PreviewWarning::NewFolderNeedsConfigServerRestart,
        ],
        changes: Vec::new(),
        changes_total: 0,
        effective_changed: false,
    };
    let v = serde_json::to_value(&preview).unwrap();
    assert_eq!(v["preview_id"], "tok");
    assert_eq!(
        v["scope"],
        serde_json::json!({"kind": "folder", "path": "new-folder"})
    );
    assert_eq!(
        v["warnings"],
        serde_json::json!([{"kind": "forks_from_base"}, {"kind": "new_folder_needs_config_server_restart"}])
    );
    assert_eq!(
        serde_json::to_value(PreviewScope::EveryService).unwrap(),
        serde_json::json!({"kind": "every_service"})
    );
    let json = serde_json::to_string(&preview).unwrap();
    assert_eq!(serde_json::from_str::<ChangePreview>(&json).unwrap(), preview);
    assert_eq!(ChangePreview::TTL_SECS, 600);
}

#[test]
fn requests_gain_their_new_fields_as_options() {
    // Compile-time shape: every new field is optional, so a caller that does not care passes `None`.
    let s = SwimlaneId::parse("sit1").unwrap();
    let p = NfsPath::parse("a.yml").unwrap();
    let _edit = domain::EditRequest {
        swimlane: s.clone(),
        path: p.clone(),
        expected: domain::Expected::Absent,
        content: bytes::Bytes::new(),
        change_eol: None,
    };
    let _revert = domain::RevertRequest {
        swimlane: s.clone(),
        path: p.clone(),
        expected: domain::Expected::Absent,
        to: ContentHash::from_bytes([0; 32]),
        change_eol: Some(ChangeEol::Lf),
    };
    let _upload = domain::UploadRequest {
        swimlane: s.clone(),
        path: p.clone(),
        expected: domain::Expected::Absent,
        content: bytes::Bytes::new(),
        preview: None,
    };
    let _delete = domain::DeleteRequest {
        swimlane: s.clone(),
        path: p.clone(),
        expected: ContentHash::from_bytes([0; 32]),
        preview: Some(PreviewId::parse("tok").unwrap()),
    };
    let _pr = PrRequest {
        swimlane: s.clone(),
        paths: vec![p.clone()],
        title: ShortText::parse("t").unwrap(),
        body: None,
        target: Some(PrTarget::Base),
    };
    let _notify = NotifyRequest {
        swimlane: s,
        paths: vec![p],
    };
}

#[test]
fn a_previewed_change_carries_the_flag_next_to_the_change() {
    let masked = domain::PreviewedChange {
        change: domain::SettingChange {
            path: domain::SettingPath::parse("aws.key").unwrap(),
            kind: domain::ChangeKind::Added,
            left: None,
            right: Some(domain::SettingValue {
                text: "[REDACTED: 20 chars, aws_access_key]".to_owned(),
                ty: domain::ValueType::String,
                flag: None,
            }),
            left_location: None,
            right_location: None,
        },
        flagged: true,
    };
    let v = serde_json::to_value(&masked).unwrap();
    assert_eq!(v["flagged"], true);
    assert_eq!(
        v["change"]["right"]["text"],
        "[REDACTED: 20 chars, aws_access_key]"
    );
    let json = serde_json::to_string(&masked).unwrap();
    assert_eq!(
        serde_json::from_str::<domain::PreviewedChange>(&json).unwrap(),
        masked
    );
    // The flag is not optional: an entry that does not say is refused, not read as unflagged.
    let without = serde_json::json!({"change": v["change"]}).to_string();
    assert!(serde_json::from_str::<domain::PreviewedChange>(&without).is_err());
    let extra = serde_json::json!({"change": v["change"], "flagged": false, "reveal": "x"}).to_string();
    assert!(serde_json::from_str::<domain::PreviewedChange>(&extra).is_err());
}

#[test]
fn a_previewed_change_debug_never_prints_values() {
    // A fake value on each side: Debug names the setting, the kind and the flag, and nothing of either value.
    let value = |text: &str| domain::SettingValue {
        text: text.to_owned(),
        ty: domain::ValueType::String,
        flag: None,
    };
    let entry = domain::PreviewedChange {
        change: domain::SettingChange {
            path: domain::SettingPath::parse("db.password").unwrap(),
            kind: domain::ChangeKind::Changed,
            left: Some(value("hunter2-left-fake")),
            right: Some(value("hunter2-right-fake")),
            left_location: None,
            right_location: None,
        },
        flagged: true,
    };
    for shown in [
        format!("{entry:?}"),
        format!("{entry:#?}"),
        format!("{:?}", vec![entry]),
    ] {
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(
            shown.contains("db.password") && shown.contains("Changed") && shown.contains("flagged: true"),
            "{shown}"
        );
    }
}
