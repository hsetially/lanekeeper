//! Builders for the domain values the tool tests put into the fakes.
#![allow(dead_code)]

use bytes::Bytes;
use domain::{
    Actor, Attribution, AttributionSource, ChangeKind, Confidence, DriftState, EolStyle, Evidence, FileClass,
    FileEntry, FileFormat, FileKind, FileRole, Finding, FindingId, FindingKind, LineRange, LogicalFile,
    NfsPath, PickupState, ServiceRef, ServiceState, SettingLocation, SettingPath, Severity, ShortText,
    SwimlaneId, TenantId, TextEncoding, Timestamp, ValueType,
};

pub fn sid(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

pub fn np(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

pub fn lf(s: &str) -> LogicalFile {
    LogicalFile::parse(s).unwrap()
}

pub fn sp(s: &str) -> SettingPath {
    SettingPath::parse(s).unwrap()
}

pub fn tid(s: &str) -> TenantId {
    TenantId::parse(s).unwrap()
}

pub fn st(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

pub fn hash_of(bytes: &[u8]) -> domain::ContentHash {
    ports::content_hash(bytes)
}

pub fn value(text: &str) -> domain::SettingValue {
    domain::SettingValue {
        text: text.to_owned(),
        ty: ValueType::String,
        flag: None,
    }
}

pub fn lines(a: u32, b: u32) -> LineRange {
    LineRange::new(a, b).unwrap()
}

pub fn location(path: &str, a: u32, b: u32) -> SettingLocation {
    SettingLocation {
        path: sp(path),
        lines: lines(a, b),
    }
}

pub fn file_format() -> FileFormat {
    FileFormat {
        eol: EolStyle::Lf,
        encoding: TextEncoding::Utf8,
        bom: false,
    }
}

pub fn attribution(source: AttributionSource, confidence: Confidence, login: Option<&str>) -> Attribution {
    Attribution {
        source,
        confidence,
        actor: login.map(|l| Actor::OsLogin { login: st(l) }),
        evidence: Evidence {
            executable: Some(st("vim")),
            login_identity: None,
            observed_at: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
        },
    }
}

pub fn entry(path: &str, drift: DriftState) -> FileEntry {
    FileEntry {
        path: np(path),
        is_dir: false,
        kind: FileKind::Base,
        class: FileClass::Structured,
        role: FileRole::PropertySource,
        size: 1234,
        hash: Some(hash_of(path.as_bytes())),
        drift,
        format: Some(file_format()),
        content_withheld: false,
        attribution: None,
        severity: None,
        pickup_state: None,
    }
}

pub fn service(namespace: &str, name: &str, pickup: PickupState) -> ServiceState {
    ServiceState {
        service: ServiceRef::new(namespace, name).unwrap(),
        pickup,
        pending_since: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
        live_by: None,
        notifications_enabled: true,
        cache_ttl_secs: 1200,
        pending_files: vec![np("a/b.yml")],
    }
}

pub fn finding(id: i64, kind: FindingKind, swimlane: &str, severity: Severity) -> Finding {
    Finding {
        id: FindingId::from(id),
        kind,
        swimlane: sid(swimlane),
        path: Some(np("tx-core/app.yml")),
        severity,
        rule_ids: vec![st("R1")],
        message: st("A plain sentence from the engine."),
        locations: vec![location("a.b", 3, 4)],
        detected_at: Timestamp::from_unix_millis(1_700_000_000_000),
    }
}

pub fn change(
    path: &str,
    kind: ChangeKind,
    left: Option<&str>,
    right: Option<&str>,
) -> domain::SettingChange {
    domain::SettingChange {
        path: sp(path),
        kind,
        left: left.map(value),
        right: right.map(value),
        left_location: left.map(|_| location(path, 1, 1)),
        right_location: right.map(|_| location(path, 2, 2)),
    }
}

pub fn bytes(text: &str) -> Bytes {
    Bytes::copy_from_slice(text.as_bytes())
}

/// A YAML file of `n` top-level groups, each with a few settings, so that it has lines and an outline.
pub fn big_yaml(groups: usize) -> String {
    let mut out = String::new();
    for g in 0..groups {
        out.push_str(&format!("group{g}:\n"));
        for k in 0..5 {
            out.push_str(&format!("  key{k}: value-{g}-{k}\n"));
        }
    }
    out
}

/// A key shaped like a real token, built at run time from parts so that no literal in the repository is one (A18).
pub fn fake_github_token() -> String {
    format!("{}{}", "ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8")
}
