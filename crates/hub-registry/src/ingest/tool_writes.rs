//! The seam with prompt 06 (A10): a write the tool is about to make is registered, so the scan that later sees it is not an
//! out-of-band change.
//!
//! Prompt 06 calls [`ToolWrites::record`] in its own transaction *before* it sends `WriteFile` to the agent. The call inserts an
//! observation with source `tool_write` and attribution `tool_write` (certain), and publishes `FileObserved`, both in the
//! caller's transaction. Ingestion treats a hash as explained when such a row exists for the same swimlane and path (and is not
//! older than the state it would replace, see `plan::TOOL_WRITE_SLACK_MS`). A write that then fails leaves a harmless row: it
//! only explains a hash that never appeared.
//!
//! The call is idempotent: the observation's key is `(swimlane, path, observed_at, hash)`, so a retry with the same instant is
//! one row and publishes nothing the second time.

use std::sync::Arc;

use domain::{
    Actor, Attribution, AttributionSource, Confidence, ContentHash, DomainEvent, Evidence, NfsPath,
    ObservationSource, SwimlaneId, UserId,
};
use ports::{EventBus, Tx};

use crate::clock::Clock;
use crate::db::ts;
use crate::error::RegistryError;

/// A write the tool is about to make.
#[derive(Debug, Clone)]
pub struct ToolWriteRecord<'a> {
    pub swimlane: &'a SwimlaneId,
    pub path: &'a NfsPath,
    /// The hash the file will have, or `None` for a delete.
    pub hash: Option<ContentHash>,
    /// The size of the new content.
    pub size: Option<u64>,
    /// The user who made the change.
    pub actor: &'a UserId,
}

#[derive(Clone)]
pub struct ToolWrites {
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ToolWrites {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolWrites").finish_non_exhaustive()
    }
}

impl ToolWrites {
    pub fn new(bus: Arc<dyn EventBus>, clock: Arc<dyn Clock>) -> Self {
        Self { bus, clock }
    }

    /// Register a write in the caller's transaction. Returns `true` when the write was new.
    pub async fn record(&self, tx: &mut Tx<'_>, w: ToolWriteRecord<'_>) -> Result<bool, RegistryError> {
        let at = self.clock.now();
        let attribution = Attribution {
            source: AttributionSource::ToolWrite,
            confidence: Confidence::Certain,
            actor: Some(Actor::User {
                user: w.actor.clone(),
            }),
            evidence: Evidence {
                observed_at: Some(at),
                ..Evidence::default()
            },
        };
        let actor = serde_json::to_value(&attribution.actor).map_err(|_| RegistryError::Invalid)?;
        let evidence = serde_json::to_value(&attribution.evidence).map_err(|_| RegistryError::Invalid)?;
        let conn = tx.pg()?;
        let done = sqlx::query(&format!(
            "INSERT INTO file_observations \
               (swimlane_id, path, hash, size, source, observed_at, attribution_source, attribution_confidence, \
                attribution_actor, attribution_evidence) \
             VALUES ($1, $2, $3::bytea, $4, 'tool_write', {at}, 'tool_write', 'certain', $6, $7) \
             ON CONFLICT DO NOTHING",
            at = ts("$5")
        ))
        .bind(w.swimlane.as_str())
        .bind(w.path.as_str())
        .bind(w.hash.map(|h| h.as_bytes().to_vec()))
        .bind(w.size.and_then(|s| i64::try_from(s).ok()))
        .bind(at.unix_millis())
        .bind(actor)
        .bind(evidence)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            tracing::warn!(op = "tool_write", why = db_word(&e), "database call failed");
            RegistryError::Database
        })?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        // A delete has no hash to name; the scan that sees it is what the pipeline reacts to.
        if let Some(hash) = w.hash {
            self.bus
                .publish_in_tx(
                    tx,
                    DomainEvent::FileObserved {
                        swimlane: w.swimlane.clone(),
                        path: w.path.clone(),
                        hash,
                        source: ObservationSource::ToolWrite,
                    },
                )
                .await?;
        }
        Ok(true)
    }
}

fn db_word(e: &sqlx::Error) -> &'static str {
    match e {
        sqlx::Error::Database(_) => "database",
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => "io",
        _ => "other",
    }
}
