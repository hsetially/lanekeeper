//! The bed of the pipeline's benchmarks (T3): the target-scale fixtures (40 swimlanes of about 2,000 files, 60 tenant branches) in a
//! real Postgres 16, loaded through the registry's own ingestion path as agents would send them, with the fixtures' Git repositories in
//! the in-memory Git and the pipeline over both. A benchmark measures the pipeline, never this set-up, and never on toy data.
//!
//! Content is read from the database on every use: the bed's blob store has no cache, which is the worse case for the pipeline (the
//! hub's `PgBlobStore` keeps a moka cache in front of the same table).
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::cast_precision_loss
)]

#[path = "../../tests/support/mod.rs"]
pub mod support;

pub mod reads;

use std::collections::BTreeSet;
use std::time::Instant;

use bytes::Bytes;
use domain::{NfsPath, ScanEntry, SwimlaneId, Timestamp};
use hub_registry::{Clock, Pipeline, PipelineHandle, RegistryConfig};
use ports::fakes::FakeGit;
use ports::{AgentIdentity, ReportSink, content_hash};
use support::rig::{Rig, hello, root};
use support::{fixtures, gitload};

/// One swimlane of the bed.
pub struct Lane {
    pub id: SwimlaneId,
    pub tenants: Vec<String>,
    /// Its files as the fixtures have them on NFS, sorted by path.
    pub files: Vec<(NfsPath, Bytes)>,
}

pub struct Bed {
    pub rig: Rig,
    pub git: FakeGit,
    pub pipeline: Pipeline,
    pub handle: PipelineHandle,
    pub lanes: Vec<Lane>,
    /// Edits made so far, so that every edit is a new version.
    round: u64,
}

/// Whether the file is YAML by its extension.
pub fn is_yaml(path: &NfsPath) -> bool {
    std::path::Path::new(path.file_name())
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
}

/// Application names that make some of the files property sources (D82), as the engine's own benchmark does: the stems of the first
/// 300 structured files, by name.
fn app_names(lanes: &[Lane]) -> Vec<String> {
    let mut stems = BTreeSet::new();
    for lane in lanes {
        for (path, _) in &lane.files {
            if let Some((stem, ext)) = path.file_name().rsplit_once('.') {
                if matches!(ext, "yml" | "yaml" | "properties") && !stem.is_empty() {
                    stems.insert(stem.to_owned());
                }
            }
        }
    }
    stems
        .into_iter()
        .filter(|s| domain::AppName::parse(s).is_ok())
        .take(300)
        .collect()
}

impl Bed {
    /// The bed with the first `lanes` swimlanes of the fixtures ingested (their inventories applied, nothing computed yet), or `None`
    /// when there is no database.
    pub async fn new(lanes: usize, config: RegistryConfig) -> Option<Self> {
        let started = Instant::now();
        let fx = fixtures::target();
        let ids: Vec<String> = fx.swimlanes().into_iter().take(lanes).collect();
        assert_eq!(ids.len(), lanes, "the fixtures have {} swimlanes", ids.len());
        let rig = Box::pin(Rig::with_event_bus(&ids[0], config.clone())).await?;
        let mut loaded = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let tenants = fx.tenants(id);
            if i == 0 {
                for t in &tenants {
                    rig.db.seed_tenant(&rig.pool, id, t).await;
                }
            } else {
                let borrowed: Vec<&str> = tenants.iter().map(String::as_str).collect();
                rig.add_swimlane(id, &borrowed, false).await;
            }
            loaded.push(Lane {
                id: SwimlaneId::parse(id).unwrap(),
                tenants,
                files: fx.files(id),
            });
        }
        let git = gitload::load(&fx.root);
        eprintln!(
            "bed: the fixtures' repositories loaded ({:.1} s)",
            started.elapsed().as_secs_f64()
        );
        let (pipeline, handle) = rig.pipeline(std::sync::Arc::new(git.clone()), config);
        let bed = Self {
            rig,
            git,
            pipeline,
            handle,
            lanes: loaded,
            round: 0,
        };
        let names = app_names(&bed.lanes);
        for i in 0..bed.lanes.len() {
            bed.ingest_inventory(i).await;
            bed.seed_services(i, &names).await;
        }
        eprintln!(
            "bed: {} swimlanes, {} files ingested ({:.1} s)",
            bed.lanes.len(),
            bed.lanes.iter().map(|l| l.files.len()).sum::<usize>(),
            started.elapsed().as_secs_f64()
        );
        Some(bed)
    }

    async fn seed_services(&self, lane: usize, names: &[String]) {
        sqlx::query(
            "INSERT INTO services (swimlane_id, namespace, name, env_values) \
             SELECT $1, 'ns', n, jsonb_build_object('SPRING_APPLICATION_NAME', n) FROM unnest($2::text[]) AS n",
        )
        .bind(self.lanes[lane].id.as_str())
        .bind(names)
        .execute(&self.rig.pool)
        .await
        .unwrap();
    }

    /// The first full scan of a swimlane, in messages of at most 1,000 files and 2.5 MiB, as an agent sends it.
    async fn ingest_inventory(&self, lane: usize) {
        let l = &self.lanes[lane];
        let identity = AgentIdentity::new(l.id.clone());
        self.rig
            .ingest
            .hello(&identity, hello(l.id.as_str()))
            .await
            .unwrap();
        let at = self.rig.clock.now().unix_millis() - 3_600_000;
        let mut chunks: Vec<Vec<ScanEntry>> = Vec::new();
        let mut batch = Vec::new();
        let mut bytes = 0_usize;
        for (path, content) in &l.files {
            if !batch.is_empty() && (batch.len() >= 1_000 || bytes + content.len() > 2_500_000) {
                chunks.push(std::mem::take(&mut batch));
                bytes = 0;
            }
            bytes += content.len();
            batch.push(entry(path, content.clone(), at));
        }
        if !batch.is_empty() {
            chunks.push(batch);
        }
        let last = chunks.len();
        for (i, entries) in chunks.into_iter().enumerate() {
            let mut d = support::rig::delta(i as u64 + 1, None, root(0), entries);
            d.more = i + 1 < last;
            d.part = u32::try_from(i).unwrap();
            self.rig.ingest.delta(&identity, d).await.unwrap();
        }
    }

    /// The scan delta in which these files of a swimlane have a new version, and the paths. Not sent: the caller decides when (and what
    /// it measures).
    pub fn edit_delta(
        &mut self,
        lane: usize,
        indices: &[usize],
    ) -> (AgentIdentity, domain::ScanDelta, Vec<NfsPath>) {
        self.round += 1;
        let round = self.round;
        self.rig.clock.advance_secs(10);
        let at = self.rig.clock.now().unix_millis() - 500;
        let l = &self.lanes[lane];
        let entries: Vec<ScanEntry> = indices
            .iter()
            .map(|&i| {
                let (path, content) = &l.files[i];
                entry(path, edit_text(content, round), at)
            })
            .collect();
        let paths = indices.iter().map(|&i| l.files[i].0.clone()).collect();
        // The agent's chain of roots is not what is measured: a delta whose base is not the stored root marks the swimlane for a full
        // scan and is still applied, which is all the pipeline needs.
        let d = support::rig::delta(1_000 + round, Some(root(0)), root(0), entries);
        (AgentIdentity::new(l.id.clone()), d, paths)
    }

    /// New versions of these files of a swimlane, sent as one scan delta the way an agent sends them (not measured by anyone: it is the
    /// change the pipeline is then asked about). Returns the paths.
    pub async fn edit(&mut self, lane: usize, indices: &[usize]) -> Vec<NfsPath> {
        let (identity, d, paths) = self.edit_delta(lane, indices);
        self.rig.ingest.delta(&identity, d).await.unwrap();
        paths
    }

    /// New files (or new versions) in a swimlane, sent as one scan delta the way an agent sends them, and the pipeline told about their
    /// paths (as the hub does when ingestion reports them): the state a read finds after a change. Returns the paths.
    pub async fn put_files(&mut self, lane: usize, files: Vec<(NfsPath, Bytes)>) -> Vec<NfsPath> {
        let paths: Vec<NfsPath> = files.iter().map(|(p, _)| p.clone()).collect();
        let identity = AgentIdentity::new(self.lanes[lane].id.clone());
        // Messages of at most 2 MB and 1,000 files, as an agent sends them (a message over the hub's bound is refused).
        let mut batches: Vec<Vec<(NfsPath, Bytes)>> = Vec::new();
        let mut bytes = 0_usize;
        for file in files {
            if batches
                .last()
                .is_none_or(|b| b.len() >= 1_000 || bytes + file.1.len() > 2_000_000)
            {
                batches.push(Vec::new());
                bytes = 0;
            }
            bytes += file.1.len();
            batches.last_mut().unwrap().push(file);
        }
        for batch in batches {
            self.round += 1;
            let round = self.round;
            self.rig.clock.advance_secs(10);
            let at = self.rig.clock.now().unix_millis() - 500;
            let entries: Vec<ScanEntry> = batch.into_iter().map(|(p, c)| entry(&p, c, at)).collect();
            let d = support::rig::delta(1_000 + round, Some(root(0)), root(0), entries);
            self.rig.ingest.delta(&identity, d).await.unwrap();
        }
        let set: BTreeSet<NfsPath> = paths.iter().cloned().collect();
        self.pipeline
            .process(vec![(
                self.lanes[lane].id.clone(),
                hub_registry::Work::Paths(set),
            )])
            .await
            .unwrap();
        paths
    }

    /// The indices of the editable files of a lane: structured YAML, not denied, not huge.
    pub fn editable(&self, lane: usize) -> Vec<usize> {
        self.lanes[lane]
            .files
            .iter()
            .enumerate()
            .filter(|(_, (p, b))| is_yaml(p) && b.len() < 200_000)
            .map(|(i, _)| i)
            .collect()
    }

    /// The index of the file of median size among the lane's editable tenant files that Git has (like the engine's benchmark).
    pub fn median_tenant_file(&self, lane: usize) -> usize {
        let l = &self.lanes[lane];
        let suffixes: Vec<String> = l.tenants.iter().map(|t| format!("-{t}")).collect();
        let mut candidates: Vec<(usize, usize)> = self
            .editable(lane)
            .into_iter()
            .filter(|&i| {
                let stem = l.files[i].0.file_name().rsplit_once('.').map_or("", |(s, _)| s);
                suffixes.iter().any(|s| stem.ends_with(s.as_str()))
            })
            .map(|i| (l.files[i].1.len(), i))
            .collect();
        candidates.sort_unstable();
        candidates[candidates.len() / 2].1
    }
}

pub fn entry(path: &NfsPath, content: Bytes, at: i64) -> ScanEntry {
    ScanEntry {
        path: path.clone(),
        hash: content_hash(&content),
        size: content.len() as u64,
        mtime: Timestamp::from_unix_millis(at - 1_000),
        observed_at: Timestamp::from_unix_millis(at),
        denied: false,
        bytes: Some(content),
    }
}

/// A new version of a file: the same text with a line that says which edit it is.
pub fn edit_text(original: &Bytes, round: u64) -> Bytes {
    let mut v = original.to_vec();
    if !v.ends_with(b"\n") {
        v.push(b'\n');
    }
    v.extend_from_slice(format!("bench_edit_{round}: {round}\n").as_bytes());
    Bytes::from(v)
}

/// Nearest-rank percentile of an ascending slice, the rule `bench-check` applies to its own samples.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    support::perf::percentile(sorted, p)
}
