//! Findings by fingerprint (T3, the `findings` table).
//!
//! The engine gives a swimlane's findings as a list ([`engine::LaneState::findings`], checks C1 to C8). The table keeps them over
//! time: a finding that was there and still is keeps its row (and its `detected_at`, so "open since" means something), one that is
//! new gets a row, one that is gone is resolved, and one whose facts changed (its message, severity, lines) is updated in place. What
//! says "the same finding" is the **fingerprint**: the check and the path, and for the rare case that one check reports two results
//! for one path, the result's place among them in the engine's order. The message is not part of it on purpose: a drifted file that
//! goes from `git_ahead` to `conflict` is the same finding, worse.
//!
//! Only the engine's kinds are touched. C9 to C11 and the "as served" mismatch are written by other tasks and are never resolved
//! here, because this list does not contain them.
//!
//! Nothing in a row is a setting value, a file's text or an environment value: the engine's results name files, tenants, setting
//! paths, lines, hashes and host names (S13, S21), and [`detail_json`] copies exactly those.

use std::collections::BTreeMap;

use domain::{ContentHash, DriftState, FindingKind};
use engine::{
    ChangeContext, CheckDetail, CheckResult, EqualBy, LaneState, OrphanReason, RestartReason, SeverityRules,
    severity,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The kinds the engine computes for a lane state (C1 to C8). The others have other writers.
pub(crate) fn is_engine_kind(kind: FindingKind) -> bool {
    matches!(
        kind,
        FindingKind::MissedBaseChanges
            | FindingKind::RedundantTenantFile
            | FindingKind::UnevenTenantFiles
            | FindingKind::OrphanFile
            | FindingKind::Drift
            | FindingKind::EnvironmentHostMismatch
            | FindingKind::OverwrittenNfsChange
            | FindingKind::DuplicateKeys
    )
}

/// The wire names of the engine's kinds, for the statements that read and resolve them.
pub(crate) fn engine_kind_names() -> Vec<&'static str> {
    FindingKind::ALL
        .into_iter()
        .filter(|k| is_engine_kind(*k))
        .map(FindingKind::as_str)
        .collect()
}

/// One finding as the table holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FindingRow {
    pub kind: FindingKind,
    pub path: Option<String>,
    pub severity: domain::Severity,
    pub rule_ids: Vec<String>,
    pub message: String,
    pub locations: Value,
    pub details: Value,
    pub fingerprint: String,
}

/// The key of a row: unique among a swimlane's open findings (the table's partial unique index).
pub(crate) type FindingKey = (FindingKind, String);

fn drift_name(state: DriftState) -> &'static str {
    state.as_str()
}

fn label_name(label: engine::DriftLabel) -> &'static str {
    match label {
        engine::DriftLabel::DeletedOnNfs => "deleted_on_nfs",
        engine::DriftLabel::NewInGit => "new_in_git",
        engine::DriftLabel::DeletedInGit => "deleted_in_git",
    }
}

fn change_name(kind: domain::ChangeKind) -> &'static str {
    match kind {
        domain::ChangeKind::Added => "added",
        domain::ChangeKind::Removed => "removed",
        domain::ChangeKind::Changed => "changed",
        domain::ChangeKind::TypeChanged => "type_changed",
    }
}

fn restart_name(reason: RestartReason) -> String {
    format!("{reason:?}").to_lowercase()
}

fn hash_text(hash: Option<ContentHash>) -> Value {
    hash.map_or(Value::Null, |h| Value::String(h.to_string()))
}

/// The facts behind a result, as JSON. Names, hashes, counts and lines; never a value.
pub(crate) fn detail_json(detail: &CheckDetail) -> Value {
    match detail {
        CheckDetail::MissedBaseChanges {
            commits,
            commits_total,
            changes,
            changes_total,
            compared,
        } => json!({
            "check": "missed_base_changes",
            "commits": commits.iter().map(|c| json!({"commit": c.commit.as_str(), "time": c.time.unix_millis()})).collect::<Vec<_>>(),
            "commits_total": commits_total,
            "changes": changes.iter().map(|c| json!({"path": c.path.as_str(), "kind": change_name(c.kind)})).collect::<Vec<_>>(),
            "changes_total": changes_total,
            "compared": compared,
        }),
        CheckDetail::DeadOverrides { settings, total } => json!({
            "check": "dead_overrides",
            "settings": settings.iter().map(domain::SettingPath::as_str).collect::<Vec<_>>(),
            "total": total,
        }),
        CheckDetail::RedundantTenantFile { tenant, equal_by } => json!({
            "check": "redundant_tenant_file",
            "tenant": tenant.as_str(),
            "equal_by": match equal_by { EqualBy::Bytes => "bytes", EqualBy::Settings => "settings" },
        }),
        CheckDetail::UnevenTenantFiles {
            with_file,
            without_file,
        } => json!({
            "check": "uneven_tenant_files",
            "with_file": with_file.iter().map(domain::TenantId::as_str).collect::<Vec<_>>(),
            "without_file": without_file.iter().map(domain::TenantId::as_str).collect::<Vec<_>>(),
        }),
        CheckDetail::OrphanFile { reason } => match reason {
            OrphanReason::TenantNotDeployed(tenant) => json!({
                "check": "orphan_file",
                "reason": "tenant_not_deployed",
                "tenant": tenant.as_str(),
            }),
            OrphanReason::Untracked => json!({"check": "orphan_file", "reason": "untracked"}),
        },
        CheckDetail::Drift { state, label } => json!({
            "check": "drift",
            "state": drift_name(*state),
            "label": label.map(label_name),
        }),
        CheckDetail::EnvironmentHostMismatch {
            swimlane_tier,
            hosts,
            total,
        } => json!({
            "check": "environment_host_mismatch",
            "swimlane_tier": swimlane_tier.as_str(),
            "hosts": hosts.iter().map(|h| json!({"path": h.path.as_str(), "host": h.host, "host_tier": h.host_tier.as_str()})).collect::<Vec<_>>(),
            "total": total,
        }),
        CheckDetail::OverwrittenNfsChange {
            previous_state,
            lost_version,
            source,
        } => json!({
            "check": "overwritten_nfs_change",
            "previous_state": drift_name(*previous_state),
            "lost_version": hash_text(*lost_version),
            "source": source.as_str(),
        }),
        CheckDetail::DuplicateKeys {
            keys,
            total,
            more_not_listed,
        } => json!({
            "check": "duplicate_keys",
            "keys": keys.iter().map(|k| json!({"path": k.path.as_ref().map(domain::SettingPath::as_str), "lines": k.lines})).collect::<Vec<_>>(),
            "total": total,
            "more_not_listed": more_not_listed,
        }),
        CheckDetail::AmbiguousFileName { name, folders, total } => json!({
            "check": "ambiguous_file_name",
            "name": name,
            "folders": folders.iter().map(domain::NfsPath::as_str).collect::<Vec<_>>(),
            "total": total,
        }),
        CheckDetail::UnresolvedPlaceholders { keys, total } => json!({
            "check": "unresolved_placeholder",
            "keys": keys.len(),
            "total": total,
        }),
        CheckDetail::ConfigServerRestart {
            reason,
            folders,
            channels,
            total,
        } => json!({
            "check": "config_server_restart_required",
            "reason": restart_name(*reason),
            "folders": folders.iter().map(domain::NfsPath::as_str).collect::<Vec<_>>(),
            "channels": channels.len(),
            "total": total,
        }),
    }
}

/// The fingerprint of the `ordinal`-th result of a check for a path.
fn fingerprint(kind: FindingKind, path: Option<&str>, ordinal: usize) -> String {
    let mut hash = Sha256::new();
    hash.update(kind.as_str().as_bytes());
    hash.update([0]);
    hash.update(path.unwrap_or("").as_bytes());
    hash.update([0]);
    hash.update((ordinal as u64).to_be_bytes());
    ContentHash::from_bytes(hash.finalize().into()).to_string()
}

/// A swimlane's findings as rows, keyed by kind and fingerprint, with their severity from `rules`.
pub(crate) fn rows_of(state: &LaneState, rules: &SeverityRules) -> BTreeMap<FindingKey, FindingRow> {
    let mut rows = BTreeMap::new();
    let mut seen: BTreeMap<(FindingKind, Option<String>), usize> = BTreeMap::new();
    for result in state.findings() {
        if !is_engine_kind(result.kind) {
            continue;
        }
        let path = result.path.as_ref().map(|p| p.as_str().to_owned());
        let ordinal = {
            let n = seen.entry((result.kind, path.clone())).or_insert(0);
            let ordinal = *n;
            *n += 1;
            ordinal
        };
        let fingerprint = fingerprint(result.kind, path.as_deref(), ordinal);
        let row = row_of(state, rules, &result, path, fingerprint.clone());
        rows.insert((result.kind, fingerprint), row);
    }
    rows
}

fn row_of(
    state: &LaneState,
    rules: &SeverityRules,
    result: &CheckResult,
    path: Option<String>,
    fingerprint: String,
) -> FindingRow {
    let file = result.path.as_ref().and_then(|p| state.file(p));
    let outcome = severity(
        &ChangeContext {
            finding_kind: Some(result.kind),
            path: result.path.as_ref(),
            drift_state: file.map(|f| f.drift.state),
            file_class: file.map(|f| f.class),
            ..ChangeContext::default()
        },
        rules,
    );
    FindingRow {
        kind: result.kind,
        path,
        severity: outcome.severity,
        rule_ids: outcome.rule_ids.iter().map(|r| r.as_str().to_owned()).collect(),
        message: result.message.as_str().to_owned(),
        locations: serde_json::to_value(&result.locations).unwrap_or(Value::Null),
        details: detail_json(&result.detail),
        fingerprint,
    }
}

/// What to write to take the table from `old` to `new`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FindingsDelta {
    pub insert: Vec<FindingRow>,
    pub update: Vec<FindingRow>,
    pub resolve: Vec<FindingKey>,
}

impl FindingsDelta {
    pub(crate) fn is_empty(&self) -> bool {
        self.insert.is_empty() && self.update.is_empty() && self.resolve.is_empty()
    }

    pub(crate) fn total(&self) -> usize {
        self.insert.len() + self.update.len() + self.resolve.len()
    }
}

/// The difference between two sets of rows.
pub(crate) fn diff(
    old: &BTreeMap<FindingKey, FindingRow>,
    new: &BTreeMap<FindingKey, FindingRow>,
) -> FindingsDelta {
    let mut delta = FindingsDelta::default();
    for (key, row) in new {
        match old.get(key) {
            None => delta.insert.push(row.clone()),
            Some(before) if before != row => delta.update.push(row.clone()),
            Some(_) => {}
        }
    }
    for key in old.keys() {
        if !new.contains_key(key) {
            delta.resolve.push(key.clone());
        }
    }
    delta
}

/// A row as the table stores it, the other way round: what `SELECT` gives, parsed. A row this code did not write (a kind or a value it
/// does not know) is `None` and so is resolved or replaced by the next sync.
pub(crate) type StoredFinding = (
    String,
    Option<String>,
    String,
    Vec<String>,
    String,
    Value,
    Value,
    String,
);

pub(crate) fn row_from_stored(stored: StoredFinding) -> Option<FindingRow> {
    let (kind, path, severity, rule_ids, message, locations, details, fingerprint) = stored;
    Some(FindingRow {
        kind: kind.parse().ok()?,
        path,
        severity: severity.parse().ok()?,
        rule_ids,
        message,
        locations,
        details,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{FileClass, Severity};

    fn row(kind: FindingKind, path: &str, message: &str, fp: &str) -> FindingRow {
        FindingRow {
            kind,
            path: Some(path.to_owned()),
            severity: Severity::Medium,
            rule_ids: vec!["default.drift".to_owned()],
            message: message.to_owned(),
            locations: json!([]),
            details: json!({"check": "drift"}),
            fingerprint: fp.to_owned(),
        }
    }

    fn map(rows: Vec<FindingRow>) -> BTreeMap<FindingKey, FindingRow> {
        rows.into_iter()
            .map(|r| ((r.kind, r.fingerprint.clone()), r))
            .collect()
    }

    #[test]
    fn the_engine_kinds_are_c1_to_c8_and_no_other() {
        let names = engine_kind_names();
        assert_eq!(names.len(), 8);
        for kind in FindingKind::ALL {
            let code = kind.code();
            let n: u8 = code.trim_start_matches('C').parse().unwrap();
            assert_eq!(is_engine_kind(kind), n <= 8, "{code}");
            assert_eq!(names.contains(&kind.as_str()), n <= 8, "{code}");
        }
    }

    #[test]
    fn the_fingerprint_is_the_check_the_path_and_the_place_among_equals() {
        let a = fingerprint(FindingKind::Drift, Some("a.yml"), 0);
        assert_eq!(a, fingerprint(FindingKind::Drift, Some("a.yml"), 0));
        assert_ne!(a, fingerprint(FindingKind::Drift, Some("a.yml"), 1));
        assert_ne!(a, fingerprint(FindingKind::Drift, Some("b.yml"), 0));
        assert_ne!(a, fingerprint(FindingKind::OrphanFile, Some("a.yml"), 0));
        assert_ne!(
            fingerprint(FindingKind::Drift, Some("a"), 0),
            fingerprint(FindingKind::Drift, Some(""), 0),
            "a path and no path differ"
        );
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn a_new_finding_is_inserted_a_gone_one_resolved_and_a_changed_one_updated_in_place() {
        let kept = row(FindingKind::Drift, "keep.yml", "git ahead", "f1");
        let worse = row(FindingKind::Drift, "worse.yml", "git ahead", "f2");
        let gone = row(FindingKind::Drift, "gone.yml", "nfs ahead", "f3");
        let old = map(vec![kept.clone(), worse.clone(), gone.clone()]);

        let worse_now = row(FindingKind::Drift, "worse.yml", "conflict", "f2");
        let fresh = row(FindingKind::OrphanFile, "new.yml", "untracked", "f4");
        let new = map(vec![kept, worse_now.clone(), fresh.clone()]);

        let d = diff(&old, &new);
        assert_eq!(d.insert, [fresh]);
        assert_eq!(
            d.update,
            [worse_now],
            "the same finding, worse: updated, not reopened"
        );
        assert_eq!(d.resolve, [(FindingKind::Drift, "f3".to_owned())]);
        assert_eq!(d.total(), 3);
        assert!(diff(&new, &new).is_empty());
        assert!(diff(&BTreeMap::new(), &BTreeMap::new()).is_empty());
    }

    #[test]
    fn a_stored_row_of_a_kind_or_severity_this_code_does_not_know_is_not_a_row() {
        let ok = (
            "drift".to_owned(),
            Some("a.yml".to_owned()),
            "high".to_owned(),
            vec!["r".to_owned()],
            "m".to_owned(),
            json!([]),
            json!({}),
            "fp".to_owned(),
        );
        assert!(row_from_stored(ok.clone()).is_some());
        let mut bad_kind = ok.clone();
        bad_kind.0 = "mystery".to_owned();
        assert!(row_from_stored(bad_kind).is_none());
        let mut bad_severity = ok;
        bad_severity.2 = "meh".to_owned();
        assert!(row_from_stored(bad_severity).is_none());
    }

    #[test]
    fn the_details_name_facts_and_never_a_value() {
        let d = detail_json(&CheckDetail::Drift {
            state: DriftState::Conflict,
            label: Some(engine::DriftLabel::NewInGit),
        });
        assert_eq!(d["state"], "conflict");
        assert_eq!(d["label"], "new_in_git");
        let none = detail_json(&CheckDetail::OrphanFile {
            reason: OrphanReason::Untracked,
        });
        assert_eq!(none["reason"], "untracked");
        // The one place a hash goes in, as 64 hex digits.
        let lost = ContentHash::from_bytes([0xab; 32]);
        let o = detail_json(&CheckDetail::OverwrittenNfsChange {
            previous_state: DriftState::NfsAhead,
            lost_version: Some(lost),
            source: domain::AttributionSource::SyncJob,
        });
        assert_eq!(o["lost_version"], "ab".repeat(32));
        assert_eq!(o["source"], "sync_job");
        let _ = FileClass::Structured;
    }

    #[test]
    fn the_severity_comes_from_the_rules_given() {
        let rules = SeverityRules::new(engine::default_rules()).unwrap();
        let state = {
            // An empty swimlane has no findings: the function must cope with that.
            let id = domain::SwimlaneId::parse("sit1").unwrap();
            let tenants = domain::TenantSet::default();
            let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
            let hosts = engine::HostRules::default();
            let limits = engine::Limits::default();
            let lane = engine::LaneInput {
                swimlane: &id,
                tenants: &tenants,
                tier: None,
                files: &[],
            };
            let rules = engine::BatchRules {
                application_names: &[],
                host_rules: &hosts,
                limits: &limits,
            };
            engine::recompute_all(&pool, &[lane], &rules).unwrap().remove(0)
        };
        assert!(rows_of(&state, &rules).is_empty());
    }
}
