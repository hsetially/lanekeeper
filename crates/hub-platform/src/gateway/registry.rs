//! Which replica holds each agent's stream (D62): the `agent_connections` table.
//!
//! One row per swimlane, written by the replica that holds the stream, and never deleted:
//!
//! - **Connect** upserts the row and bumps `epoch`, so the epoch only ever grows, whichever replica the agent reaches.
//! - **Disconnect** empties `replica_address` where the epoch still matches. A replica that learns late that its stream ended
//!   (the agent has since connected elsewhere) matches nothing and cannot clear the newer registration. A `DELETE` would let
//!   the next connect start again at epoch 1 while a stale replica still holds epoch 1.
//! - **Liveness**: the owner renews `last_seen_at` every few seconds. A row with an empty address, or one that was not renewed
//!   for `stale_after` (by the database's clock, not a replica's), counts as not connected: the owner died without clearing it.
//!
//! Every call has a timeout (rule 5), and nothing here holds a transaction across anything else.

use std::collections::HashSet;
use std::time::Duration;

use domain::SwimlaneId;
use sqlx::PgPool;

use super::forward::PeerAddress;
use super::local::Epoch;
use crate::ca::profile::PeerName;

/// One database call.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The database did not answer, or refused (a swimlane that does not exist is refused by the foreign key).
    #[error("the connection registry is unavailable")]
    Unavailable,
    /// The database holds a value that cannot be an epoch.
    #[error("the connection registry holds a value that is not valid")]
    Corrupt,
}

/// Where a swimlane's live stream is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The replica that holds the stream.
    pub replica: String,
    /// Its internal address (`host:port`), as it wrote it.
    pub address: String,
    pub epoch: Epoch,
}

/// What the registry says about a swimlane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// A replica holds a stream and has renewed its claim recently.
    Live(Route),
    /// There is a row, but nobody holds the stream now: it was cleared, or its owner stopped renewing it.
    Stale,
    /// No agent has ever registered.
    Absent,
}

/// This replica's view of `agent_connections`.
#[derive(Debug, Clone)]
pub struct ConnectionRegistry {
    pool: PgPool,
    replica: PeerName,
    address: PeerAddress,
    stale_after: Duration,
}

async fn within<T>(
    call: impl std::future::Future<Output = Result<T, sqlx::Error>>,
) -> Result<T, RegistryError> {
    match tokio::time::timeout(QUERY_TIMEOUT, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) | Err(_) => Err(RegistryError::Unavailable),
    }
}

impl ConnectionRegistry {
    #[must_use]
    pub fn new(pool: PgPool, replica: PeerName, address: PeerAddress, stale_after: Duration) -> Self {
        Self {
            pool,
            replica,
            address,
            stale_after,
        }
    }

    /// This replica's id.
    #[must_use]
    pub fn replica(&self) -> &PeerName {
        &self.replica
    }

    /// Take the swimlane's stream for this replica and return the new epoch.
    pub async fn register(&self, swimlane: &SwimlaneId) -> Result<Epoch, RegistryError> {
        let epoch: i64 = within(
            sqlx::query_scalar(
                "INSERT INTO agent_connections (swimlane_id, replica_id, replica_address, epoch) \
                 VALUES ($1, $2, $3, 1) \
                 ON CONFLICT (swimlane_id) DO UPDATE SET \
                   epoch = agent_connections.epoch + 1, \
                   replica_id = excluded.replica_id, \
                   replica_address = excluded.replica_address, \
                   connected_at = now(), \
                   last_seen_at = now() \
                 RETURNING epoch",
            )
            .bind(swimlane.as_str())
            .bind(self.replica.as_str())
            .bind(self.address.as_str())
            .fetch_one(&self.pool),
        )
        .await?;
        if epoch < 1 {
            return Err(RegistryError::Corrupt);
        }
        Ok(Epoch::new(epoch))
    }

    /// Release the registration made with `epoch`, if it is still the current one and still this replica's. False when it
    /// has been replaced or already released: nothing is changed then.
    pub async fn clear(&self, swimlane: &SwimlaneId, epoch: Epoch) -> Result<bool, RegistryError> {
        let done = within(
            sqlx::query(
                "UPDATE agent_connections SET replica_address = '' \
                 WHERE swimlane_id = $1 AND epoch = $2 AND replica_id = $3 AND replica_address <> ''",
            )
            .bind(swimlane.as_str())
            .bind(epoch.get())
            .bind(self.replica.as_str())
            .execute(&self.pool),
        )
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Renew the claim on every `(swimlane, epoch)` this replica holds, in one statement. Returns the swimlanes whose claim
    /// was renewed; one that is missing from the result has been replaced (or cleared) and its stream is stale.
    pub async fn refresh(&self, held: &[(SwimlaneId, Epoch)]) -> Result<HashSet<SwimlaneId>, RegistryError> {
        if held.is_empty() {
            return Ok(HashSet::new());
        }
        let swimlanes: Vec<&str> = held.iter().map(|(s, _)| s.as_str()).collect();
        let epochs: Vec<i64> = held.iter().map(|(_, e)| e.get()).collect();
        let renewed: Vec<String> = within(
            sqlx::query_scalar(
                "UPDATE agent_connections c SET last_seen_at = now() \
                 FROM unnest($1::text[], $2::bigint[]) AS held(swimlane_id, epoch) \
                 WHERE c.swimlane_id = held.swimlane_id AND c.epoch = held.epoch \
                   AND c.replica_id = $3 AND c.replica_address <> '' \
                 RETURNING c.swimlane_id",
            )
            .bind(&swimlanes)
            .bind(&epochs)
            .bind(self.replica.as_str())
            .fetch_all(&self.pool),
        )
        .await?;
        Ok(renewed
            .into_iter()
            .filter_map(|id| SwimlaneId::parse(&id).ok())
            .collect())
    }

    /// What the registry says about `swimlane` now.
    pub async fn lookup(&self, swimlane: &SwimlaneId) -> Result<Lookup, RegistryError> {
        let row: Option<(String, String, i64, bool)> = within(
            sqlx::query_as(
                "SELECT replica_id, replica_address, epoch, \
                        (replica_address <> '' AND last_seen_at > now() - make_interval(secs => $2::double precision)) AS live \
                 FROM agent_connections WHERE swimlane_id = $1",
            )
            .bind(swimlane.as_str())
            .bind(self.stale_after.as_secs_f64())
            .fetch_optional(&self.pool),
        )
        .await?;
        match row {
            None => Ok(Lookup::Absent),
            Some((_, _, _, false)) => Ok(Lookup::Stale),
            Some((replica, address, epoch, true)) => {
                if epoch < 1 {
                    return Err(RegistryError::Corrupt);
                }
                Ok(Lookup::Live(Route {
                    replica,
                    address,
                    epoch: Epoch::new(epoch),
                }))
            }
        }
    }

    /// Whether the swimlane's agent has ever joined (`agents.joined_at`), which tells `Disconnected` from `NeverSeen`.
    pub async fn has_joined(&self, swimlane: &SwimlaneId) -> Result<bool, RegistryError> {
        let joined: Option<bool> = within(
            sqlx::query_scalar("SELECT joined_at IS NOT NULL FROM agents WHERE swimlane_id = $1")
                .bind(swimlane.as_str())
                .fetch_optional(&self.pool),
        )
        .await?;
        Ok(joined.unwrap_or(false))
    }
}
