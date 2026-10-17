//! `DomainEvent` wire shapes are a contract: SSE clients and the outbox both depend on them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{
    ContentHash, DomainEvent, JobRef, NfsPath, ObservationSource, PrJobState, PrState, ProposalStatus,
    RepoKind, ServiceRef, SwimlaneId,
};

fn lane() -> SwimlaneId {
    SwimlaneId::parse("sitb").unwrap()
}

fn path() -> NfsPath {
    NfsPath::parse("tx-infinity-api/tx-infinity-core-sit1.yml").unwrap()
}

fn hash() -> ContentHash {
    ContentHash::from_bytes([0xab; 32])
}

fn all_events() -> Vec<DomainEvent> {
    vec![
        DomainEvent::FileObserved {
            swimlane: lane(),
            path: path(),
            hash: hash(),
            source: ObservationSource::Scan,
        },
        DomainEvent::DriftChanged {
            swimlane: lane(),
            paths: vec![path()],
        },
        DomainEvent::FindingsChanged { swimlane: lane() },
        DomainEvent::PendingRestartChanged {
            swimlane: lane(),
            services: vec![ServiceRef::new("csp", "tx-infinity-api").unwrap()],
        },
        DomainEvent::ProposalChanged {
            id: 7.into(),
            status: ProposalStatus::Pending,
        },
        DomainEvent::AgentStatusChanged {
            swimlane: lane(),
            status: domain::AgentStatus::Connected,
        },
        DomainEvent::GitHeadMoved {
            repo: RepoKind::Tenant,
            branch: domain::GitRef::parse("sit1").unwrap(),
            from: Some(domain::CommitId::parse(&"1".repeat(40)).unwrap()),
            to: domain::CommitId::parse(&"2".repeat(40)).unwrap(),
        },
        DomainEvent::AccessRequestCreated {
            user: "72f988bf-86f1-41af-91ab-2d7cd011db47:0b6c3e0e-5a1d-4a52-8f55-3f9d6f0a7f11"
                .parse()
                .unwrap(),
        },
        DomainEvent::DocIndexed { doc: 3.into() },
        DomainEvent::Resync,
        DomainEvent::AttributionUpdated {
            swimlane: lane(),
            path: path(),
            hash: hash(),
        },
        DomainEvent::PrStateChanged {
            link_id: 11.into(),
            state: PrState::Merged,
        },
        DomainEvent::PrJobProgress {
            job_id: 12.into(),
            state: PrJobState::Committing,
        },
        DomainEvent::SyncWindowOpened {
            swimlane: lane(),
            job: JobRef::new("sync-job-1", "uid-1").unwrap(),
        },
        DomainEvent::SyncWindowClosed {
            swimlane: lane(),
            job: JobRef::new("sync-job-1", "uid-1").unwrap(),
        },
        DomainEvent::UserSessionsRevoked { user: user() },
        // D85, prompt 05: replaces PendingRestartChanged as the event that a service's pickup state changed.
        DomainEvent::PickupChanged {
            swimlane: lane(),
            services: vec![ServiceRef::new("csp", "tx-infinity-api").unwrap()],
        },
        // contract/08-web (CG1b): baseline, tier, tenant set or auto-notify of a swimlane changed.
        DomainEvent::SwimlaneChanged { swimlane: lane() },
    ]
}

fn user() -> domain::UserId {
    "72f988bf-86f1-41af-91ab-2d7cd011db47:0b6c3e0e-5a1d-4a52-8f55-3f9d6f0a7f11"
        .parse()
        .unwrap()
}

#[test]
fn there_are_eighteen_variants_ten_base_five_d72_to_d80_one_internal_pickup_changed_and_swimlane_changed() {
    assert_eq!(all_events().len(), 18);
}

#[test]
fn wire_shape_is_stable() {
    insta::assert_json_snapshot!("domain_events", all_events());
}

#[test]
fn every_event_round_trips() {
    for e in all_events() {
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<DomainEvent>(&json).unwrap(), e);
    }
}

#[test]
fn unknown_event_type_is_rejected() {
    assert!(serde_json::from_str::<DomainEvent>("{\"type\":\"nope\"}").is_err());
}

#[test]
fn event_swimlane_is_exposed_for_filtering() {
    let e = DomainEvent::FindingsChanged { swimlane: lane() };
    assert_eq!(e.swimlane(), Some(&lane()));
    assert_eq!(DomainEvent::Resync.swimlane(), None);
    assert_eq!(DomainEvent::DocIndexed { doc: 1.into() }.swimlane(), None);
}

#[test]
fn user_sessions_revoked_is_global_and_carries_only_the_user_id() {
    let e = DomainEvent::UserSessionsRevoked { user: user() };
    assert_eq!(e.swimlane(), None);
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "user_sessions_revoked");
    assert_eq!(json.as_object().unwrap().len(), 2, "type and user, nothing else");
}

#[test]
fn only_user_sessions_revoked_is_internal_so_the_sse_edge_never_forwards_it() {
    for e in all_events() {
        let expected = matches!(e, DomainEvent::UserSessionsRevoked { .. });
        assert_eq!(e.is_internal(), expected, "{e:?}");
    }
}

#[test]
fn pickup_changed_is_swimlane_scoped_public_and_carries_service_ids_only() {
    let e = DomainEvent::PickupChanged {
        swimlane: lane(),
        services: vec![ServiceRef::new("csp", "tx-infinity-api").unwrap()],
    };
    assert_eq!(e.swimlane(), Some(&lane()));
    assert!(!e.is_internal(), "a browser may receive it");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "pickup_changed");
    assert_eq!(
        json.as_object().unwrap().len(),
        3,
        "type, swimlane and services, nothing else: the live-by time travels in the pending-restarts read, not in the event"
    );
    assert_eq!(serde_json::from_value::<DomainEvent>(json).unwrap(), e);
}

#[test]
fn swimlane_changed_is_swimlane_scoped_public_and_carries_ids_only() {
    let e = DomainEvent::SwimlaneChanged { swimlane: lane() };
    assert_eq!(e.swimlane(), Some(&lane()));
    assert!(!e.is_internal(), "a browser may receive it");
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["type"], "swimlane_changed");
    assert_eq!(
        json.as_object().unwrap().len(),
        2,
        "type and swimlane, nothing else: what changed (baseline, tier, tenants, auto-notify) is read from the swimlane summary, not from the event"
    );
    assert_eq!(serde_json::from_value::<DomainEvent>(json).unwrap(), e);
}

#[test]
fn pending_restart_changed_stays_so_that_old_rows_and_subscribers_still_parse() {
    let old = r#"{"type":"pending_restart_changed","swimlane":"sitb","services":[{"namespace":"csp","name":"tx-infinity-api"}]}"#;
    let e: DomainEvent = serde_json::from_str(old).unwrap();
    assert!(matches!(e, DomainEvent::PendingRestartChanged { .. }));
    assert_eq!(e.swimlane(), Some(&lane()));
}
