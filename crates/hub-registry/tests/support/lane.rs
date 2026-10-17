//! Engine lane states for the tests of the tree hashes: a swimlane of base files, computed the way the pipeline computes it (the
//! engine's full recompute), so the hashes under test are the engine's and not a copy of its rules.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use bytes::Bytes;
use domain::{LogicalFile, NfsPath, PathMappingRule, SwimlaneId, TenantId, TenantSet};
use engine::{
    Baseline, BatchRules, DenyMatcher, HostRules, LaneFile, LaneInput, LaneState, Limits, NfsContent,
    classify, classify_path, drift_hash, recompute_all,
};
use ports::content_hash;

pub fn logical(s: &str) -> LogicalFile {
    LogicalFile::parse(s).unwrap()
}

/// The state of swimlane `swimlane` with these files (path and text), all of them resources identical in Git (so in sync). No
/// application names: no file is a property source, so a file's effective hash is the hash of the file itself.
pub fn state(swimlane: &str, files: &BTreeMap<String, String>) -> LaneState {
    let id = SwimlaneId::parse(swimlane).unwrap();
    let tenants = TenantSet::new([TenantId::parse("sit1").unwrap()]).unwrap();
    let rule = PathMappingRule::default();
    let lane_files: Vec<LaneFile> = files
        .iter()
        .map(|(path, text)| {
            let path = NfsPath::parse(path).unwrap();
            let bytes = Bytes::from(text.clone());
            let class = classify(&path, Some(&bytes), &DenyMatcher::default());
            let facts = classify_path(&rule, &path, &tenants, |_| true).unwrap();
            let git = drift_hash(Some(&bytes), class).unwrap();
            LaneFile {
                path,
                facts,
                class,
                nfs: Some(NfsContent {
                    blob: content_hash(&bytes),
                    bytes,
                }),
                git: Some(git),
                baseline: Baseline::Recorded(Some(git)),
                intentional: None,
                overwrite: None,
                history: None,
            }
        })
        .collect();
    let input = LaneInput {
        swimlane: &id,
        tenants: &tenants,
        tier: None,
        files: &lane_files,
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let hosts = HostRules::default();
    let limits = Limits::default();
    let rules = BatchRules {
        application_names: &[],
        host_rules: &hosts,
        limits: &limits,
    };
    recompute_all(&pool, &[input], &rules).unwrap().remove(0)
}
