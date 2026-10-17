//! The deployment record (T4, D21): what the last sync deployed, per swimlane and tenant.
//!
//! One row for each `(swimlane, tenant)` in `swimlane_deployments`, replaced by every report: the tenant branch's commit, the base
//! repository's commit, where the report came from and who made it. The history is the audit chain: each report writes one
//! `SettingsChanged` event (A12, D93) in the transaction of the row. The event's text holds ids only (S10).

use domain::Timestamp;
use ports::{AuditAction, AuditEvent, AuditVia, Tx};

use super::{Inner, Report};
use crate::db::ts;
use crate::error::RegistryError;

impl Inner {
    /// Replace the deployment record of the report's tenant, and audit it.
    pub(crate) async fn record_deployment(&self, r: &Report) -> Result<(), RegistryError> {
        let now_ms = self.clock.now().unix_millis();
        let actor = serde_json::to_string(&r.actor).map_err(|_| RegistryError::Invalid)?;
        let mut tx = self.db.begin().await?;
        self.db
            .run(
                "sync_deployment",
                sqlx::query(&format!(
                    "INSERT INTO swimlane_deployments (swimlane_id, tenant_id, tenant_commit, base_commit, source, actor, recorded_at) \
                     VALUES ($1, $2, $3, $4, $5, $6::jsonb, {now}) \
                     ON CONFLICT (swimlane_id, tenant_id) DO UPDATE SET \
                       tenant_commit = EXCLUDED.tenant_commit, base_commit = EXCLUDED.base_commit, source = EXCLUDED.source, \
                       actor = EXCLUDED.actor, recorded_at = EXCLUDED.recorded_at",
                    now = ts("$7")
                ))
                .bind(r.swimlane.as_str())
                .bind(r.tenant.as_str())
                .bind(r.tenant_commit.as_str())
                .bind(r.base_commit.as_str())
                .bind(r.source.as_str())
                .bind(&actor)
                .bind(now_ms)
                .execute(&mut *tx),
            )
            .await?;
        let mut event = AuditEvent::new(
            Timestamp::from_unix_millis(now_ms),
            r.audit_actor()?,
            AuditAction::SettingsChanged,
            AuditVia::Sync,
        );
        event.swimlane = Some(r.swimlane.clone());
        event.diff = Some(format!(
            "deployment of tenant {}: tenant commit {}, base commit {}, reported by {}",
            r.tenant,
            r.tenant_commit,
            r.base_commit,
            r.source.as_str()
        ));
        let mut handle = Tx::from_pg(&mut tx);
        crate::batch::record_all(&*self.audit, &mut handle, vec![event]).await?;
        self.db.commit(tx).await
    }
}
