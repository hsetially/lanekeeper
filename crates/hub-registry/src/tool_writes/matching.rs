//! Which observation a registration explains: the rules of the port, without a database.
//!
//! Two places decide it, and both follow the same rules (the port's "Which observation a registration explains"):
//!
//! - **On arrival** ([`Book`]): ingestion applies observations of a path, in the order it records them, while some registrations are
//!   visible. Each observation is explained by the oldest registration that matches it, which is then consumed; and the registrations that
//!   it contradicts are dropped. This is what keeps a tool write out of the out-of-band changes.
//! - **Late** ([`late_matches`]): a registration that committed after observations were applied (the lost-wakeup window, the caller's
//!   commit comes after the file was written) explains at most one of them, found by the leased matcher.
//!
//! The rules both hold:
//!
//! - A **hash observation** matches a hash registration of exactly that hash at that path.
//! - A **deletion observation** matches a delete registration only if the state of the path immediately before it (the hash of the closest
//!   earlier observation of the path that has one, in the path's observation order) was the registration's `deleted_hash`. A deletion of
//!   any other version never matches, and a path with no earlier hash has no such state.
//! - An observation is **eligible** only if it is not attributed as a tool write already (and not already `certain`): a registration
//!   upgrades a weaker attribution, never replaces another tool write, never downgrades.
//! - The **oldest** registration (the lowest `id`) consumes an observation; the others stay.
//! - A newer observation **invalidates**: a hash observation of another hash, or a deletion, drops a pending hash registration; a hash
//!   observation drops a pending delete registration (the file came back).

use std::collections::BTreeMap;

use domain::{AttributionSource, Confidence, ContentHash, NfsPath};

use super::{LATE_SKEW_MS, Pending, TTL_MS};

/// What ingestion reads about one path of a chunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PathRegistrations {
    /// The registrations of the path that have not expired, oldest first.
    pub pending: Vec<Pending>,
    /// The state of the path as the observations have it: the hash of the latest observation of the path that has one, in the path's
    /// observation order. Read only for a path that has a delete registration (it is what a deletion is compared with).
    pub last_hash: Option<ContentHash>,
}

/// What one observation did to the registrations of its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen<'a> {
    /// The registration that explains the observation (consumed by it).
    pub explained: Option<&'a Pending>,
    /// The registrations the observation contradicts (dropped by it).
    pub invalidated: Vec<i64>,
}

/// The registrations of one path, as the observations of a chunk are applied to them one after another, in the path's order.
#[derive(Debug)]
pub(crate) struct Book<'a> {
    /// Still pending, oldest first.
    pending: Vec<&'a Pending>,
    last_hash: Option<ContentHash>,
}

impl<'a> Book<'a> {
    pub(crate) fn new(registrations: &'a PathRegistrations) -> Self {
        let mut pending: Vec<&Pending> = registrations.pending.iter().collect();
        pending.sort_by_key(|r| r.id);
        Self {
            pending,
            last_hash: registrations.last_hash,
        }
    }

    /// A hash observation of the path, applied now (so after every registration here became visible).
    pub(crate) fn hash_observed(&mut self, hash: ContentHash) -> Seen<'a> {
        let explained = self
            .pending
            .iter()
            .copied()
            .find(|r| !r.is_delete && r.hash == hash);
        let mut invalidated = Vec::new();
        self.pending.retain(|r| {
            if explained.is_some_and(|e| e.id == r.id) {
                false
            } else if r.is_delete || r.hash != hash {
                // Another hash replaced the state this registration was for, or the file came back after a delete.
                invalidated.push(r.id);
                false
            } else {
                true
            }
        });
        self.last_hash = Some(hash);
        Seen {
            explained,
            invalidated,
        }
    }

    /// A deletion observation of the path, applied now.
    pub(crate) fn deletion_observed(&mut self) -> Seen<'a> {
        let before = self.last_hash;
        let explained =
            before.and_then(|b| self.pending.iter().copied().find(|r| r.is_delete && r.hash == b));
        let mut invalidated = Vec::new();
        self.pending.retain(|r| {
            if explained.is_some_and(|e| e.id == r.id) {
                false
            } else if !r.is_delete {
                // The file is gone: a write the tool made is not what the path holds any more.
                invalidated.push(r.id);
                false
            } else {
                // A delete registration of another version stays: a deletion of the wrong version does not contradict it.
                true
            }
        });
        Seen {
            explained,
            invalidated,
        }
    }
}

/// An observation the matcher looks at, as the database has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observed {
    pub id: i64,
    /// `None` for a deletion.
    pub hash: Option<ContentHash>,
    pub source: AttributionSource,
    pub confidence: Confidence,
    /// When the registry applied it (its own clock), never the agent's `observed_at`.
    pub applied_ms: i64,
    /// When the agent saw it (the agent's clock): evidence on the attribution, never a decision.
    pub agent_seen_ms: i64,
    /// For a deletion: the state of the path immediately before it (see the module docs).
    pub before: Option<ContentHash>,
}

impl Observed {
    /// A registration may upgrade this observation: it is not a tool write, and nothing is above it.
    pub(crate) fn eligible(&self) -> bool {
        self.source != AttributionSource::ToolWrite && self.confidence < Confidence::Certain
    }
}

/// The late matches of one path: `(registration, observation)` as indexes into the two slices. `pending` and `observed` are both in
/// order of `id`, oldest first; each registration explains at most one observation and each observation is explained by at most one
/// registration, the oldest first.
///
/// - A **hash registration** takes the newest eligible observation of exactly its hash.
/// - A **delete registration** takes the newest observation of the path, which must be a deletion whose state before was the registration's
///   deleted hash, and eligible. An older deletion is never taken, however recent: a person's deletion that came before someone recreated
///   the file is not the tool's.
/// - Both need the observation applied within the TTL of `now_ms`, and not earlier than [`LATE_SKEW_MS`] before the registration's stamp
///   (the tool registers before it writes: an observation older than the registration is another change).
pub(crate) fn late_matches(pending: &[&Pending], observed: &[Observed], now_ms: i64) -> Vec<(usize, usize)> {
    let cutoff = now_ms.saturating_sub(TTL_MS);
    let mut taken = vec![false; observed.len()];
    let mut out = Vec::new();
    for (ri, r) in pending.iter().enumerate() {
        if r.registered_at_ms <= cutoff {
            continue;
        }
        let floor = r.registered_at_ms.saturating_sub(LATE_SKEW_MS);
        let usable = |i: usize, o: &Observed| {
            !taken[i] && o.eligible() && o.applied_ms > cutoff && o.applied_ms >= floor
        };
        let found = if r.is_delete {
            observed
                .iter()
                .enumerate()
                .next_back()
                .filter(|(i, o)| o.hash.is_none() && o.before == Some(r.hash) && usable(*i, o))
                .map(|(i, _)| i)
        } else {
            observed
                .iter()
                .enumerate()
                .rev()
                .find(|(i, o)| o.hash == Some(r.hash) && usable(*i, o))
                .map(|(i, _)| i)
        };
        if let Some(oi) = found {
            taken[oi] = true;
            out.push((ri, oi));
        }
    }
    out
}

/// [`late_matches`] for every path: the registrations and the observations of a swimlane, grouped by path.
pub(crate) fn late_matches_of_lane(
    pending: &BTreeMap<NfsPath, Vec<Pending>>,
    observed: &BTreeMap<NfsPath, Vec<Observed>>,
    now_ms: i64,
) -> Vec<(NfsPath, Pending, Observed)> {
    let mut out = Vec::new();
    for (path, regs) in pending {
        let Some(seen) = observed.get(path) else {
            continue;
        };
        let mut regs: Vec<&Pending> = regs.iter().collect();
        regs.sort_by_key(|r| r.id);
        let mut seen: Vec<Observed> = seen.clone();
        seen.sort_by_key(|o| o.id);
        for (ri, oi) in late_matches(&regs, &seen, now_ms) {
            out.push((path.clone(), regs[ri].clone(), seen[oi].clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::{Guid, RequestId, UserId};
    use ports::content_hash;

    use super::*;

    fn h(s: &str) -> ContentHash {
        content_hash(s.as_bytes())
    }

    fn user(n: u32) -> UserId {
        UserId::new(
            Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap(),
            Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
        )
    }

    fn write(id: i64, hash: &str) -> Pending {
        Pending {
            id,
            hash: h(hash),
            is_delete: false,
            user: user(u32::try_from(id).unwrap()),
            request_id: RequestId::parse(&format!("req-{id}")).unwrap(),
            registered_at_ms: 1_000_000,
        }
    }

    fn delete(id: i64, deleted: &str) -> Pending {
        Pending {
            is_delete: true,
            ..write(id, deleted)
        }
    }

    fn regs(pending: Vec<Pending>, last: Option<&str>) -> PathRegistrations {
        PathRegistrations {
            pending,
            last_hash: last.map(h),
        }
    }

    // ------------------------------------------------------------------------------------------------ on arrival

    #[test]
    fn a_hash_observation_is_explained_by_a_registration_of_exactly_that_hash() {
        let r = regs(vec![write(1, "b"), write(2, "c")], None);
        let mut book = Book::new(&r);
        let seen = book.hash_observed(h("b"));
        assert_eq!(seen.explained.map(|p| p.id), Some(1));
        let mut book = Book::new(&r);
        assert_eq!(book.hash_observed(h("zzz")).explained, None);
    }

    #[test]
    fn the_oldest_registration_consumes_the_observation_and_the_others_stay() {
        // The lowest id, whatever order they were given in and whatever their stamps say.
        let mut late_stamp = write(5, "b");
        late_stamp.registered_at_ms = 1;
        let r = regs(vec![write(9, "b"), late_stamp, write(7, "b")], None);
        let mut book = Book::new(&r);
        let first = book.hash_observed(h("b"));
        assert_eq!(first.explained.map(|p| p.id), Some(5), "by id, not by stamp");
        assert!(
            first.invalidated.is_empty(),
            "the others are for their own observation"
        );
        assert_eq!(book.hash_observed(h("b")).explained.map(|p| p.id), Some(7));
        assert_eq!(book.hash_observed(h("b")).explained.map(|p| p.id), Some(9));
        assert_eq!(
            book.hash_observed(h("b")).explained,
            None,
            "each registration explains one observation"
        );
    }

    #[test]
    fn another_hash_invalidates_a_pending_hash_registration_at_once() {
        // a registration for c, then someone else writes b: the later write of c is not the tool's.
        let r = regs(vec![write(1, "c")], None);
        let mut book = Book::new(&r);
        let seen = book.hash_observed(h("b"));
        assert_eq!(seen.explained, None);
        assert_eq!(seen.invalidated, vec![1]);
        assert_eq!(book.hash_observed(h("c")).explained, None, "dropped for good");
    }

    #[test]
    fn a_registration_of_the_same_hash_is_not_invalidated_by_an_observation_of_that_hash() {
        // Two requests registered b; one observation of b consumes the older and leaves the other.
        let r = regs(vec![write(1, "b"), write(2, "b")], None);
        let mut book = Book::new(&r);
        let seen = book.hash_observed(h("b"));
        assert_eq!(seen.explained.map(|p| p.id), Some(1));
        assert!(seen.invalidated.is_empty());
    }

    #[test]
    fn a_deletion_invalidates_a_pending_hash_registration() {
        let r = regs(vec![write(1, "c")], Some("a"));
        let mut book = Book::new(&r);
        let seen = book.deletion_observed();
        assert_eq!(seen.explained, None);
        assert_eq!(seen.invalidated, vec![1]);
    }

    #[test]
    fn a_deletion_matches_only_the_version_it_deleted() {
        let r = regs(vec![delete(1, "a")], Some("a"));
        assert_eq!(Book::new(&r).deletion_observed().explained.map(|p| p.id), Some(1));
        // The state before the deletion was another version: the registration is for a, the deletion is of b.
        let r = regs(vec![delete(1, "a")], Some("b"));
        let mut book = Book::new(&r);
        let seen = book.deletion_observed();
        assert_eq!(
            seen.explained, None,
            "a deletion of any other version never matches"
        );
        assert!(
            seen.invalidated.is_empty(),
            "and it does not contradict a delete of the other version"
        );
        // No earlier observation with a hash: no state before, so no match (safe).
        let r = regs(vec![delete(1, "a")], None);
        assert_eq!(Book::new(&r).deletion_observed().explained, None);
    }

    #[test]
    fn the_oldest_delete_registration_consumes_the_deletion_and_the_other_stays() {
        let r = regs(vec![delete(4, "a"), delete(3, "a")], Some("a"));
        let mut book = Book::new(&r);
        let seen = book.deletion_observed();
        assert_eq!(seen.explained.map(|p| p.id), Some(3));
        assert!(seen.invalidated.is_empty());
        assert_eq!(book.pending.len(), 1, "the other stays pending");
        assert_eq!(book.pending[0].id, 4);
    }

    #[test]
    fn a_hash_observation_invalidates_every_delete_registration() {
        // The file came back, so a later disappearance is not the tool's delete, whatever version the registration named.
        let r = regs(vec![delete(1, "a"), delete(2, "z")], Some("a"));
        let mut book = Book::new(&r);
        let seen = book.hash_observed(h("a"));
        assert_eq!(
            seen.explained, None,
            "a delete registration is not a write registration"
        );
        assert_eq!(seen.invalidated, vec![1, 2]);
    }

    #[test]
    fn a_delete_and_a_write_registration_of_one_hash_never_explain_each_other() {
        let r = regs(vec![delete(1, "b"), write(2, "b")], Some("b"));
        let mut book = Book::new(&r);
        let seen = book.hash_observed(h("b"));
        assert_eq!(
            seen.explained.map(|p| p.id),
            Some(2),
            "the write explains the hash observation"
        );
        assert_eq!(
            seen.invalidated,
            vec![1],
            "and the delete is contradicted by the file coming back"
        );
    }

    // ------------------------------------------------------------------------------------------------ late

    fn seen(id: i64, hash: Option<&str>, applied_ms: i64) -> Observed {
        Observed {
            id,
            hash: hash.map(h),
            source: AttributionSource::Unknown,
            confidence: Confidence::Low,
            applied_ms,
            agent_seen_ms: applied_ms,
            before: None,
        }
    }

    fn deletion(id: i64, before: Option<&str>, applied_ms: i64) -> Observed {
        Observed {
            before: before.map(h),
            ..seen(id, None, applied_ms)
        }
    }

    const NOW: i64 = 1_001_000;

    #[test]
    fn a_hash_registration_takes_the_newest_eligible_observation_of_its_hash() {
        let p = write(1, "b");
        let observed = [
            seen(10, Some("b"), 1_000_100),
            seen(11, Some("c"), 1_000_200),
            seen(12, Some("b"), 1_000_300),
        ];
        assert_eq!(
            late_matches(&[&p], &observed, NOW),
            vec![(0, 2)],
            "the newest b, not the oldest, and not c"
        );
    }

    #[test]
    fn a_registration_attributes_at_most_one_observation() {
        let p = write(1, "b");
        let observed = [seen(10, Some("b"), 1_000_100), seen(11, Some("b"), 1_000_200)];
        assert_eq!(late_matches(&[&p], &observed, NOW), vec![(0, 1)]);
    }

    #[test]
    fn the_oldest_registration_takes_the_observation_and_the_next_takes_the_next_one() {
        let (p1, p2) = (write(1, "b"), write(2, "b"));
        let observed = [seen(10, Some("b"), 1_000_100), seen(11, Some("b"), 1_000_200)];
        assert_eq!(
            late_matches(&[&p1, &p2], &observed, NOW),
            vec![(0, 1), (1, 0)],
            "an observation never has two actors"
        );
        // One observation, two registrations: the oldest wins, the other gets nothing.
        assert_eq!(late_matches(&[&p1, &p2], &observed[1..], NOW), vec![(0, 0)]);
    }

    #[test]
    fn an_observation_that_is_a_tool_write_already_is_not_eligible_and_a_weaker_one_is() {
        let p = write(1, "b");
        for (source, confidence, eligible) in [
            (AttributionSource::Unknown, Confidence::Low, true),
            (AttributionSource::NfsClient, Confidence::Medium, true),
            (AttributionSource::SentinelLogin, Confidence::High, true),
            (AttributionSource::SyncJob, Confidence::High, true),
            (AttributionSource::FsOwnerHint, Confidence::Low, true),
            (AttributionSource::ToolWrite, Confidence::Certain, false),
            // Nothing is above certain: never a lateral move or a downgrade.
            (AttributionSource::Unknown, Confidence::Certain, false),
        ] {
            let mut o = seen(10, Some("b"), 1_000_100);
            o.source = source;
            o.confidence = confidence;
            assert_eq!(
                !late_matches(&[&p], &[o], NOW).is_empty(),
                eligible,
                "{source:?} {confidence:?}"
            );
        }
    }

    #[test]
    fn an_observation_outside_the_ttl_is_not_taken() {
        // A registration that is itself about to expire (so that the skew floor, which is about the registration's stamp, is out of the way).
        let mut p = write(1, "b");
        p.registered_at_ms = NOW - TTL_MS + 1;
        let old = seen(10, Some("b"), NOW - TTL_MS);
        assert!(
            late_matches(&[&p], &[old], NOW).is_empty(),
            "applied exactly one TTL ago: expired"
        );
        let fresh = seen(10, Some("b"), NOW - TTL_MS + 1);
        assert_eq!(late_matches(&[&p], &[fresh], NOW), vec![(0, 0)]);
    }

    #[test]
    fn an_expired_registration_takes_nothing() {
        let mut p = write(1, "b");
        p.registered_at_ms = NOW - TTL_MS;
        assert!(late_matches(&[&p], &[seen(10, Some("b"), NOW - 10)], NOW).is_empty());
    }

    #[test]
    fn an_observation_applied_before_the_registration_was_stamped_is_not_its_write() {
        let p = write(1, "b");
        // Applied a minute before the stamp: the tool had not started yet.
        assert!(late_matches(&[&p], &[seen(10, Some("b"), p.registered_at_ms - 60_000)], NOW).is_empty());
        // Applied inside the skew allowed between two replicas' clocks: taken.
        let skewed = seen(10, Some("b"), p.registered_at_ms - LATE_SKEW_MS);
        assert_eq!(late_matches(&[&p], &[skewed], NOW), vec![(0, 0)]);
        let too_early = seen(10, Some("b"), p.registered_at_ms - LATE_SKEW_MS - 1);
        assert!(late_matches(&[&p], &[too_early], NOW).is_empty());
    }

    #[test]
    fn a_delete_registration_takes_only_the_newest_observation_and_only_a_matching_deletion() {
        let p = delete(1, "a");
        // O0 a person's deletion, O1 someone recreated a, O2 the tool's deletion: only O2.
        let observed = [
            deletion(10, Some("a"), 1_000_100),
            seen(11, Some("a"), 1_000_200),
            deletion(12, Some("a"), 1_000_300),
        ];
        assert_eq!(late_matches(&[&p], &observed, NOW), vec![(0, 2)]);
        // The newest observation is a hash: the deletion is not the newest, so nothing (a hash observation follows it).
        assert!(late_matches(&[&p], &observed[..2], NOW).is_empty());
        let after = [deletion(10, Some("a"), 1_000_100), seen(11, Some("a"), 1_000_200)];
        assert!(
            late_matches(&[&p], &after, NOW).is_empty(),
            "no hash observation of the path may follow the deletion"
        );
        // The newest deletion deleted another version: never.
        let other = [deletion(12, Some("b"), 1_000_300)];
        assert!(late_matches(&[&p], &other, NOW).is_empty());
        // A deletion with no earlier observation that has a hash.
        let none = [deletion(12, None, 1_000_300)];
        assert!(late_matches(&[&p], &none, NOW).is_empty());
    }

    #[test]
    fn a_delete_registration_does_not_take_a_hash_observation_nor_a_hash_registration_a_deletion() {
        let d = delete(1, "a");
        let w = write(2, "a");
        assert!(late_matches(&[&d], &[seen(10, Some("a"), 1_000_100)], NOW).is_empty());
        assert!(late_matches(&[&w], &[deletion(10, Some("a"), 1_000_100)], NOW).is_empty());
    }

    #[test]
    fn two_registrations_do_not_take_one_deletion() {
        let (d1, d2) = (delete(1, "a"), delete(2, "a"));
        let observed = [deletion(10, Some("a"), 1_000_100)];
        assert_eq!(late_matches(&[&d1, &d2], &observed, NOW), vec![(0, 0)]);
    }

    #[test]
    fn the_lane_groups_by_path_and_keeps_paths_apart() {
        let mut pending = BTreeMap::new();
        pending.insert(NfsPath::parse("svc/a.yml").unwrap(), vec![write(1, "b")]);
        pending.insert(NfsPath::parse("svc/c.yml").unwrap(), vec![write(2, "b")]);
        let mut observed = BTreeMap::new();
        observed.insert(
            NfsPath::parse("svc/a.yml").unwrap(),
            vec![seen(10, Some("b"), 1_000_100)],
        );
        // The same hash at another path matches the registration of that path only.
        observed.insert(
            NfsPath::parse("svc/d.yml").unwrap(),
            vec![seen(11, Some("b"), 1_000_100)],
        );
        let found = late_matches_of_lane(&pending, &observed, NOW);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.as_str(), "svc/a.yml");
        assert_eq!((found[0].1.id, found[0].2.id), (1, 10));
    }
}
