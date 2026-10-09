//! Enum wire names are part of the contract (REST, MCP, SQL and the event stream).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{
    AgentStatus, AttributionSource, Confidence, DriftState, EolStyle, FileClass, FileKind, FileRole,
    FindingKind, MergeMode, ObservationSource, PickupState, ProposalStatus, RepoKind, Role, Severity,
    UserStatus, Via,
};
use serde::Serialize;

fn names<T: Serialize>(all: &[T]) -> Vec<String> {
    all.iter()
        .map(|v| serde_json::to_value(v).unwrap().as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn role_is_ordered_viewer_editor_operator_admin() {
    assert!(Role::Viewer < Role::Editor);
    assert!(Role::Editor < Role::Operator);
    assert!(Role::Operator < Role::Admin);
    assert!(Role::Admin >= Role::Viewer);
    assert!(Role::Operator.at_least(Role::Editor));
    assert!(!Role::Viewer.at_least(Role::Editor));
    let mut roles = vec![Role::Admin, Role::Viewer, Role::Operator, Role::Editor];
    roles.sort();
    assert_eq!(roles, [Role::Viewer, Role::Editor, Role::Operator, Role::Admin]);
}

#[test]
fn wire_names_are_stable() {
    insta::assert_json_snapshot!(
        "wire_names",
        serde_json::json!({
            "role": names(&Role::ALL),
            "user_status": names(&UserStatus::ALL),
            "file_kind": names(&FileKind::ALL),
            "file_class": names(&FileClass::ALL),
            "eol_style": names(&EolStyle::ALL),
            "merge_mode": names(&MergeMode::ALL),
            "drift_state": names(&DriftState::ALL),
            "via": names(&Via::ALL),
            "proposal_status": names(&ProposalStatus::ALL),
            "finding_kind": names(&FindingKind::ALL),
            "file_role": names(&FileRole::ALL),
            "pickup_state": names(&PickupState::ALL),
            "repo_kind": names(&RepoKind::ALL),
            "attribution_source": names(&AttributionSource::ALL),
            "confidence": names(&Confidence::ALL),
            "severity": names(&Severity::ALL),
            "observation_source": names(&ObservationSource::ALL),
            "agent_status": names(&AgentStatus::ALL),
        })
    );
}

#[test]
fn as_str_from_str_and_serde_agree() {
    fn check<T>(all: &[T])
    where
        T: Serialize + std::str::FromStr + std::fmt::Display + PartialEq + std::fmt::Debug + Copy,
        <T as std::str::FromStr>::Err: std::fmt::Debug,
    {
        for v in all {
            let s = v.to_string();
            assert_eq!(serde_json::to_value(v).unwrap().as_str().unwrap(), s);
            assert_eq!(&s.parse::<T>().unwrap(), v);
        }
        assert!("definitely-not-a-variant".parse::<T>().is_err());
    }
    check(&Role::ALL);
    check(&UserStatus::ALL);
    check(&FileKind::ALL);
    check(&FileClass::ALL);
    check(&EolStyle::ALL);
    check(&MergeMode::ALL);
    check(&DriftState::ALL);
    check(&Via::ALL);
    check(&ProposalStatus::ALL);
    check(&FindingKind::ALL);
    check(&FileRole::ALL);
    check(&PickupState::ALL);
    check(&RepoKind::ALL);
    check(&AttributionSource::ALL);
    check(&Confidence::ALL);
    check(&Severity::ALL);
    check(&ObservationSource::ALL);
    check(&AgentStatus::ALL);
}

#[test]
fn finding_kinds_cover_c1_to_c11_with_stable_codes() {
    let codes: Vec<_> = FindingKind::ALL.iter().map(|k| k.code()).collect();
    assert_eq!(
        codes,
        ["C1", "C2", "C3", "C4", "C5", "C6", "C7", "C8", "C9", "C10", "C11"]
    );
    assert_eq!(FindingKind::MissedBaseChanges.code(), "C1");
    assert_eq!(FindingKind::DuplicateKeys.code(), "C8");
    assert_eq!(FindingKind::ConfigServerRestartRequired.code(), "C11");
}

#[test]
fn confidence_and_severity_are_ordered() {
    assert!(Confidence::Low < Confidence::Medium);
    assert!(Confidence::Medium < Confidence::High);
    assert!(Confidence::High < Confidence::Certain);
    assert!(Severity::Low < Severity::Medium);
    assert!(Severity::Medium < Severity::High);
    assert!(Severity::High < Severity::Critical);
}

#[test]
fn drift_states_distinguish_sync_from_not() {
    assert!(DriftState::InSync.is_in_sync());
    assert!(DriftState::IntentionalDivergence.is_acknowledged());
    for s in [
        DriftState::GitAhead,
        DriftState::NfsAhead,
        DriftState::Conflict,
        DriftState::Unknown,
        DriftState::Untracked,
    ] {
        assert!(!s.is_in_sync());
        assert!(!s.is_acknowledged());
    }
}
