//! The domain additions of the `contract/08-web` change (CG2): `logical_file` and `tenant` on the four types a file shows up
//! in (`FileEntry`, `Finding`, `SettingHit`, `TextHit`). Both are optional on the wire: absent when `None`, so a client built
//! from the document before the change keeps working, and JSON written before the change still reads.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Debug;

use domain::{
    ContentHash, DriftState, FileClass, FileEntry, FileKind, FileRole, Finding, FindingId, FindingKind,
    LineRange, LogicalFile, NfsPath, SettingHit, SettingLocation, SettingPath, SettingValue, Severity,
    ShortText, SwimlaneId, TenantId, TextHit, Timestamp, ValueType,
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
