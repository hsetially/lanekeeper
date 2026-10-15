//! The domain additions of the `contract/07-mcp` change (C4, G1 to G8): the queries the MCP tools need and the ports did not
//! carry, the served file, and the two fields a draft gains. Wire shapes, bounds, that JSON written before the change still
//! reads, and that nothing prints a file's text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{
    AppName, ChannelName, CompareQuery, CompareRef, ContentHash, Draft, DraftId, DriftQuery, DriftState,
    EffectiveQuery, EffectiveSource, Expected, FileRole, LogicalFile, NfsPath, PickupState, ProposalStatus,
    ServeRequest, ServedFile, ServiceRef, SettingPath, Severity, ShortText, SwimlaneId, TenantId, Timestamp,
    UserId,
};
use serde_json::{Value, json};

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[test]
fn effective_query_names_a_tenant_and_may_name_a_channel_and_a_prefix() {
    let q = EffectiveQuery {
        file: LogicalFile::parse("tx-infinity-api/tx-infinity-core.yml").unwrap(),
        tenant: TenantId::parse("sit1").unwrap(),
        channel: Some(ChannelName::parse("remote-itm-teller").unwrap()),
        setting_prefix: Some(SettingPath::parse("a.b").unwrap()),
    };
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(
        json,
        json!({
            "file": "tx-infinity-api/tx-infinity-core.yml",
            "tenant": "sit1",
            "channel": "remote-itm-teller",
            "setting_prefix": "a.b"
        })
    );
    assert_eq!(serde_json::from_value::<EffectiveQuery>(json).unwrap(), q);

    // The channel and the prefix are optional, the file and the tenant are not, and each is validated as it is read.
    let minimal = json!({"file": "a/b.yml", "tenant": "sit1", "channel": null, "setting_prefix": null});
    let q: EffectiveQuery = serde_json::from_value(minimal).unwrap();
    assert_eq!((q.channel, q.setting_prefix), (None, None));
    for bad in [
        json!({"file": "a/b.yml", "channel": null, "setting_prefix": null}),
        json!({"tenant": "sit1", "channel": null, "setting_prefix": null}),
        json!({"file": "a/b.yml", "tenant": "Sit1", "channel": null, "setting_prefix": null}),
        json!({"file": "a/../b.yml", "tenant": "sit1", "channel": null, "setting_prefix": null}),
        json!({"file": "a/b.yml", "tenant": "sit1", "channel": "bad channel", "setting_prefix": null}),
    ] {
        assert!(
            serde_json::from_value::<EffectiveQuery>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn compare_query_is_two_typed_references_a_file_and_a_channel() {
    let q = CompareQuery {
        left: "nfs:sit1".parse::<CompareRef>().unwrap(),
        right: "git:tenant@sit1".parse::<CompareRef>().unwrap(),
        file: Some(LogicalFile::parse("a/b.yml").unwrap()),
        channel: Some(ChannelName::parse("web").unwrap()),
    };
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(json["left"], "nfs:sit1");
    assert_eq!(json["right"], "git:tenant@sit1");
    assert_eq!(serde_json::from_value::<CompareQuery>(json).unwrap(), q);
    // A reference that is not one of the five forms is refused when read, so a port never sees a raw string.
    let bad = json!({"left": "nfs:", "right": "nfs:sit1", "file": null, "channel": null});
    assert!(serde_json::from_value::<CompareQuery>(bad).is_err());
}

#[test]
fn drift_query_filters_by_state_and_folder() {
    let q = DriftQuery {
        state: Some(DriftState::NfsAhead),
        prefix: Some(NfsPath::parse("tx-infinity-api").unwrap()),
    };
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(json, json!({"state": "nfs_ahead", "prefix": "tx-infinity-api"}));
    assert_eq!(serde_json::from_value::<DriftQuery>(json).unwrap(), q);
    let all = DriftQuery {
        state: None,
        prefix: None,
    };
    assert_eq!(
        serde_json::to_value(&all).unwrap(),
        json!({"state": null, "prefix": null})
    );
    assert!(serde_json::from_value::<DriftQuery>(json!({"state": "pending", "prefix": null})).is_err());
    assert!(serde_json::from_value::<DriftQuery>(json!({"state": null, "prefix": "../x"})).is_err());
}

fn served() -> ServedFile {
    ServedFile {
        swimlane: SwimlaneId::parse("sit1").unwrap(),
        application: AppName::parse("tx-infinity-api").unwrap(),
        tenant: TenantId::parse("sit1").unwrap(),
        channel: None,
        file: NfsPath::parse("tx-infinity-core.yml").unwrap(),
        role: FileRole::PropertySource,
        hash: ContentHash::from_bytes([7; 32]),
        content_withheld: false,
        text: Some("password: hunter2-in-a-test\n".to_owned()),
        sources: vec![EffectiveSource {
            path: NfsPath::parse("tx-infinity-api/tx-infinity-core-sit1.yml").unwrap(),
            rank: 1,
        }],
        unresolved_placeholders: vec![ShortText::parse("${SOME_ENV}").unwrap()],
    }
}

/// The wire names are the REST `ServedFile`'s (`crates/ports/tests/openapi_conventions.rs` holds the document to them), and a
/// file with no text still has the key: the document requires it, and `null` says there is none.
#[test]
fn served_file_wire_shape_has_every_field_and_null_for_no_text() {
    let mut f = served();
    let expect = [
        "application",
        "channel",
        "content_withheld",
        "file",
        "hash",
        "role",
        "sources",
        "swimlane",
        "tenant",
        "text",
        "unresolved_placeholders",
    ];
    let json = serde_json::to_value(&f).unwrap();
    assert_eq!(keys(&json), expect);
    assert_eq!(json["sources"][0]["rank"], 1);
    assert_eq!(serde_json::from_value::<ServedFile>(json).unwrap(), f);

    f.text = None;
    f.content_withheld = true;
    f.channel = Some(ChannelName::parse("web").unwrap());
    let json = serde_json::to_value(&f).unwrap();
    assert_eq!(keys(&json), expect, "the keys do not depend on the values");
    assert_eq!(json["text"], Value::Null);
    assert_eq!(json["channel"], "web");
    assert_eq!(serde_json::from_value::<ServedFile>(json).unwrap(), f);
}

/// Served text is config text: it may hold a value the secret scan has not flagged (S10, S21). Debug names the file and its
/// size and nothing of its content, like `ChangePreview` and `PreviewedChange`.
#[test]
fn served_file_debug_prints_no_text() {
    let f = served();
    let debug = format!("{f:?}");
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(!debug.contains("password"), "{debug}");
    assert!(
        debug.contains("ServedFile") && debug.contains("tx-infinity-core.yml"),
        "{debug}"
    );
    // The placeholder names are not text of the file's values, but they are not needed in a log either.
    assert!(!debug.contains("SOME_ENV"), "{debug}");
}

#[test]
fn served_file_answers_a_serve_request() {
    // The request and the answer name the same five things, so an implementation cannot answer another question.
    let req = ServeRequest {
        application: AppName::parse("tx-infinity-api").unwrap(),
        tenant: TenantId::parse("sit1").unwrap(),
        channel: None,
        file: NfsPath::parse("tx-infinity-core.yml").unwrap(),
    };
    let f = served();
    assert_eq!(
        (&f.application, &f.tenant, &f.channel, &f.file),
        (&req.application, &req.tenant, &req.channel, &req.file)
    );
}

fn draft() -> Draft {
    Draft {
        id: DraftId::parse("draft-1").unwrap(),
        author: "72f988bf-86f1-41af-91ab-2d7cd011db47:0b6c3e0e-5a1d-4a52-8f55-3f9d6f0a7f11"
            .parse::<UserId>()
            .unwrap(),
        swimlane: SwimlaneId::parse("sit1").unwrap(),
        path: NfsPath::parse("tx-infinity-api/tx-infinity-core-sit1.yml").unwrap(),
        expected: Expected::Absent,
        new_hash: ContentHash::from_bytes([1; 32]),
        hunks: Vec::new(),
        setting_changes: Vec::new(),
        status: ProposalStatus::Draft,
        expires_at: Timestamp::from_unix_millis(1_700_000_000_000),
        severity: None,
        effects: Vec::new(),
    }
}

/// A draft written before `severity` and `effects` existed still reads, with no severity and no effects, and a draft that has
/// neither writes neither, so a client built from the old document sees the same JSON as before.
#[test]
fn a_draft_without_the_new_fields_reads_and_writes_as_before() {
    let old = serde_json::to_value(draft()).unwrap();
    assert!(
        old.get("severity").is_none() && old.get("effects").is_none(),
        "{old}"
    );
    assert_eq!(
        keys(&old),
        [
            "author",
            "expected",
            "expires_at",
            "hunks",
            "id",
            "new_hash",
            "path",
            "setting_changes",
            "status",
            "swimlane"
        ]
    );
    let back: Draft = serde_json::from_value(old).unwrap();
    assert_eq!(back, draft());
    assert_eq!((back.severity, back.effects.len()), (None, 0));
}

#[test]
fn a_draft_carries_its_severity_and_the_pickup_state_of_each_service() {
    let svc = ServiceRef::new("csp", "tx-infinity-api").unwrap();
    let mut d = draft();
    d.severity = Some(Severity::High);
    d.effects = vec![(svc.clone(), PickupState::NeedsNotifyOrRestart)];
    let json = serde_json::to_value(&d).unwrap();
    assert_eq!(json["severity"], "high");
    // A pair, like `SwimlaneSummary::drift_counts`: the document spells it `prefixItems`.
    assert_eq!(
        json["effects"],
        json!([[{"namespace": "csp", "name": "tx-infinity-api"}, "needs_notify_or_restart"]])
    );
    let back: Draft = serde_json::from_value(json).unwrap();
    assert_eq!(back, d);
    assert_eq!(back.effects[0].0, svc);
    // `severity: null` and `effects: []` read as well, for a producer that writes them.
    let mut explicit = serde_json::to_value(draft()).unwrap();
    explicit["severity"] = Value::Null;
    explicit["effects"] = json!([]);
    assert_eq!(serde_json::from_value::<Draft>(explicit).unwrap(), draft());
    // A pair with a state this version does not know is refused rather than guessed.
    let mut bad = serde_json::to_value(&d).unwrap();
    bad["effects"] = json!([[{"namespace": "csp", "name": "tx-infinity-api"}, "pending_restart"]]);
    assert!(serde_json::from_value::<Draft>(bad).is_err());
}
