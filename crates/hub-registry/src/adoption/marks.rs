//! The marks of an intentional divergence (T5, D20, D93, S4): "NFS and Git differ on purpose, and that is not a finding".
//!
//! A mark holds the hashes of NFS and of Git (both for drift) at the moment it was made, and it overrides `git_ahead`, `nfs_ahead`,
//! `conflict` and `unknown` only while both still equal them (`engine::drift_state`). The pipeline clears a mark that no longer holds,
//! with its own audit event; these two calls are the person's side: [`mark_intentional`](super::AdoptionService::mark_intentional) and
//! [`clear_intentional`](super::AdoptionService::clear_intentional). They need an active **Operator** (the rule of the design's "mark
//! intentional (where allowed)", plan A6) and each writes **one audit event** in its transaction, with the `DriftChanged` event in the
//! outbox, and then tells the pipeline which path to recompute.
//!
//! Only a file in a state that a mark can override is marked: a file that is in sync has nothing to accept, an untracked file is not in
//! the override list (D20), and a file that is marked already keeps its mark (clear it and mark it again to change the reason).
//! The hashes come from the file's row (`nfs_drift_hash` and `git_hash`, the columns the pipeline derives), read in the transaction.

use domain::{ContentHash, DomainEvent, NfsPath, SwimlaneId, Timestamp, User};
use ports::{AuditAction, AuditEvent, AuditVia, Tx};

use super::resolutions::Reason;
use super::{AdoptionError, Inner, hash_of};
use crate::db::ts;
use crate::error::RegistryError;

/// The states a mark can override (D20).
const MARKABLE: [&str; 4] = ["git_ahead", "nfs_ahead", "conflict", "unknown"];

/// A file's state and the two hashes a mark would hold: N (the hash for drift, `None` for a file that is not on NFS) and G.
const FILE_SQL: &str = "\
SELECT state, CASE WHEN nfs_hash IS NULL THEN NULL ELSE COALESCE(nfs_drift_hash, nfs_hash) END::bytea, git_hash::bytea \
FROM file_state WHERE swimlane_id = $1 AND path = $2";

type FileTuple = (String, Option<Vec<u8>>, Option<Vec<u8>>);

/// The two hashes of a mark.
type MarkHashes = (Option<Vec<u8>>, Option<Vec<u8>>);

impl Inner {
    fn mark_event(
        &self,
        u: &User,
        action: AuditAction,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        via: AuditVia,
    ) -> AuditEvent {
        let mut event = AuditEvent::new(
            Timestamp::from_unix_millis(self.clock.now().unix_millis()),
            Self::actor(u),
            action,
            via,
        );
        event.swimlane = Some(swimlane.clone());
        event.path = Some(path.clone());
        event
    }

    /// The `DriftChanged` event and the audit event of a mark or a clear, in the caller's transaction: the outbox first, the audit event
    /// last (the chain's lock is held until the commit).
    async fn record_mark(
        &self,
        tx: &mut crate::db::Txn,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        audit: AuditEvent,
    ) -> Result<(), RegistryError> {
        let mut handle = Tx::from_pg(tx);
        crate::batch::publish_all(
            &*self.bus,
            &mut handle,
            vec![DomainEvent::DriftChanged {
                swimlane: swimlane.clone(),
                paths: vec![path.clone()],
            }],
        )
        .await?;
        crate::batch::record_all(&*self.audit, &mut handle, vec![audit]).await
    }

    pub(crate) async fn mark_intentional(
        &self,
        u: &User,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        reason: &Reason,
        via: AuditVia,
    ) -> Result<(), AdoptionError> {
        let now = self.clock.now().unix_millis();
        let (tid, oid) = Self::user_ids(u);
        let mut tx = self.db.begin().await?;
        let found: Option<FileTuple> = self
            .db
            .run(
                "mark_file",
                sqlx::query_as(FILE_SQL)
                    .bind(swimlane.as_str())
                    .bind(path.as_str())
                    .fetch_optional(&mut *tx),
            )
            .await?;
        let (state, nfs, git) = found.ok_or(AdoptionError::Read(ports::ReadError::NotFound))?;
        if !MARKABLE.contains(&state.as_str()) {
            return Err(AdoptionError::NotDivergent);
        }
        let nfs: Option<ContentHash> = nfs.map(hash_of).transpose()?;
        let git: Option<ContentHash> = git.map(hash_of).transpose()?;
        let sql = format!(
            "INSERT INTO intentional_divergence (swimlane_id, path, nfs_hash, git_hash, reason, marked_by_tid, marked_by_oid, marked_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, {}) \
             ON CONFLICT (swimlane_id, path) DO UPDATE SET nfs_hash = EXCLUDED.nfs_hash, git_hash = EXCLUDED.git_hash, \
               reason = EXCLUDED.reason, marked_by_tid = EXCLUDED.marked_by_tid, marked_by_oid = EXCLUDED.marked_by_oid, \
               marked_at = EXCLUDED.marked_at",
            ts("$8")
        );
        self.db
            .run(
                "mark_write",
                sqlx::query(&sql)
                    .bind(swimlane.as_str())
                    .bind(path.as_str())
                    .bind(nfs.map(|h| h.as_bytes().to_vec()))
                    .bind(git.map(|h| h.as_bytes().to_vec()))
                    .bind(reason.as_str())
                    .bind(&tid)
                    .bind(&oid)
                    .bind(now)
                    .execute(&mut *tx),
            )
            .await?;
        let mut audit = self.mark_event(u, AuditAction::DriftMarkedIntentional, swimlane, path, via);
        audit.hash_before = nfs;
        audit.hash_after = git;
        audit.diff = Some(format!("intentional divergence: {}", reason.as_str()));
        self.record_mark(&mut tx, swimlane, path, audit).await?;
        self.db.commit(tx).await?;
        if let Some(pipeline) = &self.pipeline {
            pipeline.paths_changed(swimlane.clone(), vec![path.clone()]);
        }
        Ok(())
    }

    pub(crate) async fn clear_intentional(
        &self,
        u: &User,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        via: AuditVia,
    ) -> Result<(), AdoptionError> {
        let mut tx = self.db.begin().await?;
        let removed: Option<MarkHashes> = self
            .db
            .run(
                "mark_clear",
                sqlx::query_as(
                    "DELETE FROM intentional_divergence WHERE swimlane_id = $1 AND path = $2 \
                     RETURNING nfs_hash::bytea, git_hash::bytea",
                )
                .bind(swimlane.as_str())
                .bind(path.as_str())
                .fetch_optional(&mut *tx),
            )
            .await?;
        let (nfs, git) = removed.ok_or(AdoptionError::Read(ports::ReadError::NotFound))?;
        let mut audit = self.mark_event(u, AuditAction::DriftMarkCleared, swimlane, path, via);
        audit.hash_before = nfs.map(hash_of).transpose()?;
        audit.hash_after = git.map(hash_of).transpose()?;
        self.record_mark(&mut tx, swimlane, path, audit).await?;
        self.db.commit(tx).await?;
        if let Some(pipeline) = &self.pipeline {
            pipeline.paths_changed(swimlane.clone(), vec![path.clone()]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_overrides_exactly_the_states_the_engine_says() {
        // `engine::drift_state`: `intentional_divergence` replaces git_ahead, nfs_ahead, conflict and unknown, and nothing else.
        assert_eq!(MARKABLE, ["git_ahead", "nfs_ahead", "conflict", "unknown"]);
        for state in ["in_sync", "untracked", "intentional_divergence"] {
            assert!(!MARKABLE.contains(&state), "{state}");
        }
    }

    #[test]
    fn the_file_statement_is_keyed_and_binds_two_values() {
        assert!(FILE_SQL.contains("$2") && !FILE_SQL.contains("$3"));
        assert!(FILE_SQL.contains("swimlane_id = $1 AND path = $2"));
    }
}
