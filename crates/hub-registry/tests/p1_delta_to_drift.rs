//! P1 end to end (T3, A44): a fake agent changes one file on NFS, and `DriftChanged` reaches the outbox within the 15 seconds P1 gives
//! an out-of-band change.
//!
//! The fake agent is the agent's side of the report channel: it says hello, delivers the swimlane's first scan, and later delivers a scan
//! delta with one file changed, through the registry's ingestion (the `ReportSink` the gateway calls). The pipeline job runs as it does in
//! the hub, over the real event bus: ingestion publishes `FileObserved` in its transaction, the relay wakes the job, the job waits out
//! its coalescing window, recomputes the path, and commits with `DriftChanged` in the same transaction. The swimlane and the Git
//! repositories are the small synthetic fixtures (rule 12).
//!
//! The 15 seconds are shared: the agent walks for up to 10 s and waits 3 s for quiet before it sends (the agent's own bounds, tested in
//! its crate), and the hub has the other 2 s (`P1.hub_delta_to_drift`, whose p95 the bench `delta_to_drift_changed` holds at target scale).
//! The test adds the agent's 13 s as time that has passed, and asserts the sum against P1, so that the hub's share is the 2 s and no more.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use domain::{NfsPath, SwimlaneId};
use hub_registry::metrics::LaneResult;
use hub_registry::{Clock, RegistryConfig};
use ports::{AgentIdentity, ReportSink};
use support::rig::{Rig, delta, entry, hello, root};
use support::world::{RunningJob, wait_until};
use support::{fixtures, gitload};

/// P1.
const BUDGET: Duration = Duration::from_secs(15);
/// What the agent spends before the hub hears of a change: its walk (D-agent bound) and the quiet period it waits for.
const AGENT_WALK: Duration = Duration::from_secs(10);
const AGENT_QUIET: Duration = Duration::from_secs(3);
const WAIT: Duration = Duration::from_secs(30);

/// A file the fake agent can change: structured YAML that Git has, not denied and not huge.
fn a_file_to_change(files: &[(NfsPath, Bytes)]) -> (NfsPath, Bytes) {
    files
        .iter()
        .find(|(p, b)| {
            let name = p.as_str();
            let yaml = std::path::Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"));
            yaml && b.len() < 50_000 && !name.contains("secret")
        })
        .cloned()
        .expect("the small fixtures have a YAML file")
}

#[tokio::test]
async fn fake_agent_change_reaches_drift_changed_within_15_seconds() {
    let fx = fixtures::small();
    let lane_name = fx.swimlanes().remove(0);
    let tenants = fx.tenants(&lane_name);
    let files = fx.files(&lane_name);
    let Some(rig) = Rig::with_event_bus(&lane_name, RegistryConfig::default()).await else {
        return;
    };
    for t in &tenants {
        rig.db.seed_tenant(&rig.pool, &lane_name, t).await;
    }
    let git = gitload::load(&fx.root);
    let (pipeline, _handle) = rig.pipeline(Arc::new(git), RegistryConfig::default());
    let identity = AgentIdentity::new(SwimlaneId::parse(&lane_name).unwrap());

    // The agent's first scan: hello, then the whole tree (in parts, as a large scan is sent).
    rig.ingest.hello(&identity, hello(&lane_name)).await.unwrap();
    let at = rig.clock.now().unix_millis() - 3_600_000;
    let chunks: Vec<&[(NfsPath, Bytes)]> = files.chunks(500).collect();
    let last = chunks.len();
    for (i, chunk) in chunks.into_iter().enumerate() {
        let entries = chunk.iter().map(|(p, b)| entry(p.as_str(), b, at)).collect();
        let mut d = delta(i as u64 + 1, None, root(0), entries);
        d.more = i + 1 < last;
        d.part = u32::try_from(i).unwrap();
        rig.ingest.delta(&identity, d).await.unwrap();
    }

    // The hub is running and has loaded the swimlane, as it has long before an operator's change.
    let wholes = rig.metrics.pipeline_lane_count(LaneResult::Whole);
    let job = RunningJob::start(&pipeline);
    wait_until("the job to load the swimlane", WAIT, || async {
        rig.metrics.pipeline_lane_count(LaneResult::Whole) > wholes
            && !pipeline.lanes_in_memory().await.is_empty()
    })
    .await;

    // The change: a new version of one file, as the agent finds it after its walk and its quiet period.
    let (changed, original) = a_file_to_change(&files);
    let mut text = original.to_vec();
    text.extend_from_slice(b"\n# changed by an operator on the NFS volume\n");
    rig.clock
        .advance_secs(i64::try_from(AGENT_WALK.as_secs() + AGENT_QUIET.as_secs()).unwrap());
    let at = rig.clock.now().unix_millis() - 500;
    let after: i64 = sqlx::query_scalar("SELECT COALESCE(max(id), 0) FROM outbox")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    let before_state = sqlx::query_scalar::<_, String>(
        "SELECT state::text FROM file_state WHERE swimlane_id = $1 AND path = $2",
    )
    .bind(&lane_name)
    .bind(changed.as_str())
    .fetch_optional(&rig.pool)
    .await
    .unwrap();
    assert!(
        before_state.is_some(),
        "the file was computed with the swimlane: {changed}"
    );

    let sent = Instant::now();
    rig.ingest
        .delta(
            &identity,
            delta(
                1_000,
                Some(root(0)),
                root(0),
                vec![entry(changed.as_str(), &text, at)],
            ),
        )
        .await
        .unwrap();
    wait_until("DriftChanged naming the changed file", WAIT, || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM outbox WHERE id > $1 AND event #>> '{event,type}' = 'drift_changed' \
             AND (event #> '{event,paths}') @> to_jsonb($2::text)",
        )
        .bind(after)
        .bind(changed.as_str())
        .fetch_one(&rig.pool)
        .await
        .unwrap()
            > 0
    })
    .await;
    let hub = sent.elapsed();

    let end_to_end = AGENT_WALK + AGENT_QUIET + hub;
    assert!(
        end_to_end < BUDGET,
        "P1: the agent's {AGENT_WALK:?} and {AGENT_QUIET:?} plus the hub's {hub:?} must be under {BUDGET:?}"
    );
    // The row the event names is the change: the file's content hash is the new one.
    let nfs_hash: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT nfs_hash::bytea FROM file_state WHERE swimlane_id = $1 AND path = $2")
            .bind(&lane_name)
            .bind(changed.as_str())
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(
        nfs_hash.as_deref(),
        Some(ports::content_hash(&text).as_bytes().as_slice()),
        "the row has the new content"
    );
    let _ = job.lose_lease().await;
}
