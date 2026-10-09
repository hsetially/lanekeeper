//! Behaviour of the fakes that other crates rely on, beyond what the conformance suites assert.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{AgentReply, DomainEvent, HubCommand, OpResult, ProposalId, ProposalStatus};
use futures::StreamExt;
use ports::conformance::sample::{hash, nfs, request_id, swimlane};
use ports::fakes::{
    FakeAgentGateway, FakeAuditLog, FakeBlobStore, FakeEventBus, FakeGit, FakeLeases, FakeSecretSource,
    FakeTxFactory,
};
use ports::{
    AgentGateway, AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, BlobError, BlobStore, EventBus,
    EventFilter, GatewayError, GitReader, Leases, SecretSource, TxFactory, verify_chain,
};
use tokio::time::Instant;

fn cluster(id: &str) -> HubCommand {
    HubCommand::RequestClusterReport {
        request_id: request_id(id),
    }
}

#[tokio::test(start_paused = true)]
async fn gateway_timeout_takes_exactly_the_timeout_on_the_tokio_clock() {
    let gw = FakeAgentGateway::new();
    let s = swimlane("sit1");
    gw.attach(&s, Arc::new(|_| None));
    let started = Instant::now();
    let r = gw.request(&s, cluster("r-1"), Duration::from_secs(30)).await;
    assert_eq!(r, Err(GatewayError::Timeout));
    assert_eq!(started.elapsed(), Duration::from_secs(30));
}

#[tokio::test(start_paused = true)]
async fn gateway_scripts_replies_and_records_commands() {
    let gw = FakeAgentGateway::new();
    let s = swimlane("sit1");
    gw.attach(
        &s,
        Arc::new(|cmd| match cmd {
            HubCommand::ReadFile { request_id, path } => Some(AgentReply::File {
                request_id: request_id.clone(),
                path: path.clone(),
                hash: hash(b"abc"),
                bytes: Bytes::from_static(b"abc"),
            }),
            _ => None,
        }),
    );
    let cmd = HubCommand::ReadFile {
        request_id: request_id("r-1"),
        path: nfs("a.yml"),
    };
    let reply = gw.request(&s, cmd.clone(), Duration::from_secs(1)).await.unwrap();
    assert!(matches!(reply, AgentReply::File { .. }));
    assert_eq!(gw.received(&s), [cmd]);
}

#[tokio::test(start_paused = true)]
async fn gateway_refuses_more_than_64_requests_in_flight() {
    let gw = FakeAgentGateway::new();
    let s = swimlane("sit1");
    gw.attach(&s, Arc::new(|_| None));
    let mut tasks = Vec::new();
    for i in 0..64 {
        let (gw, s) = (gw.clone(), s.clone());
        tasks.push(tokio::spawn(async move {
            gw.request(&s, cluster(&format!("r-{i}")), Duration::from_secs(10))
                .await
        }));
    }
    // Let every spawned request reach the agent; the scheduler polls only so many tasks per turn.
    for _ in 0..100 {
        if gw.received(&s).len() == 64 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(gw.received(&s).len(), 64, "64 requests are in flight");
    assert_eq!(
        gw.request(&s, cluster("r-over"), Duration::from_secs(1)).await,
        Err(GatewayError::Busy),
        "the 65th request"
    );
    for t in tasks {
        assert_eq!(t.await.unwrap(), Err(GatewayError::Timeout));
    }
    // The slots are free again once the requests ended.
    gw.attach(
        &s,
        Arc::new(|_| {
            Some(AgentReply::Op(OpResult {
                request_id: request_id("x"),
                ok: true,
                error: None,
                current_hash: None,
            }))
        }),
    );
    assert!(
        gw.request(&s, cluster("r-after"), Duration::from_secs(1))
            .await
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn gateway_slot_is_released_when_the_caller_gives_up() {
    let gw = FakeAgentGateway::new();
    let s = swimlane("sit1");
    gw.attach(&s, Arc::new(|_| None));
    for i in 0..200 {
        // A caller that is dropped (cancelled) must not leak its slot.
        let fut = gw.request(&s, cluster(&format!("c-{i}")), Duration::from_secs(60));
        let _ = tokio::time::timeout(Duration::from_millis(1), fut).await;
    }
    assert_eq!(
        gw.request(&s, cluster("last"), Duration::from_millis(5)).await,
        Err(GatewayError::Timeout),
        "still not Busy after 200 cancelled requests"
    );
}

#[tokio::test(start_paused = true)]
async fn blob_store_refuses_new_content_when_full_but_still_accepts_known_content() {
    let store = FakeBlobStore::with_capacity(10);
    let a = store.put(Bytes::from_static(b"123456")).await.unwrap();
    assert_eq!(
        store.put(Bytes::from_static(b"abcdef")).await,
        Err(BlobError::Full)
    );
    assert_eq!(
        store.put(Bytes::from_static(b"123456")).await.unwrap(),
        a,
        "known content is not new"
    );
    assert_eq!(store.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn event_stream_ends_when_the_bus_is_dropped() {
    let bus = FakeEventBus::new(4);
    let mut sub = bus.subscribe(EventFilter::all());
    bus.publish(DomainEvent::Resync).await.unwrap();
    drop(bus);
    assert_eq!(
        sub.next().await,
        Some(DomainEvent::Resync),
        "queued events are still delivered"
    );
    assert_eq!(sub.next().await, None);
}

#[tokio::test(start_paused = true)]
async fn lease_can_be_taken_the_moment_the_clock_passes_expiry() {
    let leases = FakeLeases::new();
    let ttl = Duration::from_secs(30);
    let g = leases.try_acquire("job.x", ttl).await.unwrap().unwrap();
    tokio::time::advance(ttl - Duration::from_millis(1)).await;
    assert!(g.is_valid());
    assert!(leases.try_acquire("job.x", ttl).await.unwrap().is_none());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(!g.is_valid());
    assert!(leases.try_acquire("job.x", ttl).await.unwrap().is_some());
}

#[tokio::test(start_paused = true)]
async fn tampering_with_a_committed_audit_entry_breaks_the_chain_at_that_entry() {
    let log = FakeAuditLog::new();
    let txf = FakeTxFactory::new();
    for i in 0..3 {
        let mut tx = txf.begin().await.unwrap();
        let e = AuditEvent::new(
            domain::Timestamp::from_unix_millis(i),
            AuditActor::System {
                component: "test".parse().unwrap(),
            },
            AuditAction::SettingsChanged,
            AuditVia::System,
        );
        log.record(&mut tx.tx(), e).await.unwrap();
        tx.commit().await.unwrap();
    }
    let mut chain = log.entries();
    assert_eq!(verify_chain(&chain), Ok(()));
    chain[1].event.at = domain::Timestamp::from_unix_millis(999);
    assert_eq!(
        verify_chain(&chain),
        Err(1),
        "the edited entry no longer matches its hash"
    );
    let mut chain = log.entries();
    chain.swap(1, 2);
    assert_eq!(verify_chain(&chain), Err(1), "reordering breaks the links");
    let mut chain = log.entries();
    chain.remove(0);
    assert_eq!(
        verify_chain(&chain),
        Err(0),
        "dropping the first entry breaks the genesis link"
    );
}

#[test]
fn git_tree_index_is_built_once_per_commit() {
    let (git, x) = FakeGit::with_conformance_repo().unwrap();
    let head = git.head(x.repo, &x.branch).unwrap();
    let a = git.tree_index(x.repo, &head).unwrap();
    let b = git.tree_index(x.repo, &head).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
}

#[tokio::test]
async fn secret_source_debug_never_prints_values() {
    let src = FakeSecretSource::new().with("token", "hunter2-super-secret");
    let shown = format!("{src:?}");
    assert!(shown.contains("token") && !shown.contains("hunter2"));
    let got = src.get("token").await.unwrap();
    assert!(!format!("{got:?}").contains("hunter2"));
}

#[test]
fn proposal_and_status_types_used_by_fakes_are_plain_data() {
    // Guards the assumption in WriteService fakes that outcomes are cheap to clone and compare.
    let a = DomainEvent::ProposalChanged {
        id: ProposalId::from(1),
        status: ProposalStatus::Applied,
    };
    assert_eq!(a.clone(), a);
}
