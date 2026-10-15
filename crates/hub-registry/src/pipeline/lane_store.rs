//! The swimlanes the pipeline holds in memory (T3, plan "Pipeline", risk 6).
//!
//! A [`LaneEntry`] is what the pipeline needs to bring one swimlane up to date after some paths changed, and nothing more: the files'
//! metadata as the engine takes it (path, what the path is, class, the hash of the content on NFS, the hash in Git, the baseline, the
//! mark), the engine's [`LaneState`] (hashes and findings, never content), and the findings as the table holds them. **Content is not
//! kept.** A file's bytes are loaded only for the groups an update touches and dropped as soon as the engine is done, so 40 swimlanes
//! of 2,000 files cost tens of megabytes (the hashes and the paths), not the size of the configuration.
//!
//! Two kinds of file are not the engine's. A **denied** file (D79) and a file whose blob the store does not have have no content to give
//! the engine, and the engine hashes the content it is given, so they are computed by `drift_state` alone from the hashes the registry
//! stored ([`OpaqueFile`]): they get a drift state and a row, and no effective hash and no finding, which is also what "never an
//! effective of a denied file" asks.
//!
//! The store is bounded by a number of swimlanes. A swimlane that does not fit pushes out the least recently used one, which costs
//! that swimlane a full load the next time it is touched, and never a wrong answer.

use std::collections::BTreeMap;

use std::sync::Arc;

use domain::{
    AppName, ContentHash, DriftState, FileClass, FileKind, FileRole, LogicalFile, NfsPath, Severity,
    ShortText, SwimlaneId, TenantSet,
};
use engine::{
    Baseline, Drift, FileFacts, IntentionalMark, LaneFile, LaneInput, LaneState, SeverityRules, file_role,
};

use super::findings_sync::{FindingKey, FindingRow};
use super::git_side::Heads;
use crate::severity::file_severity;

/// A file the engine is not given, computed by `drift_state` from the hashes the registry stored.
#[derive(Debug, Clone)]
pub(crate) struct OpaqueFile {
    pub path: NfsPath,
    pub facts: FileFacts,
    pub class: FileClass,
    /// N: the hash of the file on NFS as the registry stored it for comparing; `None` when the file is not on NFS.
    pub nfs: Option<ContentHash>,
    /// G.
    pub git: Option<ContentHash>,
    pub baseline: Baseline,
    pub intentional: Option<IntentionalMark>,
    pub drift: Drift,
}

/// What a `file_state` row holds that the pipeline derives, as one comparable value. Two signatures that are equal are two rows that
/// need no write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowSig {
    pub logical: LogicalFile,
    pub kind: FileKind,
    pub role: FileRole,
    pub state: DriftState,
    pub git: Option<ContentHash>,
    pub baseline: Option<ContentHash>,
    /// N, for the paths named in `DriftChanged`: a file that changed on NFS is news even when its state did not move.
    pub nfs: Option<ContentHash>,
    pub class: FileClass,
    /// What the change that is pending is worth under the rules the swimlane was computed under (`None`: nothing is pending, T9).
    pub severity: Option<Severity>,
}

/// The derived columns of a stored row, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Persisted {
    pub state: String,
    pub git: Option<ContentHash>,
    pub baseline: Option<ContentHash>,
    pub logical: String,
    pub kind: String,
    pub role: Option<String>,
    pub severity: Option<String>,
}

impl RowSig {
    /// True when the two need the same row: everything but N, which ingestion writes and the pipeline does not.
    pub(crate) fn derived_eq(&self, other: &Self) -> bool {
        self.logical == other.logical
            && self.kind == other.kind
            && self.role == other.role
            && self.state == other.state
            && self.git == other.git
            && self.baseline == other.baseline
            && self.severity == other.severity
    }

    /// True when the stored row already holds what this signature says.
    pub(crate) fn matches(&self, stored: &Persisted) -> bool {
        stored.state == self.state.as_str()
            && stored.git == self.git
            && stored.baseline == self.baseline
            && stored.logical == self.logical.as_str()
            && stored.kind == self.kind.as_str()
            && stored.role.as_deref() == Some(self.role.as_str())
            && stored.severity.as_deref() == self.severity.map(Severity::as_str)
    }
}

/// B as a hash. A baseline of "the file was missing" is not stored (plan A24), so it reads as none.
pub(crate) fn baseline_hash(baseline: Baseline) -> Option<ContentHash> {
    match baseline {
        Baseline::Recorded(hash) => hash,
        Baseline::NotRecorded => None,
    }
}

/// One swimlane's state in memory.
pub(crate) struct LaneEntry {
    pub swimlane: SwimlaneId,
    pub tenants: TenantSet,
    pub apps: Vec<AppName>,
    pub tier: Option<ShortText>,
    pub fingerprint: ContentHash,
    pub adopted: bool,
    /// The severity rules the swimlane was computed under: what a file's row is rated with. They are part of `fingerprint`, so an entry is
    /// never used under other rules.
    pub severity: Arc<SeverityRules>,
    /// The commits the Git side was last computed at (for the reconcile and the logs).
    pub heads: Heads,
    /// The engine's input, sorted by path. The content of a file is empty here, except while the engine runs.
    pub files: Vec<LaneFile>,
    pub opaque: BTreeMap<NfsPath, OpaqueFile>,
    /// `None` only while a computation owns it.
    pub state: Option<LaneState>,
    /// The open findings of the engine's kinds, as the table holds them.
    pub findings: BTreeMap<FindingKey, FindingRow>,
    used: u64,
}

impl std::fmt::Debug for LaneEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaneEntry")
            .field("swimlane", &self.swimlane)
            .field("files", &self.files.len())
            .field("opaque", &self.opaque.len())
            .field("findings", &self.findings.len())
            .finish_non_exhaustive()
    }
}

impl LaneEntry {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        swimlane: SwimlaneId,
        tenants: TenantSet,
        apps: Vec<AppName>,
        tier: Option<ShortText>,
        fingerprint: ContentHash,
        adopted: bool,
        severity: Arc<SeverityRules>,
        heads: Heads,
        mut files: Vec<LaneFile>,
        opaque: BTreeMap<NfsPath, OpaqueFile>,
        state: LaneState,
    ) -> Self {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Self {
            swimlane,
            tenants,
            apps,
            tier,
            fingerprint,
            adopted,
            severity,
            heads,
            files,
            opaque,
            state: Some(state),
            findings: BTreeMap::new(),
            used: 0,
        }
    }

    pub(crate) fn pos(&self, path: &NfsPath) -> Result<usize, usize> {
        self.files.binary_search_by(|f| f.path.cmp(path))
    }

    pub(crate) fn file(&self, path: &NfsPath) -> Option<&LaneFile> {
        self.pos(path).ok().map(|i| &self.files[i])
    }

    /// Replace the file at `file.path`, or add it.
    pub(crate) fn put_file(&mut self, file: LaneFile) {
        self.opaque.remove(&file.path);
        match self.pos(&file.path) {
            Ok(i) => self.files[i] = file,
            Err(i) => self.files.insert(i, file),
        }
    }

    pub(crate) fn put_opaque(&mut self, file: OpaqueFile) {
        if let Ok(i) = self.pos(&file.path) {
            self.files.remove(i);
        }
        self.opaque.insert(file.path.clone(), file);
    }

    /// Forget a path: it is neither on NFS nor in Git.
    pub(crate) fn remove(&mut self, path: &NfsPath) {
        self.opaque.remove(path);
        if let Ok(i) = self.pos(path) {
            self.files.remove(i);
        }
    }

    /// The engine's view of the swimlane.
    pub(crate) fn input(&self) -> LaneInput<'_> {
        LaneInput {
            swimlane: &self.swimlane,
            tenants: &self.tenants,
            tier: self.tier.as_ref(),
            files: &self.files,
        }
    }

    /// The logical file of a path, as the engine's state or the opaque files know it.
    pub(crate) fn logical_of(&self, path: &NfsPath) -> Option<LogicalFile> {
        if let Some(o) = self.opaque.get(path) {
            return Some(o.facts.logical.clone());
        }
        self.file(path).map(|f| f.facts.logical.clone())
    }

    /// The signature of the row a path has now, or `None` when the path is not part of the swimlane.
    pub(crate) fn sig(&self, path: &NfsPath) -> Option<RowSig> {
        let role = file_role(path, &self.tenants, &self.apps);
        if let Some(o) = self.opaque.get(path) {
            return Some(RowSig {
                logical: o.facts.logical.clone(),
                kind: o.facts.kind,
                role,
                state: o.drift.state,
                git: o.git,
                baseline: baseline_hash(o.baseline),
                nfs: o.nfs,
                class: o.class,
                severity: file_severity(&self.severity, path, o.drift.state, o.class),
            });
        }
        let file = self.file(path)?;
        let state = self.state.as_ref()?.file(path)?;
        Some(RowSig {
            logical: state.logical.clone(),
            kind: file.facts.kind,
            role,
            state: state.drift.state,
            git: file.git,
            baseline: baseline_hash(file.baseline),
            nfs: state.hash,
            class: file.class,
            severity: file_severity(&self.severity, path, state.drift.state, file.class),
        })
    }

    /// Every path of the swimlane, the engine's and the opaque ones, in path order.
    pub(crate) fn paths(&self) -> Vec<NfsPath> {
        let mut paths: Vec<NfsPath> = self.files.iter().map(|f| f.path.clone()).collect();
        paths.extend(self.opaque.keys().cloned());
        paths.sort();
        paths
    }

    /// How many paths are in each drift state, for `swimlane_registry.state_counts`.
    pub(crate) fn state_counts(&self) -> BTreeMap<DriftState, u64> {
        let mut counts = BTreeMap::new();
        if let Some(state) = &self.state {
            for file in state.files() {
                *counts.entry(file.drift.state).or_insert(0) += 1;
            }
        }
        for file in self.opaque.values() {
            *counts.entry(file.drift.state).or_insert(0) += 1;
        }
        counts
    }
}

/// The swimlanes in memory, at most `cap` of them.
pub(crate) struct LaneStore {
    cap: usize,
    tick: u64,
    lanes: BTreeMap<SwimlaneId, LaneEntry>,
}

impl LaneStore {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            tick: 0,
            lanes: BTreeMap::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.lanes.len()
    }

    pub(crate) fn contains(&self, id: &SwimlaneId) -> bool {
        self.lanes.contains_key(id)
    }

    pub(crate) fn ids(&self) -> Vec<SwimlaneId> {
        self.lanes.keys().cloned().collect()
    }

    pub(crate) fn get(&self, id: &SwimlaneId) -> Option<&LaneEntry> {
        self.lanes.get(id)
    }

    /// Take an entry out to work on it. Put it back with [`put`](Self::put), or let it go: a swimlane that is not in the store is
    /// loaded again from the database, which is always right.
    pub(crate) fn take(&mut self, id: &SwimlaneId) -> Option<LaneEntry> {
        self.lanes.remove(id)
    }

    /// Add an entry. When the store is full the least recently used swimlane goes, and its id is returned.
    pub(crate) fn put(&mut self, mut entry: LaneEntry) -> Option<SwimlaneId> {
        self.tick += 1;
        entry.used = self.tick;
        let mut evicted = None;
        if !self.lanes.contains_key(&entry.swimlane) && self.lanes.len() >= self.cap {
            let oldest = self
                .lanes
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.lanes.remove(&id);
                evicted = Some(id);
            }
        }
        self.lanes.insert(entry.swimlane.clone(), entry);
        evicted
    }

    pub(crate) fn remove(&mut self, id: &SwimlaneId) {
        self.lanes.remove(id);
    }

    pub(crate) fn clear(&mut self) {
        self.lanes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{PathMappingRule, TenantId};
    use engine::{BatchRules, HostRules, Limits, classify_path, recompute_all};

    fn lane_state(id: &SwimlaneId) -> LaneState {
        let tenants = TenantSet::default();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let hosts = HostRules::default();
        let limits = Limits::default();
        let input = LaneInput {
            swimlane: id,
            tenants: &tenants,
            tier: None,
            files: &[],
        };
        let rules = BatchRules {
            application_names: &[],
            host_rules: &hosts,
            limits: &limits,
        };
        recompute_all(&pool, &[input], &rules).unwrap().remove(0)
    }

    fn entry(id: &str) -> LaneEntry {
        let swimlane = SwimlaneId::parse(id).unwrap();
        let state = lane_state(&swimlane);
        LaneEntry::new(
            swimlane,
            TenantSet::default(),
            Vec::new(),
            None,
            ContentHash::from_bytes([1; 32]),
            false,
            Arc::new(SeverityRules::new(engine::default_rules()).unwrap()),
            Heads::default(),
            Vec::new(),
            BTreeMap::new(),
            state,
        )
    }

    fn file(path: &str) -> LaneFile {
        let path = NfsPath::parse(path).unwrap();
        LaneFile {
            facts: classify_path(&PathMappingRule::default(), &path, &TenantSet::default(), |_| {
                true
            })
            .unwrap(),
            path,
            class: FileClass::Structured,
            nfs: None,
            git: Some(ContentHash::from_bytes([2; 32])),
            baseline: Baseline::NotRecorded,
            intentional: None,
            overwrite: None,
            history: None,
        }
    }

    #[test]
    fn files_are_kept_in_path_order_and_a_put_replaces() {
        let mut e = entry("sit1");
        for p in ["b.yml", "a.yml", "c/d.yml", "a.yml"] {
            e.put_file(file(p));
        }
        let paths: Vec<&str> = e.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.yml", "b.yml", "c/d.yml"]);
        e.remove(&NfsPath::parse("b.yml").unwrap());
        assert!(e.file(&NfsPath::parse("b.yml").unwrap()).is_none());
        assert_eq!(e.paths().len(), 2);
    }

    #[test]
    fn a_path_is_the_engines_or_opaque_never_both() {
        let mut e = entry("sit1");
        let path = NfsPath::parse("k.key").unwrap();
        e.put_file(file("k.key"));
        let f = e.files[0].clone();
        e.put_opaque(OpaqueFile {
            path: path.clone(),
            facts: f.facts.clone(),
            class: FileClass::Denied,
            nfs: None,
            git: None,
            baseline: Baseline::NotRecorded,
            intentional: None,
            drift: engine::drift_state(&engine::DriftInput {
                baseline: Baseline::NotRecorded,
                nfs: None,
                git: None,
                intentional: None,
            }),
        });
        assert!(e.file(&path).is_none() && e.opaque.contains_key(&path));
        e.put_file(file("k.key"));
        assert!(e.file(&path).is_some() && !e.opaque.contains_key(&path));
        assert_eq!(e.paths().len(), 1);
    }

    #[test]
    fn a_stored_row_matches_the_signature_that_says_the_same() {
        let sig = RowSig {
            logical: LogicalFile::parse("a/b.yml").unwrap(),
            kind: FileKind::Tenant,
            role: FileRole::Resource,
            state: DriftState::GitAhead,
            git: Some(ContentHash::from_bytes([3; 32])),
            baseline: None,
            nfs: None,
            class: FileClass::Structured,
            severity: Some(Severity::Low),
        };
        let stored = Persisted {
            state: "git_ahead".to_owned(),
            git: sig.git,
            baseline: None,
            logical: "a/b.yml".to_owned(),
            kind: "tenant".to_owned(),
            role: Some("resource".to_owned()),
            severity: Some("low".to_owned()),
        };
        assert!(sig.matches(&stored));
        for change in [
            Persisted {
                state: "in_sync".to_owned(),
                ..stored.clone()
            },
            Persisted {
                git: None,
                ..stored.clone()
            },
            Persisted {
                role: None,
                ..stored.clone()
            },
            Persisted {
                kind: "base".to_owned(),
                ..stored.clone()
            },
            Persisted {
                severity: Some("high".to_owned()),
                ..stored.clone()
            },
            Persisted {
                severity: None,
                ..stored.clone()
            },
        ] {
            assert!(!sig.matches(&change));
        }
        let other = RowSig {
            severity: Some(Severity::High),
            ..sig.clone()
        };
        assert!(
            !sig.derived_eq(&other),
            "a row whose rating changed is a row to write"
        );
    }

    #[test]
    fn the_store_is_bounded_and_the_least_recently_used_swimlane_goes() {
        let mut store = LaneStore::new(2);
        assert_eq!(store.put(entry("a")), None);
        assert_eq!(store.put(entry("b")), None);
        // Using `a` again (take and put back) makes `b` the oldest.
        let a = store.take(&SwimlaneId::parse("a").unwrap()).unwrap();
        assert_eq!(store.put(a), None);
        let evicted = store.put(entry("c"));
        assert_eq!(evicted, Some(SwimlaneId::parse("b").unwrap()));
        assert_eq!(store.len(), 2);
        assert!(store.contains(&SwimlaneId::parse("a").unwrap()));
        // Putting an entry back that is already there does not evict anything.
        let c = store.take(&SwimlaneId::parse("c").unwrap()).unwrap();
        assert_eq!(store.put(c), None);
        store.clear();
        assert_eq!(store.len(), 0);
        let _ = TenantId::parse("sit1");
    }

    #[test]
    fn state_counts_include_the_opaque_files() {
        let mut e = entry("sit1");
        let f = file("k.key");
        e.put_opaque(OpaqueFile {
            path: f.path.clone(),
            facts: f.facts,
            class: FileClass::Denied,
            nfs: Some(ContentHash::from_bytes([9; 32])),
            git: None,
            baseline: Baseline::NotRecorded,
            intentional: None,
            drift: engine::drift_state(&engine::DriftInput {
                baseline: Baseline::NotRecorded,
                nfs: Some(ContentHash::from_bytes([9; 32])),
                git: None,
                intentional: None,
            }),
        });
        let counts = e.state_counts();
        assert_eq!(counts.get(&DriftState::Untracked), Some(&1));
    }
}
