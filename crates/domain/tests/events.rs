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
    ]
}

#[test]
fn there_are_fifteen_variants_ten_base_and_five_d72_to_d80() {
    assert_eq!(all_events().len(), 15);
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
