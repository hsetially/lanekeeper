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
use crate::sync::overwrite::{Previous, previous_of};

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
    /// The drift state the pipeline last computed for the path (`None`: it has not computed one, or the column holds a word this code
    /// does not know), and the hash for comparing that it computed it from (N, `nfs_drift_hash`). What a sync job's change replaces
    /// (the C7 context, [`crate::sync::overwrite`]).
    pub state: Option<domain::DriftState>,
    pub drift_hash: Option<ContentHash>,
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
    /// What a sync job's version replaced, for C7 ([`crate::sync::overwrite::previous_of`]). Stored on the observation.
    pub previous: Option<Previous>,
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
    // The flag the version before the next one has. A version that moves the state moves it too.
    let mut cur_denied = stored.is_some_and(|s| s.denied);
    let mut cur_at = stored.map_or(-1, |s| s.observed_at_ms);
    let mut plan = PathPlan::default();

    for version in files.into_iter().chain(removals) {
        match version {
            Version::File { entry, during_job } => {
                // The same content is the same version, **unless the flag changed** (D79, D104). The agent names a change of its deny list as
                // a change of the file, with the hash it had; a hub that took the hash for the whole answer would keep serving a file that
                // its agent has since said is denied (and refuse one that no longer is) until its content happened to change.
                if cur_hash == Some(entry.hash) && cur_denied == entry.denied {
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
                let previous = moves_state
                    .then(|| previous_of(stored, cur_hash, &attribution))
                    .flatten();
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
                    previous,
                });
                if moves_state {
                    cur_hash = Some(entry.hash);
                    cur_denied = entry.denied;
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
                    // A sync does not delete (Q3): a removal replaces no content that a sync could have overwritten.
                    previous: None,
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
            state: None,
            drift_hash: None,
        }
    }

    /// A stored path that the pipeline has computed: `state`, from the hash for comparing `drift`.
    fn computed(hash: &str, drift: &str, at: i64, state: domain::DriftState) -> Stored {
        Stored {
            state: Some(state),
            drift_hash: Some(h(drift)),
            ..stored(Some(hash), at)
        }
    }

    fn by_job(version: &str, at: i64) -> Version {
        let mut v = file(version, at);
        if let Version::File { during_job, .. } = &mut v {
            *during_job = Some(JobRef::new("sync-sit1", "uid-1").unwrap());
        }
        v
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

    // ------------------------------------------------------------------------------------------ what a sync replaced (C7)

    #[test]
    fn a_sync_change_to_a_file_that_was_ahead_on_nfs_carries_what_it_replaced() {
        use domain::DriftState::{Conflict, NfsAhead};
        for state in [NfsAhead, Conflict] {
            let p = plan(
                Some(&computed("edited", "edited-for-drift", 10, state)),
                &[by_job("git", 20)],
                false,
            );
            let c = &p.candidates[0];
            assert_eq!(c.attribution.source, AttributionSource::SyncJob);
            assert_eq!(
                c.previous,
                Some(Previous {
                    state,
                    hash: Some(h("edited-for-drift")),
                }),
                "the state the pipeline had, and N: the hash for comparing, not the raw hash"
            );
        }
    }

    #[test]
    fn the_context_is_kept_for_every_state_the_file_was_in() {
        // The engine decides which states make C7; the observation records what was there.
        let p = plan(
            Some(&computed("a", "a", 10, domain::DriftState::InSync)),
            &[by_job("b", 20)],
            false,
        );
        assert_eq!(
            p.candidates[0].previous,
            Some(Previous {
                state: domain::DriftState::InSync,
                hash: Some(h("a")),
            })
        );
    }

    #[test]
    fn only_a_sync_job_s_change_carries_it() {
        // The same file in the same state, changed out of band: nothing was overwritten by a sync.
        let p = plan(
            Some(&computed("a", "a", 10, domain::DriftState::NfsAhead)),
            &[file("b", 20)],
            false,
        );
        assert_eq!(p.candidates[0].previous, None);
    }

    #[test]
    fn a_path_the_pipeline_has_not_computed_has_no_context() {
        // The row is the placeholder ingestion wrote (`unknown`, no state read back): there is nothing to say it replaced.
        let p = plan(Some(&stored(Some("a"), 10)), &[by_job("b", 20)], false);
        assert_eq!(p.candidates[0].previous, None);
    }

    #[test]
    fn a_new_file_or_one_that_comes_back_replaces_no_content() {
        assert_eq!(plan(None, &[by_job("a", 20)], false).candidates[0].previous, None);
        let gone = Stored {
            state: Some(domain::DriftState::NfsAhead),
            drift_hash: None,
            ..stored(None, 10)
        };
        assert_eq!(
            plan(Some(&gone), &[by_job("a", 20)], false).candidates[0].previous,
            None,
            "a file that was not there"
        );
    }

    #[test]
    fn only_the_version_that_replaces_the_stored_one_carries_it() {
        // a (stored, ahead) -> b by the job -> c by the job in one delta: b replaced what the hub knows, c replaced b, which the hub
        // never computed a state for.
        let p = plan(
            Some(&computed("a", "a", 5, domain::DriftState::NfsAhead)),
            &[by_job("b", 20), by_job("c", 30)],
            false,
        );
        assert_eq!(p.candidates.len(), 2);
        assert!(p.candidates[0].previous.is_some());
        assert_eq!(p.candidates[1].previous, None);
    }

    #[test]
    fn a_version_that_is_only_history_carries_none() {
        let p = plan(
            Some(&computed("b", "b", 100, domain::DriftState::NfsAhead)),
            &[by_job("a", 50)],
            false,
        );
        assert!(!p.candidates[0].moves_state);
        assert_eq!(p.candidates[0].previous, None);
    }

    /// A version of `hash` at `at` with the denied flag as given.
    fn flagged(hash: &str, at: i64, denied: bool) -> Version {
        let mut v = file(hash, at);
        if let Version::File { entry, .. } = &mut v {
            entry.denied = denied;
            if denied {
                entry.class = FileClass::Denied;
                entry.has_content = false;
            }
        }
        v
    }

    #[test]
    fn the_same_content_with_another_denied_flag_is_a_change_of_the_file() {
        // The agent's tree names a change of the flag as a change (its deny list changed), with the content as it was. The hub must act
        // on it: a file whose flag it ignores stays readable for as long as its content does not change (D79, D104).
        let ordinary = stored(Some("a"), 10);
        let p = plan(Some(&ordinary), &[flagged("a", 20, true)], false);
        assert_eq!(p.unchanged, 0, "not a version equal to the stored one");
        let c = &p.candidates[0];
        assert!(c.moves_state, "it moves the state: the row's class and flag");
        assert_eq!(c.hash, Some(h("a")));
        assert_eq!(c.before, Some(h("a")), "the same content");
        assert!(
            c.out_of_band && !c.initial(),
            "audited like any change of a denied file"
        );

        // The other way: the flag cleared, with the bytes.
        let denied = Stored {
            denied: true,
            class: FileClass::Denied,
            ..stored(Some("a"), 10)
        };
        let p = plan(Some(&denied), &[flagged("a", 20, false)], false);
        assert_eq!(p.candidates.len(), 1);
        assert_eq!(p.unchanged, 0);

        // The same flag with the same content is still nothing: a replay, or the next scan.
        assert!(
            plan(Some(&denied), &[flagged("a", 20, true)], false)
                .candidates
                .is_empty()
        );
        assert!(
            plan(Some(&ordinary), &[flagged("a", 20, false)], false)
                .candidates
                .is_empty()
        );
    }

    #[test]
    fn within_one_delta_the_flag_is_compared_with_the_version_before_it() {
        // a (ordinary), then a (denied), then a (denied) again: the flag changes once.
        let p = plan(
            Some(&stored(Some("a"), 5)),
            &[flagged("a", 10, true), flagged("a", 20, true)],
            false,
        );
        assert_eq!(p.candidates.len(), 1);
        assert_eq!(p.unchanged, 1);
        // a denied, then a ordinary again, then a denied: three changes of the flag.
        let p = plan(
            Some(&stored(Some("a"), 5)),
            &[
                flagged("a", 10, true),
                flagged("a", 20, false),
                flagged("a", 30, true),
            ],
            false,
        );
        assert_eq!(p.candidates.len(), 3);
        assert_eq!(p.candidates[2].before, Some(h("a")));
    }

    #[test]
    fn a_path_nobody_has_observed_is_not_a_stored_flag() {
        // A row that Git alone made (no observation) has no flag of NFS's: the first sight is a first sight.
        let mut git_only = stored(Some("a"), 0);
        git_only.observed = false;
        git_only.denied = true;
        let p = plan(Some(&git_only), &[flagged("a", 20, false)], false);
        assert_eq!(p.candidates.len(), 1);
        assert!(p.candidates[0].first_sight());
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
