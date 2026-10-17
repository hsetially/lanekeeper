//! The ranking of the deployable branches (T5, D22, P6): the branch a swimlane was deployed from ranks first, over the synthetic fixtures
//! (rule 12) loaded through the registry's own ingestion path, and the fixtures' repositories in the in-memory Git.
//!
//! This is the small scale (4 swimlanes); `tests/adoption_rank_target.rs` is the same at design scale (40 swimlanes, 60 tenant branches)
//! with the time bound, and the benchmark `adoption_rank_per_swimlane` measures it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;

use bytes::Bytes;
use domain::{NfsPath, SwimlaneId};
use hub_registry::{AdoptionDeps, AdoptionReason, AdoptionService, RegistryConfig};
use ports::{AgentIdentity, ReportSink};
use support::adoption::{admin, assert_ranked_right, insert_user};
use support::rig::{Rig, delta, entry, hello, root};
use support::{fixtures, gitload};

/// Ingest a swimlane's files as an agent sends a first scan: in messages of 500 files.
async fn ingest(rig: &Rig, id: &str, files: &[(NfsPath, Bytes)]) {
    let identity = AgentIdentity::new(SwimlaneId::parse(id).unwrap());
    rig.ingest.hello(&identity, hello(id)).await.unwrap();
    let at = 1_000_000;
    let chunks: Vec<&[(NfsPath, Bytes)]> = files.chunks(500).collect();
    let last = chunks.len();
    for (i, chunk) in chunks.into_iter().enumerate() {
        let entries = chunk.iter().map(|(p, b)| entry(p.as_str(), b, at)).collect();
        let mut d = delta(i as u64 + 1, None, root(0), entries);
        d.more = i + 1 < last;
        d.part = u32::try_from(i).unwrap();
        rig.ingest.delta(&identity, d).await.unwrap();
    }
}

#[tokio::test]
async fn the_correct_branch_ranks_first_for_every_small_fixture_swimlane() {
    let fx = fixtures::small();
    let lanes = fx.swimlanes();
    let Some(rig) = Rig::with_event_bus(&lanes[0], RegistryConfig::default()).await else {
        return;
    };
    for (i, id) in lanes.iter().enumerate() {
        let tenants = fx.tenants(id);
        if i > 0 {
            let borrowed: Vec<&str> = tenants.iter().map(String::as_str).collect();
            rig.add_swimlane(id, &borrowed, false).await;
        }
        ingest(&rig, id, &fx.files(id)).await;
    }
    let git = gitload::load(&fx.root);
    let admin = admin();
    insert_user(&rig.pool, &admin).await;
    let svc = AdoptionService::new(AdoptionDeps {
        pool: rig.pool.clone(),
        git: Arc::new(git),
        blobs: Arc::clone(&rig.blobs) as _,
        bus: Arc::clone(&rig.bus) as _,
        audit: Arc::clone(&rig.audit) as _,
        clock: Arc::clone(&rig.clock) as _,
        config: RegistryConfig::default(),
        metrics: rig.metrics.clone(),
        pipeline: None,
    })
    .unwrap();
    assert!(lanes.len() >= 2, "the small fixtures have swimlanes");
    for id in &lanes {
        let view = svc
            .start(&admin, &SwimlaneId::parse(id).unwrap(), AdoptionReason::Initial)
            .await
            .unwrap();
        assert_ranked_right(&view, &fx.tenants(id));
        // The hints name nothing here (the fixtures have no cluster), and the pass is at its first step.
        assert!(view.hints.is_empty());
        assert_eq!(view.step, hub_registry::AdoptionStep::Branch);
    }
}
