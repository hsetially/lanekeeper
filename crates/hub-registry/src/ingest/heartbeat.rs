//! `heartbeat`: the agent's Merkle root against the hub's (D63, P3).

use domain::{ContentHash, Heartbeat, HeartbeatAction};
use ports::AgentIdentity;

use super::Inner;
use crate::db::ts;
use crate::error::RegistryError;

impl Inner {
    /// The hub keeps the root of the last delta it applied. Unknown asks for a full scan; equal asks for nothing; different asks
    /// for the delta since the hub's root. A scan that the hub still needs (a lost stretch of the spool, a delta that did not start
    /// at the hub's root, an apply that did not finish) is asked for whatever the roots say (A13).
    pub(crate) async fn heartbeat(
        &self,
        id: &AgentIdentity,
        hb: Heartbeat,
    ) -> Result<HeartbeatAction, RegistryError> {
        let swimlane = id.swimlane();
        if !self.said_hello(swimlane) {
            return Err(RegistryError::HelloRequired);
        }
        let now_ms = self.clock.now().unix_millis();
        // One statement: note the heartbeat, and read the hub's root and whether a scan is needed.
        let row: Option<(Option<Vec<u8>>, bool)> = self
            .db
            .run(
                "heartbeat",
                sqlx::query_as(&format!(
                    "WITH seen AS ( \
                       UPDATE agents SET last_heartbeat_at = {now}, scan_seq = $2, file_count = $3 WHERE swimlane_id = $1 \
                     ) \
                     SELECT m.hash::bytea, r.needs_full_scan \
                     FROM swimlane_registry r \
                     LEFT JOIN swimlane_merkle m ON m.swimlane_id = r.swimlane_id AND m.dir_path = '' \
                     WHERE r.swimlane_id = $1",
                    now = ts("$4")
                ))
                .bind(swimlane.as_str())
                .bind(i64::try_from(hb.scan_seq).unwrap_or(i64::MAX))
                .bind(i64::try_from(hb.file_count).unwrap_or(i64::MAX))
                .bind(now_ms)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        // No row: the registry row is made by `hello`, so a swimlane that said hello has one unless an admin removed it.
        let Some((known, needs_full_scan)) = row else {
            return Err(RegistryError::NotFound);
        };
        let known = known
            .map(|b| <[u8; 32]>::try_from(b).map(ContentHash::from_bytes))
            .transpose()
            .map_err(|_| RegistryError::Corrupt)?;
        Ok(match known {
            None => HeartbeatAction::RequestFullScan,
            _ if needs_full_scan => HeartbeatAction::RequestFullScan,
            Some(k) if k == hb.merkle_root => HeartbeatAction::None,
            Some(k) => HeartbeatAction::RequestDelta { since_root: k },
        })
    }
}
