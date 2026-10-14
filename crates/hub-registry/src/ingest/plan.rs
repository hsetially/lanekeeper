//! What becomes of the versions of one path in a scan delta, decided without touching the database.
//!
//! Given the state the hub stores for a path and the versions the delta carries for it (oldest first), [`plan_path`] says
//! which versions are new observations, which of them move the current state, which are out of band, and how each is
//! attributed (D73). Keeping it pure makes the rules testable on their own and the apply code a thin layer of SQL.
//!
//! The rules (plan 05, T1, A10 to A12):
//!
//! - A version equal to the current one changes nothing, so a full scan that finds what the hub already has writes nothing.
//! - A version observed *before* the stored one is history: it is recorded and does not move the state back.
//! - A removal of a file that is not there changes nothing; a removal is ordered right after what it removes (a removal
//!   carries no time of its own).
//! - A hash a registered tool write explains is not out of band. A change observed while a sync Job was running (the delta says
//!   so, or a window of `sync_windows` was open) is attributed to the Job. The first inventory of a swimlane is not a change.
//!   Everything else is out of band: it is audited, with the author unknown until the sentinel (T7) says otherwise.

use std::collections::HashMap;

use domain::{
    Actor, Attribution, AttributionSource, Confidence, ContentHash, Evidence, JobRef, NfsPath,
    ObservationSource, Timestamp,
};

use super::validate::StagedEntry;

/// A tool write whose observation is older than the state it would explain by more than this does not explain it. Clocks of the
/// hub (tool writes) and the agent (scans) differ a little, and a scan that sees the old content a moment before a write is
/// followed by one that sees the new.
pub(crate) const TOOL_WRITE_SLACK_MS: i64 = 60_000;

/// What the hub stores for a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
    pub hash: Option<ContentHash>,
    /// NFS has reported this path at some time (a file, or the trace of a removed one). A row that Git alone gave the path (the pipeline
    /// creates one for every file a repository has and NFS does not) has no observation, and for what NFS says about the path it is
    /// as good as no row.
    pub observed: bool,
    pub observed_at_ms: i64,
    pub class: domain::FileClass,
    pub denied: bool,
}

/// A registered tool write of a hash (A10), as the database has it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolWrite {
    pub observed_at_ms: i64,
    pub actor: Option<Actor>,
}

/// A sync Job's window, from `sync_windows`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Window {
    pub job: JobRef,
    pub opened_ms: i64,
    pub closed_ms: Option<i64>,
}

impl Window {
    fn contains(&self, at_ms: i64) -> bool {
        self.opened_ms <= at_ms && self.closed_ms.is_none_or(|c| at_ms <= c)
    }
}

/// One version of a path in a delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Version {
    File {
        entry: Box<StagedEntry>,
        during_job: Option<JobRef>,
    },
    Removal,
}

/// What the plan needs to know that is not about the path.
pub(crate) struct PlanCtx<'a> {
    /// The swimlane has had no inventory yet, so a path it has never seen is part of the first one.
    pub initial_inventory: bool,
    /// The newest time in the logical delta; a removal is placed at this time, or right after the state it removes.
    pub delta_time_ms: i64,
    pub windows: &'a [Window],
    /// Tool writes of this path, by the hash they wrote (`None`: a deletion).
    pub tool_writes: &'a HashMap<Option<ContentHash>, ToolWrite>,
}

/// What the path was before a version of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The path had a state (a file, or the trace of a removed one).
    Known,
    /// The path had no state: a file nobody had seen. After the first inventory this is a change.
    New,
    /// The path had no state and the swimlane has had no inventory yet: part of the first inventory, which is not a change (A11).
    Inventory,
}

/// One observation to record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub path: NfsPath,
    /// `None` is a removal.
    pub hash: Option<ContentHash>,
    pub entry: Option<StagedEntry>,
    pub observed_at_ms: i64,
    pub source: ObservationSource,
    pub during_job: Option<JobRef>,
    pub attribution: Attribution,
    /// This version is the path's current state once recorded.
    pub moves_state: bool,
    /// What the path was before this version.
    pub origin: Origin,
    /// Nobody explains this change: audit it.
    pub out_of_band: bool,
    /// The hash the path had before this version moved it.
    pub before: Option<ContentHash>,
}

impl Candidate {
    /// The path had no state before this version.
    pub(crate) fn first_sight(&self) -> bool {
        self.origin != Origin::Known
    }

    /// The version is part of the first inventory of the swimlane: not a change.
    pub(crate) fn initial(&self) -> bool {
        self.origin == Origin::Inventory
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct PathPlan {
    pub candidates: Vec<Candidate>,
    /// Versions equal to the state, which need nothing.
    pub unchanged: usize,
}

/// Plan the versions of one path. `versions` is in the order the delta carries them; files are taken oldest first, and a
/// removal after the files.
pub(crate) fn plan_path(
    path: &NfsPath,
    stored: Option<&Stored>,
    versions: &[Version],
    ctx: &PlanCtx<'_>,
) -> PathPlan {
    let mut files: Vec<&Version> = versions
        .iter()
        .filter(|v| matches!(v, Version::File { .. }))
        .collect();
    files.sort_by_key(|v| match v {
        Version::File { entry, .. } => entry.observed_at_ms,
        Version::Removal => i64::MAX,
    });
    let removals = versions.iter().filter(|v| matches!(v, Version::Removal));

    let stored = stored.filter(|s| s.observed);
    let existed = stored.is_some();
    let mut cur_hash = stored.and_then(|s| s.hash);
    let mut cur_at = stored.map_or(-1, |s| s.observed_at_ms);
    let mut plan = PathPlan::default();

    for version in files.into_iter().chain(removals) {
        match version {
            Version::File { entry, during_job } => {
                if cur_hash == Some(entry.hash) {
                    plan.unchanged += 1;
                    continue;
                }
                let at = entry.observed_at_ms;
                let moves_state = at >= cur_at;
                let origin = if !existed && cur_hash.is_none() && cur_at < 0 {
                    if ctx.initial_inventory {
                        Origin::Inventory
                    } else {
                        Origin::New
                    }
                } else {
                    Origin::Known
                };
                let explained = explained_by_tool_write(ctx, Some(entry.hash), cur_at);
                let (attribution, source, job) = attribute(explained, during_job.as_ref(), ctx.windows, at);
                plan.candidates.push(Candidate {
                    path: path.clone(),
                    hash: Some(entry.hash),
                    entry: Some((**entry).clone()),
                    observed_at_ms: at,
                    source,
                    during_job: job,
                    out_of_band: origin != Origin::Inventory
                        && explained.is_none()
                        && source == ObservationSource::Scan,
                    attribution,
                    moves_state,
                    origin,
                    before: cur_hash,
                });
                if moves_state {
                    cur_hash = Some(entry.hash);
                    cur_at = at;
                }
            }
            Version::Removal => {
                let Some(before) = cur_hash else {
                    plan.unchanged += 1;
                    continue;
                };
                // The agent stamps every entry of a message with the time of the message. A delta that carries such a time and is
                // older than the state it would remove is a replay of the past: the file was seen again since. A removal-only
                // delta has no time, and is placed right after the state it removes.
                if ctx.delta_time_ms > 0 && ctx.delta_time_ms < cur_at {
                    plan.unchanged += 1;
                    continue;
                }
                let at = ctx.delta_time_ms.max(cur_at);
                let explained = explained_by_tool_write(ctx, None, cur_at);
                let (attribution, source, job) = attribute(explained, None, ctx.windows, at);
                plan.candidates.push(Candidate {
                    path: path.clone(),
                    hash: None,
                    entry: None,
                    observed_at_ms: at,
                    source,
                    during_job: job,
                    out_of_band: explained.is_none() && source == ObservationSource::Scan,
                    attribution,
                    moves_state: true,
                    origin: Origin::Known,
                    before: Some(before),
                });
                cur_hash = None;
                cur_at = at;
            }
        }
    }
    plan
}

fn explained_by_tool_write<'a>(
    ctx: &'a PlanCtx<'_>,
    hash: Option<ContentHash>,
    state_observed_at_ms: i64,
) -> Option<&'a ToolWrite> {
    ctx.tool_writes
        .get(&hash)
        .filter(|w| w.observed_at_ms >= state_observed_at_ms.saturating_sub(TOOL_WRITE_SLACK_MS))
}

/// The attribution a change gets when it is first seen (D73, steps 1 and 3). The sentinel (T7) upgrades it later; it is never
/// downgraded.
fn attribute(
    tool_write: Option<&ToolWrite>,
    during_job: Option<&JobRef>,
    windows: &[Window],
    at_ms: i64,
) -> (Attribution, ObservationSource, Option<JobRef>) {
    let evidence = Evidence {
        observed_at: Some(Timestamp::from_unix_millis(at_ms)),
        ..Evidence::default()
    };
    if let Some(w) = tool_write {
        return (
            Attribution {
                source: AttributionSource::ToolWrite,
                confidence: Confidence::Certain,
                actor: w.actor.clone(),
                evidence,
            },
            ObservationSource::Scan,
            None,
        );
    }
    let job = during_job
        .cloned()
        .or_else(|| windows.iter().find(|w| w.contains(at_ms)).map(|w| w.job.clone()));
    if let Some(job) = job {
        return (
            Attribution {
                source: AttributionSource::SyncJob,
                confidence: Confidence::High,
                actor: Some(Actor::Job { job: job.clone() }),
                evidence,
            },
            ObservationSource::Sync,
            Some(job),
        );
    }
    (
        Attribution {
            evidence,
            ..Attribution::unknown()
        },
        ObservationSource::Scan,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{FileClass, UserId};
    use ports::content_hash;

    fn h(s: &str) -> ContentHash {
        content_hash(s.as_bytes())
    }

    fn path() -> NfsPath {
        NfsPath::parse("svc/app.yml").unwrap()
    }

    fn file(hash: &str, at: i64) -> Version {
        Version::File {
            entry: Box::new(StagedEntry {
                path: path(),
                hash: h(hash),
                drift_hash: h(hash),
                size: 1,
                mtime_ms: 0,
                observed_at_ms: at,
                denied: false,
                class: FileClass::Structured,
                eol: None,
                encoding: None,
                bom: None,
                has_content: true,
            }),
            during_job: None,
        }
    }

    fn stored(hash: Option<&str>, at: i64) -> Stored {
        Stored {
            hash: hash.map(h),
            observed: true,
            observed_at_ms: at,
            class: FileClass::Structured,
            denied: false,
        }
    }

    fn ctx<'a>(
        initial: bool,
        windows: &'a [Window],
        tool_writes: &'a HashMap<Option<ContentHash>, ToolWrite>,
    ) -> PlanCtx<'a> {
        PlanCtx {
            initial_inventory: initial,
            delta_time_ms: 1_000,
            windows,
            tool_writes,
        }
    }

    fn plan(stored: Option<&Stored>, versions: &[Version], initial: bool) -> PathPlan {
        plan_path(&path(), stored, versions, &ctx(initial, &[], &HashMap::new()))
    }

    #[test]
    fn a_version_equal_to_the_current_one_changes_nothing() {
        let p = plan(Some(&stored(Some("a"), 10)), &[file("a", 20)], false);
        assert!(p.candidates.is_empty());
        assert_eq!(p.unchanged, 1);
    }

    #[test]
    fn a_new_version_moves_the_state_and_is_out_of_band() {
        let p = plan(Some(&stored(Some("a"), 10)), &[file("b", 20)], false);
        let c = &p.candidates[0];
        assert!(c.moves_state && c.out_of_band && !c.initial() && !c.first_sight());
        assert_eq!(c.before, Some(h("a")));
        assert_eq!(c.attribution.source, AttributionSource::Unknown);
        assert_eq!(c.source, ObservationSource::Scan);
    }

    #[test]
    fn versions_are_taken_oldest_first_and_each_change_is_recorded() {
        // The spool replays a file that went a -> b -> a while the hub was away: both changes are in the history.
        let p = plan(
            Some(&stored(Some("a"), 5)),
            &[file("a", 40), file("b", 20)],
            false,
        );
        let hashes: Vec<_> = p.candidates.iter().map(|c| c.hash).collect();
        assert_eq!(hashes, vec![Some(h("b")), Some(h("a"))]);
        assert!(p.candidates.iter().all(|c| c.moves_state));
        assert_eq!(p.candidates[1].before, Some(h("b")));
    }

    #[test]
    fn a_version_older_than_the_stored_one_is_history_only() {
        let p = plan(Some(&stored(Some("b"), 100)), &[file("a", 50)], false);
        let c = &p.candidates[0];
        assert!(!c.moves_state, "an older version never moves the state back");
        assert_eq!(c.hash, Some(h("a")));
    }

    #[test]
    fn the_first_inventory_is_not_a_change() {
        let p = plan(None, &[file("a", 10)], true);
        let c = &p.candidates[0];
        assert!(c.initial() && c.first_sight() && c.moves_state);
        assert!(!c.out_of_band, "A11: no audit event for the first inventory");
        // The same file seen for the first time after the inventory is a change.
        let later = plan(None, &[file("a", 10)], false);
        assert!(later.candidates[0].out_of_band && !later.candidates[0].initial());
        assert_eq!(later.candidates[0].origin, Origin::New);
    }

    #[test]
    fn a_registered_tool_write_explains_the_hash() {
        let user = domain::UserId::new(
            domain::Guid::parse("11111111-1111-1111-1111-111111111111").unwrap(),
            domain::Guid::parse("22222222-2222-2222-2222-222222222222").unwrap(),
        );
        let _: &UserId = &user;
        let mut writes = HashMap::new();
        writes.insert(
            Some(h("b")),
            ToolWrite {
                observed_at_ms: 25,
                actor: Some(Actor::User { user }),
            },
        );
        let c = ctx(false, &[], &writes);
        let p = plan_path(&path(), Some(&stored(Some("a"), 10)), &[file("b", 30)], &c);
        let cand = &p.candidates[0];
        assert!(!cand.out_of_band);
        assert_eq!(cand.attribution.source, AttributionSource::ToolWrite);
        assert_eq!(cand.attribution.confidence, Confidence::Certain);
        assert!(matches!(cand.attribution.actor, Some(Actor::User { .. })));
        assert_eq!(cand.source, ObservationSource::Scan);
    }

    #[test]
    fn an_old_tool_write_does_not_explain_a_later_change_to_the_same_hash() {
        // a -> (tool write) b -> (out of band) a -> (out of band) b: the last b is not the tool write's.
        let mut writes = HashMap::new();
        writes.insert(
            Some(h("b")),
            ToolWrite {
                observed_at_ms: 1_000,
                actor: None,
            },
        );
        let c = ctx(false, &[], &writes);
        let state = stored(Some("a"), 1_000_000);
        let p = plan_path(&path(), Some(&state), &[file("b", 1_000_100)], &c);
        assert!(p.candidates[0].out_of_band);
    }

    #[test]
    fn a_job_in_the_delta_or_an_open_window_attributes_the_change_to_the_job() {
        let job = JobRef::new("sync-sit1", "uid-1").unwrap();
        let mut with_job = file("b", 20);
        if let Version::File { during_job, .. } = &mut with_job {
            *during_job = Some(job.clone());
        }
        let p = plan(Some(&stored(Some("a"), 10)), &[with_job], false);
        let c = &p.candidates[0];
        assert_eq!(c.attribution.source, AttributionSource::SyncJob);
        assert_eq!(c.attribution.confidence, Confidence::High);
        assert_eq!(c.source, ObservationSource::Sync);
        assert!(!c.out_of_band);
        assert_eq!(c.during_job.as_ref(), Some(&job));

        let windows = [Window {
            job: job.clone(),
            opened_ms: 15,
            closed_ms: Some(30),
        }];
        let none = HashMap::new();
        let c2 = ctx(false, &windows, &none);
        let inside = plan_path(&path(), Some(&stored(Some("a"), 10)), &[file("b", 20)], &c2);
        assert_eq!(
            inside.candidates[0].attribution.source,
            AttributionSource::SyncJob
        );
        let outside = plan_path(&path(), Some(&stored(Some("a"), 10)), &[file("b", 31)], &c2);
        assert_eq!(
            outside.candidates[0].attribution.source,
            AttributionSource::Unknown
        );
        assert!(outside.candidates[0].out_of_band);
    }

    #[test]
    fn a_removal_is_placed_after_what_it_removes() {
        // A delta with no time of its own (removals only): right after the state it removes.
        let p = plan_path(
            &path(),
            Some(&stored(Some("a"), 5_000)),
            &[Version::Removal],
            &PlanCtx {
                delta_time_ms: 0,
                ..ctx(false, &[], &HashMap::new())
            },
        );
        let c = &p.candidates[0];
        assert_eq!(c.hash, None);
        assert_eq!(c.observed_at_ms, 5_000, "never before the state it removes");
        assert!(c.moves_state && c.out_of_band);
        assert_eq!(c.before, Some(h("a")));
        let later = plan_path(
            &path(),
            Some(&stored(Some("a"), 500)),
            &[Version::Removal],
            &PlanCtx {
                delta_time_ms: 900,
                ..ctx(false, &[], &HashMap::new())
            },
        );
        assert_eq!(later.candidates[0].observed_at_ms, 900);
    }

    #[test]
    fn a_removal_in_a_delta_older_than_the_state_is_a_replay_and_changes_nothing() {
        // The delta is dated 900, the file was seen again at 5,000: removing it now would delete a file that exists.
        let p = plan_path(
            &path(),
            Some(&stored(Some("a"), 5_000)),
            &[Version::Removal],
            &PlanCtx {
                delta_time_ms: 900,
                ..ctx(false, &[], &HashMap::new())
            },
        );
        assert!(p.candidates.is_empty());
        assert_eq!(p.unchanged, 1);
    }

    #[test]
    fn a_removal_of_nothing_changes_nothing() {
        assert!(plan(None, &[Version::Removal], false).candidates.is_empty());
        assert!(
            plan(Some(&stored(None, 5)), &[Version::Removal], false)
                .candidates
                .is_empty()
        );
    }

    #[test]
    fn a_file_that_comes_back_after_removal_is_a_new_version() {
        let p = plan(Some(&stored(None, 100)), &[file("a", 200)], false);
        let c = &p.candidates[0];
        assert!(c.moves_state && !c.first_sight(), "the path has been seen before");
        assert_eq!(c.before, None);
    }

    #[test]
    fn a_denied_file_is_planned_like_any_other() {
        let mut v = file("k", 20);
        if let Version::File { entry, .. } = &mut v {
            entry.denied = true;
            entry.class = FileClass::Denied;
            entry.has_content = false;
        }
        let p = plan(Some(&stored(Some("j"), 10)), &[v], false);
        assert!(
            p.candidates[0].out_of_band,
            "D79: a denied file changing is audited"
        );
    }
}
