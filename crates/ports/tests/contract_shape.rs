//! Shape of the contract: object safety, bounds, error enums, wire names and the audit canonical form.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use domain::{
    AgentStatus, CommitId, ContentHash, DomainEvent, GitRef, JobRef, NfsPath, ObservationSource, PrJobId,
    PrJobState, PrLinkId, PrState, ProposalId, ProposalStatus, RepoKind, Role, ServiceRef, SwimlaneId,
    Timestamp, UserId,
};
use ports::conformance::sample::{swimlane, user_id};
use ports::{
    AckSeq, AgentGateway, AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, BlobStore, DocSearch,
    EventBus, EventFilter, EventKind, GitReader, KeyRef, KeyRefError, KmsEnvelope, KmsSigner, Leases,
    NotificationKind, Notifier, RegistryRead, ReportSink, SecretSource, SentinelSink, TokenVerifier,
    ToolWriteEntry, ToolWrites, Users, WriteService, canonical_json, chain_hash, genesis_hash,
};
use static_assertions::{assert_impl_all, assert_obj_safe};

macro_rules! ports {
    ($($t:ident),+ $(,)?) => {
        $( assert_obj_safe!($t); )+
        fn assert_bounds<T: ?Sized + Send + Sync + 'static>() {}
        /// Every port, as the names `docs/interfaces.md` uses.
        const PORTS: &[&str] = &[ $( stringify!($t) ),+ ];
        #[test]
        fn all_ports_are_object_safe_send_sync_static() {
            $( assert_bounds::<dyn $t>(); )+
            // Usable as `Arc<dyn Port>`, which is how the hub holds them.
            $( let _ = |p: std::sync::Arc<dyn $t>| p; )+
        }
    };
}

ports!(
    AgentGateway,
    ReportSink,
    BlobStore,
    GitReader,
    EventBus,
    Leases,
    KmsSigner,
    KmsEnvelope,
    SecretSource,
    Notifier,
    TokenVerifier,
    Users,
    AuditLog,
    RegistryRead,
    WriteService,
    ToolWrites,
    DocSearch,
    SentinelSink,
);

#[test]
fn every_interfaces_md_trait_is_covered() {
    let doc = include_str!("../../../docs/interfaces.md");
    let mut in_doc = BTreeSet::new();
    for line in doc.lines() {
        if let Some(rest) = line.split("pub trait ").nth(1) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            in_doc.insert(name);
        }
    }
    let ours: BTreeSet<String> = PORTS.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(
        in_doc, ours,
        "docs/interfaces.md and the ports crate list the same traits"
    );
    assert_eq!(ours.len(), 18);
}

#[test]
fn errors_do_not_leak_inputs() {
    // A `Copy` enum cannot hold a `String` or a `Vec`, so no error can echo a path, token or secret (S21).
    assert_impl_all!(ports::GatewayError: Copy);
    assert_impl_all!(ports::SinkError: Copy);
    assert_impl_all!(ports::BlobError: Copy);
    assert_impl_all!(ports::GitError: Copy);
    assert_impl_all!(ports::BusError: Copy);
    assert_impl_all!(ports::LeaseError: Copy);
    assert_impl_all!(ports::KmsError: Copy);
    assert_impl_all!(ports::KeyRefError: Copy);
    assert_impl_all!(ports::SecretError: Copy);
    assert_impl_all!(ports::NotifyError: Copy);
    assert_impl_all!(ports::AuthError: Copy);
    assert_impl_all!(ports::UserError: Copy);
    assert_impl_all!(ports::AuditError: Copy);
    assert_impl_all!(ports::ReadError: Copy);
    assert_impl_all!(ports::WriteError: Copy);
    assert_impl_all!(ports::DocError: Copy);
    assert_impl_all!(ports::TxError: Copy);
    assert_impl_all!(ports::WrongTxKind: Copy);
}

fn sample_events() -> Vec<DomainEvent> {
    let sl = || SwimlaneId::parse("sit1").unwrap();
    let path = || NfsPath::parse("a/b.yml").unwrap();
    let hash = || ContentHash::from_bytes([3; 32]);
    let commit = || CommitId::parse(&"a".repeat(40)).unwrap();
    let job = || JobRef::new("sync", "uid-1").unwrap();
    vec![
        DomainEvent::FileObserved {
            swimlane: sl(),
            path: path(),
            hash: hash(),
            source: ObservationSource::Scan,
        },
        DomainEvent::DriftChanged {
            swimlane: sl(),
            paths: vec![path()],
        },
        DomainEvent::FindingsChanged { swimlane: sl() },
        DomainEvent::PendingRestartChanged {
            swimlane: sl(),
            services: vec![ServiceRef::new("ns", "svc").unwrap()],
        },
        DomainEvent::ProposalChanged {
            id: ProposalId::from(1),
            status: ProposalStatus::Pending,
        },
        DomainEvent::AgentStatusChanged {
            swimlane: sl(),
            status: AgentStatus::Connected,
        },
        DomainEvent::GitHeadMoved {
            repo: RepoKind::Base,
            branch: GitRef::parse("main").unwrap(),
            from: None,
            to: commit(),
        },
        DomainEvent::AccessRequestCreated { user: user_id(1) },
        DomainEvent::DocIndexed {
            doc: domain::DocId::from(1),
        },
        DomainEvent::Resync,
        DomainEvent::AttributionUpdated {
            swimlane: sl(),
            path: path(),
            hash: hash(),
        },
        DomainEvent::PrStateChanged {
            link_id: PrLinkId::from(1),
            state: PrState::Open,
        },
        DomainEvent::PrJobProgress {
            job_id: PrJobId::from(1),
            state: PrJobState::Pending,
        },
        DomainEvent::SyncWindowOpened {
            swimlane: sl(),
            job: job(),
        },
        DomainEvent::SyncWindowClosed {
            swimlane: sl(),
            job: job(),
        },
        DomainEvent::UserSessionsRevoked { user: user_id(1) },
        DomainEvent::PickupChanged {
            swimlane: sl(),
            services: vec![ServiceRef::new("ns", "svc").unwrap()],
        },
        DomainEvent::SwimlaneChanged { swimlane: sl() },
    ]
}

#[test]
fn event_kind_is_the_wire_type_of_every_domain_event() {
    let events = sample_events();
    let mut kinds = BTreeSet::new();
    for e in &events {
        let tag = serde_json::to_value(e).unwrap()["type"]
            .as_str()
            .unwrap()
            .to_owned();
        let kind = serde_json::to_value(EventKind::of(e)).unwrap();
        assert_eq!(kind, serde_json::Value::String(tag), "kind of {e:?}");
        kinds.insert(EventKind::of(e));
    }
    assert_eq!(
        kinds.len(),
        18,
        "5 added by D72-D80, the internal UserSessionsRevoked, PickupChanged (prompt 05), SwimlaneChanged (contract/08-web) and the 10 base variants, all distinct"
    );
}

/// `ports::conformance::event_bus` has teeth for `swimlane_changed` (contract/08-web): a bus that drops the event, or hands it
/// on with another swimlane, fails the suite, and the same wrapper with no defect passes it, so the wrapper is not what fails.
mod swimlane_changed_conformance {
    use async_trait::async_trait;
    use domain::DomainEvent;
    use futures::stream::BoxStream;
    use ports::conformance::{self, sample::swimlane};
    use ports::fakes::{FakeEventBus, FakeTxFactory};
    use ports::{BusError, EventBus, EventFilter, Tx};

    #[derive(Clone, Copy)]
    enum Defect {
        None,
        /// Loses every `swimlane_changed`, and says nothing.
        Drops,
        /// Hands every `swimlane_changed` on, for another swimlane.
        WrongSwimlane,
    }

    struct Bus {
        inner: FakeEventBus,
        defect: Defect,
    }

    impl Bus {
        /// What the bus makes of an event: `None` when it loses it.
        fn handle(&self, e: DomainEvent) -> Option<DomainEvent> {
            match (self.defect, e) {
                (Defect::Drops, DomainEvent::SwimlaneChanged { .. }) => None,
                (Defect::WrongSwimlane, DomainEvent::SwimlaneChanged { .. }) => {
                    Some(DomainEvent::SwimlaneChanged {
                        swimlane: swimlane("sit9"),
                    })
                }
                (_, e) => Some(e),
            }
        }
    }

    #[async_trait]
    impl EventBus for Bus {
        async fn publish(&self, e: DomainEvent) -> Result<(), BusError> {
            match self.handle(e) {
                Some(e) => self.inner.publish(e).await,
                None => Ok(()),
            }
        }

        async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
            match self.handle(e) {
                Some(e) => self.inner.publish_in_tx(tx, e).await,
                None => Ok(()),
            }
        }

        fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent> {
            self.inner.subscribe(f)
        }
    }

    async fn run(defect: Defect) {
        let bus = Bus {
            inner: FakeEventBus::new(8),
            defect,
        };
        conformance::event_bus(&bus, &FakeTxFactory::new(), 8).await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_event_bus_suite_passes_for_a_bus_that_handles_swimlane_changed() {
        run(Defect::None).await;
    }

    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "swimlane_changed was not delivered")]
    async fn the_event_bus_suite_catches_a_bus_that_drops_swimlane_changed() {
        run(Defect::Drops).await;
    }

    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "swimlane_changed arrives exactly as published")]
    async fn the_event_bus_suite_catches_a_bus_that_hands_on_another_swimlane() {
        run(Defect::WrongSwimlane).await;
    }
}

#[test]
fn event_filter_limits_by_swimlane_and_kind_and_never_hides_resync() {
    let events = sample_events();
    let find = |k: EventKind| events.iter().find(|e| EventKind::of(e) == k).unwrap();

    assert!(events.iter().all(|e| EventFilter::all().matches(e)));

    let sit1 = EventFilter::for_swimlane(swimlane("sit1"));
    let sit2 = EventFilter::for_swimlane(swimlane("sit2"));
    assert!(sit1.matches(find(EventKind::FindingsChanged)));
    assert!(!sit2.matches(find(EventKind::FindingsChanged)));
    assert!(
        sit2.matches(find(EventKind::ProposalChanged)),
        "events without a swimlane pass"
    );
    assert!(sit2.matches(&DomainEvent::Resync), "Resync always passes");

    let only_drift = EventFilter {
        swimlanes: None,
        kinds: [EventKind::DriftChanged].into(),
    };
    assert!(only_drift.matches(find(EventKind::DriftChanged)));
    assert!(!only_drift.matches(find(EventKind::FindingsChanged)));
    assert!(
        only_drift.matches(&DomainEvent::Resync),
        "even a kind filter learns about lag"
    );
}

#[test]
fn key_ref_accepts_kms_resource_names_and_rejects_the_rest() {
    let name = "projects/p/locations/europe/keyRings/r/cryptoKeys/audit/cryptoKeyVersions/1";
    assert_eq!(KeyRef::parse(name).unwrap().as_str(), name);
    assert_eq!(KeyRef::parse("").unwrap_err(), KeyRefError::Empty);
    assert_eq!(KeyRef::parse("has space").unwrap_err(), KeyRefError::BadChar);
    assert_eq!(KeyRef::parse("semi;colon").unwrap_err(), KeyRefError::BadChar);
    assert_eq!(
        KeyRef::parse(&"k".repeat(KeyRef::MAX_LEN + 1)).unwrap_err(),
        KeyRefError::TooLong
    );
    assert!(serde_json::from_str::<KeyRef>("\"bad key\"").is_err());
}

fn all_actions() -> Vec<AuditAction> {
    use AuditAction::*;
    let all = vec![
        FileEdited,
        FileUploaded,
        FileDeleted,
        FileReverted,
        FileChangeDetected,
        AttributionUpgraded,
        ServiceRestarted,
        ConfigServerNotified,
        PrRaised,
        ProposalCreated,
        ProposalApproved,
        ProposalRejected,
        ProposalExpired,
        ProposalStale,
        DraftApplied,
        BaselineAdopted,
        DriftMarkedIntentional,
        DriftMarkCleared,
        FlaggedValueRevealed,
        RoleChanged,
        UserStatusChanged,
        AccessRequested,
        AccessApproved,
        AccessDenied,
        GithubTokenStored,
        GithubTokenDeleted,
        DocCreated,
        DocUpdated,
        DocDeleted,
        AgentJoined,
        AgentCertificateRenewed,
        SentinelEnrolled,
        SettingsChanged,
        AutoNotifyChanged,
        SessionSignedIn,
        SessionSignedOut,
        PrStateChanged,
        McpTokenCreated,
        McpTokenRevoked,
    ];
    // An exhaustive match: adding a variant without listing it above stops this test from compiling.
    for a in &all {
        match a {
            FileEdited
            | FileUploaded
            | FileDeleted
            | FileReverted
            | FileChangeDetected
            | AttributionUpgraded
            | ServiceRestarted
            | ConfigServerNotified
            | PrRaised
            | ProposalCreated
            | ProposalApproved
            | ProposalRejected
            | ProposalExpired
            | ProposalStale
            | DraftApplied
            | BaselineAdopted
            | DriftMarkedIntentional
            | DriftMarkCleared
            | FlaggedValueRevealed
            | RoleChanged
            | UserStatusChanged
            | AccessRequested
            | AccessApproved
            | AccessDenied
            | GithubTokenStored
            | GithubTokenDeleted
            | DocCreated
            | DocUpdated
            | DocDeleted
            | AgentJoined
            | AgentCertificateRenewed
            | SentinelEnrolled
            | SettingsChanged
            | AutoNotifyChanged
            | SessionSignedIn
            | SessionSignedOut
            | PrStateChanged
            | McpTokenCreated
            | McpTokenRevoked => {}
        }
    }
    all
}

#[test]
fn audit_action_wire_names_are_stable() {
    let names: Vec<String> = all_actions()
        .iter()
        .map(|a| serde_json::to_value(a).unwrap().as_str().unwrap().to_owned())
        .collect();
    insta::assert_json_snapshot!("audit_action_wire_names", names);
}

fn fixed_event() -> AuditEvent {
    let mut e = AuditEvent::new(
        Timestamp::from_unix_millis(1_700_000_000_000),
        AuditActor::User { user: user_id(1) },
        AuditAction::FileEdited,
        AuditVia::Mcp,
    );
    e.swimlane = Some(swimlane("sit1"));
    e.path = Some(NfsPath::parse("tx-infinity-api/tx-infinity-core-sit1.yml").unwrap());
    e.hash_before = Some(ContentHash::from_bytes([1; 32]));
    e.hash_after = Some(ContentHash::from_bytes([2; 32]));
    e.diff = Some("-a: 1\n+a: 2\n".to_owned());
    e.github_login = None;
    e.approval = Some(ProposalId::from(7));
    e
}

#[test]
fn audit_canonical_json_and_chain_hash_are_pinned() {
    // Changing the field order or a wire name breaks every stored chain, so this snapshot is a contract.
    let json = String::from_utf8(canonical_json(&fixed_event()).unwrap()).unwrap();
    let hash = chain_hash(&genesis_hash(), json.as_bytes());
    insta::assert_snapshot!(
        "audit_canonical_json",
        format!("{json}\nchain_hash(genesis) = {hash}")
    );
}

#[test]
fn audit_author_is_omitted_when_absent_and_written_after_the_approval_when_set() {
    // The canonical JSON of an event without an author has no `author` key, so every stored chain is unchanged.
    let without = String::from_utf8(canonical_json(&fixed_event()).unwrap()).unwrap();
    assert!(!without.contains("author"), "{without}");
    let mut with = fixed_event();
    with.author = Some(user_id(2));
    let with = String::from_utf8(canonical_json(&with).unwrap()).unwrap();
    let approval = with.find("\"approval\"").unwrap();
    let author = with.find("\"author\"").unwrap();
    let request_id = with.find("\"request_id\"").unwrap();
    assert!(approval < author && author < request_id, "{with}");
    insta::assert_snapshot!("audit_canonical_json_with_author", with);
    // An event stored before the field existed still reads, with no author.
    let old: AuditEvent = serde_json::from_str(&without).unwrap();
    assert_eq!(old, fixed_event());
}

#[test]
fn audit_actor_and_via_wire_shapes() {
    let actors = [
        AuditActor::User { user: user_id(1) },
        AuditActor::Agent {
            swimlane: swimlane("sit1"),
        },
        AuditActor::Sentinel {
            name: "nfs-vm-1".parse().unwrap(),
        },
        AuditActor::System {
            component: "retention".parse().unwrap(),
        },
    ];
    let vias = [
        AuditVia::Ui,
        AuditVia::Mcp,
        AuditVia::Sync,
        AuditVia::AgentDetected,
        AuditVia::System,
    ];
    insta::assert_json_snapshot!(
        "audit_actor_via",
        serde_json::json!({ "actors": actors, "vias": vias })
    );
}

#[test]
fn ack_seq_is_transparent() {
    assert_eq!(serde_json::to_string(&AckSeq::new(42)).unwrap(), "42");
    assert!(AckSeq::new(2) > AckSeq::new(1));
}

#[test]
fn identities_cannot_be_confused() {
    // An agent identity is a swimlane; a sentinel identity is a name. They are different types, so a
    // sentinel can never be passed where an agent is expected (S5, Q11).
    let a = ports::AgentIdentity::new(swimlane("sit1"));
    let s = ports::SentinelIdentity::new("sit1".parse().unwrap());
    assert_eq!(a.swimlane().as_str(), "sit1");
    assert_eq!(s.name().as_str(), "sit1");
    let _: UserId = user_id(1);
    let _: Role = Role::Viewer;
}

#[cfg(feature = "postgres")]
#[test]
fn an_in_memory_tx_is_not_a_postgres_tx() {
    use ports::fakes::FakeTxState;
    let mut state = FakeTxState::new();
    let mut tx = ports::Tx::in_memory(&mut state);
    assert!(tx.pg().is_err());
    assert!(tx.mem().is_ok());
}

#[test]
fn tx_debug_names_the_backing_store_only() {
    let mut state = ports::fakes::FakeTxState::new();
    let tx = ports::Tx::in_memory(&mut state);
    assert_eq!(format!("{tx:?}"), "Tx { kind: \"in_memory\" }");
}

#[test]
fn internal_events_reach_in_process_subscribers_but_are_flagged_for_the_sse_edge() {
    let revoked = DomainEvent::UserSessionsRevoked { user: user_id(1) };
    assert!(revoked.is_internal());
    // The bus filter does not hide it: the session cache subscribes with these filters.
    assert!(EventFilter::all().matches(&revoked));
    assert!(EventFilter::for_swimlane(swimlane("sit1")).matches(&revoked));
    let only_revoked = EventFilter {
        swimlanes: None,
        kinds: [EventKind::UserSessionsRevoked].into(),
    };
    assert!(only_revoked.matches(&revoked));
    let only_drift = EventFilter {
        swimlanes: None,
        kinds: [EventKind::DriftChanged].into(),
    };
    assert!(!only_drift.matches(&revoked));
    // Every other event is public.
    assert!(
        sample_events()
            .iter()
            .filter(|e| !matches!(e, DomainEvent::UserSessionsRevoked { .. }))
            .all(|e| !e.is_internal())
    );
}

/// The notification kinds are named in snake case on the wire. `DriftDetected` and `SyncWindowStale` were added for the
/// registry (prompt 05, T8): held drift alerts evaluated when a sync window closes, and a window open for over two hours.
#[test]
fn notification_kinds_have_stable_wire_names() {
    let kinds = [
        (NotificationKind::AccessRequested, "access_requested"),
        (NotificationKind::ProposalPending, "proposal_pending"),
        (NotificationKind::ProposalDecided, "proposal_decided"),
        (NotificationKind::ServiceRestarted, "service_restarted"),
        (NotificationKind::PrRaised, "pr_raised"),
        (NotificationKind::AgentDisconnected, "agent_disconnected"),
        (NotificationKind::CriticalFinding, "critical_finding"),
        (NotificationKind::DriftDetected, "drift_detected"),
        (NotificationKind::SyncWindowStale, "sync_window_stale"),
    ];
    for (kind, name) in kinds {
        assert_eq!(serde_json::to_value(kind).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<NotificationKind>(name.into()).unwrap(),
            kind
        );
    }
    assert!(serde_json::from_value::<NotificationKind>("nope".into()).is_err());
}

#[test]
fn tool_write_ttl_is_pinned() {
    // `docs/interfaces.md` says 15 minutes; a registry and its fake rely on the number, so a change is a contract change.
    // (`ChangePreview::TTL_SECS`, 10 minutes, is pinned in the domain tests.)
    assert_eq!(ToolWriteEntry::TTL_SECS, 900);
}
