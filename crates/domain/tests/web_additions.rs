//! The domain additions of the `contract/08-web` change.
//!
//! CG2: `logical_file` and `tenant` on the four types a file shows up in (`FileEntry`, `Finding`, `SettingHit`, `TextHit`).
//! `CG4a`: `tier`, `project`, `baseline`, `pickup_counts` and `syncing` on `SwimlaneSummary`, and `UserRef`.
//!
//! Every addition is optional on the wire: absent when `None` or empty, so a client built from the document before the
//! change keeps working, and JSON written before the change still reads.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Debug;

use domain::{
    AgentStatus, BaselineState, BaselineStatus, ContentHash, DriftState, FileClass, FileEntry, FileKind,
    FileRole, Finding, FindingId, FindingKind, Guid, JobRef, LineRange, LogicalFile, NfsPath, PickupState,
    Role, SettingHit, SettingLocation, SettingPath, SettingValue, Severity, ShortText, SwimlaneId,
    SwimlaneSummary, SyncingInfo, TenantId, TextHit, Timestamp, User, UserId, UserRef, UserStatus, ValueType,
};
use proptest::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// The two fields of CG2, set the same way on each type.
trait Logical {
    fn set(&mut self, logical_file: Option<LogicalFile>, tenant: Option<TenantId>);
}

fn nfs(p: &str) -> NfsPath {
    NfsPath::parse(p).unwrap()
}

fn sl() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

fn file_entry() -> FileEntry {
    FileEntry {
        path: nfs("tx-infinity-api/tx-infinity-core-sit1.yml"),
        is_dir: false,
        kind: FileKind::Tenant,
        class: FileClass::Structured,
        role: FileRole::PropertySource,
        size: 12,
        hash: Some(ContentHash::from_bytes([3; 32])),
        drift: DriftState::InSync,
        format: None,
        content_withheld: false,
        attribution: None,
        severity: None,
        pickup_state: None,
        logical_file: None,
        tenant: None,
    }
}

fn finding() -> Finding {
    Finding {
        id: FindingId::from(5),
        kind: FindingKind::Drift,
        swimlane: sl(),
        path: Some(nfs("tx-infinity-api/tx-infinity-core-sit1.yml")),
        severity: Severity::High,
        rule_ids: vec![ShortText::parse("R1").unwrap()],
        message: ShortText::parse("A plain sentence from the engine.").unwrap(),
        locations: Vec::new(),
        detected_at: Timestamp::from_unix_millis(1_700_000_000_000),
        logical_file: None,
        tenant: None,
    }
}

fn setting_hit() -> SettingHit {
    SettingHit {
        swimlane: sl(),
        file: nfs("tx-infinity-api/tx-infinity-core-sit1.yml"),
        path: SettingPath::parse("a.b").unwrap(),
        value: SettingValue {
            text: "1".to_owned(),
            ty: ValueType::Int,
            flag: None,
        },
        location: SettingLocation {
            path: SettingPath::parse("a.b").unwrap(),
            lines: LineRange::new(3, 3).unwrap(),
        },
        logical_file: None,
        tenant: None,
    }
}

fn text_hit() -> TextHit {
    TextHit {
        swimlane: sl(),
        path: nfs("tx-infinity-api/tx-infinity-core-sit1.yml"),
        line: 3,
        text: "timeout: 30".to_owned(),
        ranges: vec![(0, 7)],
        logical_file: None,
        tenant: None,
    }
}

macro_rules! logical {
    ($($t:ty),*) => {$(
        impl Logical for $t {
            fn set(&mut self, logical_file: Option<LogicalFile>, tenant: Option<TenantId>) {
                self.logical_file = logical_file;
                self.tenant = tenant;
            }
        }
    )*};
}
logical!(FileEntry, Finding, SettingHit, TextHit);

/// What is true of any of the four types for these two values: absent when `None`, present as the plain string otherwise,
/// and the same value comes back when the JSON is read.
fn holds_for<T: Logical + Clone + Serialize + DeserializeOwned + PartialEq + Debug>(
    base: &T,
    logical_file: Option<&LogicalFile>,
    tenant: Option<&TenantId>,
) -> Result<(), TestCaseError> {
    let mut t = base.clone();
    t.set(logical_file.cloned(), tenant.cloned());
    let json = serde_json::to_value(&t).unwrap();
    let object = json.as_object().unwrap();
    prop_assert_eq!(object.contains_key("logical_file"), logical_file.is_some());
    prop_assert_eq!(object.contains_key("tenant"), tenant.is_some());
    if let Some(l) = logical_file {
        prop_assert_eq!(&json["logical_file"], &Value::String(l.as_str().to_owned()));
    }
    if let Some(x) = tenant {
        prop_assert_eq!(&json["tenant"], &Value::String(x.as_str().to_owned()));
    }
    prop_assert_eq!(&serde_json::from_value::<T>(json.clone()).unwrap(), &t);

    // JSON written before the change has neither key and reads as `None`, whatever the value above was.
    let mut old = json;
    old.as_object_mut().unwrap().remove("logical_file");
    old.as_object_mut().unwrap().remove("tenant");
    let mut none = base.clone();
    none.set(None, None);
    prop_assert_eq!(&serde_json::from_value::<T>(old).unwrap(), &none);
    Ok(())
}

fn logical_strategy() -> impl Strategy<Value = Option<LogicalFile>> {
    proptest::option::of(
        (
            proptest::collection::vec("[a-z][a-z0-9-]{0,8}", 0..4),
            "[a-z][a-z0-9_-]{0,12}",
            "[a-z]{1,4}",
        )
            .prop_map(|(dirs, stem, ext)| {
                let mut p = dirs.join("/");
                if !p.is_empty() {
                    p.push('/');
                }
                p.push_str(&stem);
                p.push('.');
                p.push_str(&ext);
                LogicalFile::parse(&p).unwrap()
            }),
    )
}

fn tenant_strategy() -> impl Strategy<Value = Option<TenantId>> {
    proptest::option::of("[a-z0-9][a-z0-9_-]{0,20}".prop_map(|t| TenantId::parse(&t).unwrap()))
}

proptest! {
    /// `logical_file` and `tenant` are absent from the JSON when they are `None`, present as plain strings when they are not,
    /// and JSON without them (written before the change) reads as `None`, on every type that carries them.
    #[test]
    fn logical_fields_are_absent_when_none_and_old_json_still_reads(
        logical_file in logical_strategy(),
        tenant in tenant_strategy(),
    ) {
        holds_for(&file_entry(), logical_file.as_ref(), tenant.as_ref())?;
        holds_for(&finding(), logical_file.as_ref(), tenant.as_ref())?;
        holds_for(&setting_hit(), logical_file.as_ref(), tenant.as_ref())?;
        holds_for(&text_hit(), logical_file.as_ref(), tenant.as_ref())?;
    }
}

/// The JSON a producer wrote before the change, spelled out, so the test does not depend on how the types serialise today.
#[test]
fn old_json_reads_with_no_logical_file_and_no_tenant() {
    let entry: FileEntry = serde_json::from_value(json!({
        "path": "a/b-sit1.yml", "is_dir": false, "kind": "tenant", "class": "structured", "role": "property_source",
        "size": 3, "hash": null, "drift": "in_sync", "format": null, "content_withheld": false,
        "attribution": null, "severity": null, "pickup_state": null
    }))
    .unwrap();
    assert_eq!((entry.logical_file, entry.tenant), (None, None));

    let hit: TextHit = serde_json::from_value(json!({
        "swimlane": "sit1", "path": "a/b.yml", "line": 1, "text": "x", "ranges": [[0, 1]]
    }))
    .unwrap();
    assert_eq!((hit.logical_file, hit.tenant), (None, None));

    // A producer that writes `null` says the same thing as one that writes nothing.
    let hit: TextHit = serde_json::from_value(json!({
        "swimlane": "sit1", "path": "a/b.yml", "line": 1, "text": "x", "ranges": [],
        "logical_file": null, "tenant": null
    }))
    .unwrap();
    assert_eq!((hit.logical_file, hit.tenant), (None, None));
}

/// Both values are validated as they are read, so a port never sees a raw string (S11).
#[test]
fn logical_fields_are_validated_when_read() {
    let mut json = serde_json::to_value(text_hit()).unwrap();
    json["logical_file"] = json!("../etc/passwd");
    assert!(serde_json::from_value::<TextHit>(json.clone()).is_err());
    json["logical_file"] = json!("a/b.yml");
    json["tenant"] = json!("Not A Tenant");
    assert!(serde_json::from_value::<TextHit>(json.clone()).is_err());
    json["tenant"] = json!("sit1");
    let ok: TextHit = serde_json::from_value(json).unwrap();
    assert_eq!(ok.logical_file.unwrap().as_str(), "a/b.yml");
    assert_eq!(ok.tenant.unwrap().as_str(), "sit1");
}

/// The wire names are the document's (`crates/ports/tests/web_contract_logical_file.rs` holds the document to the same two).
#[test]
fn the_two_fields_are_named_logical_file_and_tenant() {
    let mut e = file_entry();
    e.logical_file = Some(LogicalFile::parse("tx-infinity-api/tx-infinity-core.yml").unwrap());
    e.tenant = Some(TenantId::parse("sit1").unwrap());
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["logical_file"], "tx-infinity-api/tx-infinity-core.yml");
    assert_eq!(json["tenant"], "sit1");
}

// ---------------------------------------------------------------------------------------------------------------
// CG4a: the swimlane summary

fn user_id(n: u32) -> UserId {
    let tenant = Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap();
    let object = Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap();
    UserId::new(tenant, object)
}

fn user_ref(n: u32) -> UserRef {
    UserRef {
        id: user_id(n),
        display_name: ShortText::parse(&format!("Person {n}")).unwrap(),
    }
}

fn job(name: &str, uid: &str) -> JobRef {
    JobRef::new(name, uid).unwrap()
}

/// A summary the way a producer wrote it before the change: only the eight fields that existed.
fn old_summary() -> Value {
    json!({
        "id": "sit1",
        "display_name": "SIT 1",
        "tenants": ["sit1"],
        "agent": "connected",
        "last_scan_at": null,
        "base_version": null,
        "drift_counts": [["nfs_ahead", 2]],
        "finding_count": 4
    })
}

const ADDITIONS: [&str; 5] = ["tier", "project", "baseline", "pickup_counts", "syncing"];

#[test]
fn old_summary_json_reads_with_every_addition_absent() {
    let s: SwimlaneSummary = serde_json::from_value(old_summary()).unwrap();
    assert_eq!(s.tier, None);
    assert_eq!(s.project, None);
    assert_eq!(s.baseline, None);
    assert!(s.pickup_counts.is_empty());
    assert_eq!(s.syncing, None);
    assert_eq!(s.finding_count, 4);

    // Writing it back adds nothing: an absent addition stays absent, so the old document still describes the value.
    let json = serde_json::to_value(&s).unwrap();
    for key in ADDITIONS {
        assert!(json.get(key).is_none(), "{key} is not written when it is absent");
    }
    assert_eq!(json, old_summary());

    // A producer that writes `null` for the nullable ones says the same thing as one that writes nothing.
    let mut explicit = old_summary();
    for key in ["tier", "project", "baseline", "syncing"] {
        explicit[key] = Value::Null;
    }
    explicit["pickup_counts"] = json!([]);
    assert_eq!(serde_json::from_value::<SwimlaneSummary>(explicit).unwrap(), s);
}

fn summary(
    tier: Option<ShortText>,
    project: Option<ShortText>,
    baseline: Option<BaselineStatus>,
    pickup_counts: Vec<(PickupState, u32)>,
    syncing: Option<SyncingInfo>,
) -> SwimlaneSummary {
    let mut s: SwimlaneSummary = serde_json::from_value(old_summary()).unwrap();
    s.agent = AgentStatus::Connected;
    s.tier = tier;
    s.project = project;
    s.baseline = baseline;
    s.pickup_counts = pickup_counts;
    s.syncing = syncing;
    s
}

fn non_live() -> impl Strategy<Value = PickupState> {
    prop_oneof![
        Just(PickupState::LiveWithinTtl),
        Just(PickupState::NeedsNotifyOrRestart),
        Just(PickupState::NeedsConfigServerRestart),
    ]
}

fn baseline_strategy() -> impl Strategy<Value = Option<BaselineStatus>> {
    proptest::option::of(
        (
            prop_oneof![
                Just(BaselineState::Unconfirmed),
                Just(BaselineState::InProgress),
                Just(BaselineState::Confirmed)
            ],
            proptest::option::of(1u32..50),
            proptest::option::of(0i64..4_000_000_000_000),
        )
            .prop_map(|(state, by, at)| BaselineStatus {
                state,
                by: by.map(user_ref),
                at: at.map(Timestamp::from_unix_millis),
            }),
    )
}

proptest! {
    /// Each addition is written exactly when it holds something, reads back the same, and JSON without it (written before the
    /// change) reads as the default: `None`, or an empty list.
    #[test]
    fn summary_additions_default_when_absent(
        tier in proptest::option::of("[A-Za-z0-9 ]{1,12}"),
        project in proptest::option::of("[A-Za-z0-9 ]{1,20}"),
        baseline in baseline_strategy(),
        counts in proptest::collection::vec((non_live(), 1u32..10_000), 0..4),
        syncing in proptest::option::of(("[a-z][a-z0-9]{0,10}", "[a-z0-9-]{1,10}", 0i64..4_000_000_000_000)),
    ) {
        let tier = tier.map(|t| ShortText::parse(&t).unwrap());
        let project = project.map(|t| ShortText::parse(&t).unwrap());
        let syncing = syncing.map(|(name, uid, since)| SyncingInfo {
            job: job(&name, &uid),
            since: Timestamp::from_unix_millis(since),
        });
        let s = summary(tier.clone(), project.clone(), baseline.clone(), counts.clone(), syncing.clone());
        let json = serde_json::to_value(&s).unwrap();
        let present = |key: &str| json.get(key).is_some();
        prop_assert_eq!(present("tier"), tier.is_some());
        prop_assert_eq!(present("project"), project.is_some());
        prop_assert_eq!(present("baseline"), baseline.is_some());
        prop_assert_eq!(present("pickup_counts"), !counts.is_empty());
        prop_assert_eq!(present("syncing"), syncing.is_some());
        prop_assert_eq!(&serde_json::from_value::<SwimlaneSummary>(json.clone()).unwrap(), &s);

        let mut old = json;
        for key in ADDITIONS {
            old.as_object_mut().unwrap().remove(key);
        }
        let back: SwimlaneSummary = serde_json::from_value(old).unwrap();
        prop_assert_eq!(back, summary(None, None, None, Vec::new(), None));
    }
}

/// The baseline of a swimlane, by the three states the document names. `by` and `at` are always written: a new type follows
/// the document's rule that a nullable field is `null`, not absent.
#[test]
fn baseline_status_is_a_state_who_and_when() {
    assert_eq!(
        BaselineState::ALL.map(BaselineState::as_str),
        ["unconfirmed", "in_progress", "confirmed"]
    );
    for state in BaselineState::ALL {
        assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        assert_eq!(
            serde_json::from_value::<BaselineState>(json!(state.as_str())).unwrap(),
            state
        );
    }
    assert!(serde_json::from_value::<BaselineState>(json!("adopted")).is_err());

    let open = BaselineStatus {
        state: BaselineState::InProgress,
        by: None,
        at: None,
    };
    assert_eq!(
        serde_json::to_value(&open).unwrap(),
        json!({"state": "in_progress", "by": null, "at": null})
    );
    let done = BaselineStatus {
        state: BaselineState::Confirmed,
        by: Some(user_ref(1)),
        at: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
    };
    let json = serde_json::to_value(&done).unwrap();
    assert_eq!(json["state"], "confirmed");
    assert_eq!(json["by"]["display_name"], "Person 1");
    assert_eq!(json["at"], 1_700_000_000_000_i64);
    assert_eq!(serde_json::from_value::<BaselineStatus>(json).unwrap(), done);
}

#[test]
fn syncing_info_is_the_job_and_when_it_started() {
    let info = SyncingInfo {
        job: job("sync-job-1", "uid-1"),
        since: Timestamp::from_unix_millis(1_700_000_000_000),
    };
    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(
        json,
        json!({"job": {"name": "sync-job-1", "uid": "uid-1"}, "since": 1_700_000_000_000_i64})
    );
    assert_eq!(serde_json::from_value::<SyncingInfo>(json).unwrap(), info);
    // The job is validated as it is read, like every other `JobRef`.
    let bad = json!({"job": {"name": "Not A Name", "uid": "uid-1"}, "since": 1});
    assert!(serde_json::from_value::<SyncingInfo>(bad).is_err());
}

/// A `UserRef` is what another user may see of a person (Q6): the id and the display name, and nowhere an email. Made from a
/// `User`, it keeps those two and drops the rest.
#[test]
fn user_ref_is_an_id_and_a_display_name_never_an_email() {
    let user = User {
        id: user_id(7),
        email: ShortText::parse("person7@example.com").unwrap(),
        display_name: ShortText::parse("Person 7").unwrap(),
        role: Some(Role::Admin),
        status: UserStatus::Active,
        requires_approval: false,
        github_login: Some(ShortText::parse("person-seven").unwrap()),
        last_seen_at: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
    };
    let r = UserRef::from(&user);
    assert_eq!(
        (r.id.clone(), r.display_name.as_str()),
        (user.id.clone(), "Person 7")
    );
    let json = serde_json::to_value(&r).unwrap();
    let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["display_name", "id"]);
    let text = json.to_string();
    for private in ["person7@example.com", "person-seven", "admin"] {
        assert!(
            !text.contains(private),
            "{private} must not be in a UserRef: {text}"
        );
    }
    assert_eq!(serde_json::from_value::<UserRef>(json).unwrap(), r);
    // The id is validated as it is read.
    assert!(serde_json::from_value::<UserRef>(json!({"id": "not-an-id", "display_name": "x"})).is_err());
}
