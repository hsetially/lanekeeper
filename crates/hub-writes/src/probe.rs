//! The hot statements of the write path, one call each, for the benchmark of P12 (`benches/write_queries.rs`) and nothing else.
//! Hidden from the documentation: it is not an API.

use domain::{SwimlaneId, UserId};

use crate::idempotency::{ClaimKey, RequestHash};
use crate::pipeline::lock;
use crate::prs::{job, links};
use crate::service::{Inner, WriteHub, millis};

/// Runs the statements of the write path against the hub's own pool, as the pipeline does.
#[doc(hidden)]
pub struct QueryProbe<'a> {
    hub: &'a Inner,
}

impl std::fmt::Debug for QueryProbe<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryProbe").finish_non_exhaustive()
    }
}

impl WriteHub {
    #[doc(hidden)]
    pub fn probe(&self) -> QueryProbe<'_> {
        QueryProbe { hub: &self.inner }
    }
}

fn key(user: &UserId, key: &str) -> ClaimKey {
    ClaimKey::probe(user, key)
}

impl QueryProbe<'_> {
    /// A fresh claim (the insert, as the first statement of every write). `n` makes the request hash.
    pub async fn claim(&self, user: &UserId, name: &str, n: u64) -> bool {
        let hash = RequestHash::probe(n);
        let ttl = millis(self.hub.cfg.idempotency_ttl);
        matches!(
            self.hub
                .idem
                .claim(&key(user, name), &hash, self.hub.now_ms(), ttl)
                .await,
            Ok(crate::idempotency::Claim::Fresh { .. })
        )
    }

    /// The read of a retry of a finished claim: the row of the key holds the response of this very request (the hash of `n`), and
    /// the read finds it as a replay. A row of another request, one still running or a missing one is `false`.
    pub async fn replay(&self, user: &UserId, name: &str, n: u64) -> bool {
        let hash = RequestHash::probe(n);
        matches!(
            self.hub.idem.peek(&key(user, name), &hash).await,
            Ok(Some(crate::idempotency::Claim::Replay(_)))
        )
    }

    /// The read of a retry whose first request is still running: the row of the key is the claim of this very request (the hash
    /// of `n`) and has no response yet.
    pub async fn in_flight(&self, user: &UserId, name: &str, n: u64) -> bool {
        let hash = RequestHash::probe(n);
        matches!(
            self.hub.idem.peek(&key(user, name), &hash).await,
            Ok(Some(crate::idempotency::Claim::InFlight { .. }))
        )
    }

    /// The request hash a seeded row must hold for [`Self::replay`] with `n` to find the same request.
    pub fn seeded_hash(n: u64) -> Vec<u8> {
        RequestHash::probe(n).as_bytes().to_vec()
    }

    /// The response a seeded finished row holds, as `finish` stores it, and the status stored with it.
    pub fn seeded_response() -> (i32, Vec<u8>) {
        let outcome = domain::WriteOutcome::Conflict { current: None };
        (
            crate::idempotency::status_of(&outcome),
            serde_json::to_vec(&outcome).unwrap_or_default(),
        )
    }

    /// Finish a claim with a stored response.
    pub async fn finish(&self, user: &UserId, name: &str) -> bool {
        let outcome = domain::WriteOutcome::Conflict { current: None };
        self.hub.idem.finish(&key(user, name), &outcome).await.is_ok()
    }

    /// The editing-lock read.
    pub async fn lock(&self, swimlane: &SwimlaneId) -> bool {
        matches!(lock::check(self.hub, swimlane).await, Ok(lock::LockState::Open))
    }

    /// The reconcile job's scan for unfinished claims.
    pub async fn stale_scan(&self) -> usize {
        self.hub
            .idem
            .stale(
                self.hub.now_ms(),
                millis(self.hub.cfg.stale_claim_after),
                millis(self.hub.cfg.idempotency_ttl),
                i64::try_from(self.hub.cfg.reconcile_batch).unwrap_or(100),
            )
            .await
            .map_or(usize::MAX, |rows| rows.len())
    }

    /// One page of the expiry sweep. How many rows it deleted, or `None` if it failed.
    pub async fn purge_page(&self) -> Option<u64> {
        self.hub
            .idem
            .purge(
                self.hub.now_ms(),
                i64::try_from(self.hub.cfg.reconcile_batch).unwrap_or(100),
            )
            .await
            .ok()
    }

    /// One page of the list of proposals (25 rows, newest first), without the diffs: the statement of the list route.
    pub async fn proposal_page(&self) -> usize {
        let Ok(mut conn) = self.hub.pool.acquire().await else {
            return usize::MAX;
        };
        let filter = crate::proposals::store::Filter {
            below: None,
            status: Some(domain::ProposalStatus::Pending),
            swimlane: None,
        };
        crate::proposals::store::list(&mut conn, &filter, 25)
            .await
            .map_or(usize::MAX, |rows| rows.len())
    }

    /// The expiry job's scan for pending proposals whose time has run out.
    pub async fn proposal_expiry_scan(&self) -> usize {
        let Ok(mut conn) = self.hub.pool.acquire().await else {
            return usize::MAX;
        };
        crate::proposals::store::expired(
            &mut conn,
            self.hub.now_ms(),
            i64::try_from(self.hub.cfg.expire_batch).unwrap_or(100),
            &[],
        )
        .await
        .map_or(usize::MAX, |ids| ids.len())
    }

    /// The read of one proposal with its decision, as an approval starts.
    pub async fn proposal_get(&self, id: i64) -> bool {
        let Ok(mut conn) = self.hub.pool.acquire().await else {
            return false;
        };
        crate::proposals::store::get(&mut conn, domain::ProposalId::from(id))
            .await
            .is_ok()
    }

    /// The open-pull-request lookup of a request for a pull request (D77): two (path, hash) pairs of one swimlane, as the
    /// benchmark seeds them (`g` names a seeded link; the second pair is one that is not linked).
    pub async fn open_pr_lookup(&self, g: u64) -> bool {
        let Ok(mut conn) = self.hub.pool.acquire().await else {
            return false;
        };
        let Ok(swimlane) = SwimlaneId::parse(&format!("sit{}", g % 40)) else {
            return false;
        };
        let pairs = vec![
            (
                format!("a/{g}.yml"),
                ports::content_hash(g.to_string().as_bytes()),
            ),
            (format!("a/{}.yml", g + 1), ports::content_hash(b"not linked")),
        ];
        links::find_open(&mut conn, &swimlane, &pairs).await.is_ok()
    }

    /// The scan of the pull-request jobs that need a runner: the jobs nobody has touched for 30 s, at most 50.
    pub async fn pr_job_resume_scan(&self) -> usize {
        let Ok(mut conn) = self.hub.pool.acquire().await else {
            return usize::MAX;
        };
        job::needing_a_runner(&mut conn, self.hub.now_ms().saturating_sub(30_000), 50)
            .await
            .map_or(usize::MAX, |ids| ids.len())
    }
}
