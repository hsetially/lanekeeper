//! The hot statements of the write path, one call each, for the benchmark of P12 (`benches/write_queries.rs`) and nothing else.
//! Hidden from the documentation: it is not an API.

use domain::{SwimlaneId, UserId};

use crate::idempotency::{ClaimKey, RequestHash};
use crate::pipeline::lock;
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
            Ok(crate::idempotency::Claim::Fresh)
        )
    }

    /// The read of a retry: the row of the key, whatever it holds.
    pub async fn replay(&self, user: &UserId, name: &str, n: u64) -> bool {
        let hash = RequestHash::probe(n);
        matches!(self.hub.idem.peek(&key(user, name), &hash).await, Ok(Some(_)))
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

    /// One page of the expiry sweep.
    pub async fn purge_page(&self) -> bool {
        self.hub
            .idem
            .purge(
                self.hub.now_ms(),
                i64::try_from(self.hub.cfg.reconcile_batch).unwrap_or(100),
            )
            .await
            .is_ok()
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
}
