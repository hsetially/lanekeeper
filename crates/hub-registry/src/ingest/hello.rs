//! `hello`: the first message of a stream.

use domain::{AgentConfig, Hello, ShortText, SwimlaneId, TenantId};
use ports::AgentIdentity;

use super::Inner;
use super::staging;
use crate::db::ts;
use crate::error::RegistryError;

impl Inner {
    /// Check the identity, make room for the stream, store what the agent says about itself, forget what an earlier stream left
    /// half done, and answer with the agent's configuration.
    pub(crate) async fn hello(&self, id: &AgentIdentity, h: Hello) -> Result<AgentConfig, RegistryError> {
        // S5: an identity can only act for its own swimlane, whatever the payload says.
        if &h.swimlane != id.swimlane() {
            return Err(RegistryError::IdentityMismatch);
        }
        let swimlane = id.swimlane();
        // Room first, so that a refused hello writes nothing; and the place goes back if the hello then fails (an unknown
        // swimlane must not use one up).
        let new_place = self.admit_stream(swimlane)?;
        let config = self.join(swimlane, h).await;
        if config.is_err() && new_place {
            self.forget_stream(swimlane);
        }
        config
    }

    async fn join(&self, swimlane: &SwimlaneId, h: Hello) -> Result<AgentConfig, RegistryError> {
        let now_ms = self.clock.now().unix_millis();
        let mut tx = self.db.begin().await?;

        // An admin must have created the swimlane (A15). The agent fills in what the admin left empty and changes nothing else:
        // the mount facts are the admin's configuration, an agent only confirms them.
        let known = self
            .db
            .run(
                "hello_swimlane",
                sqlx::query(
                    "UPDATE swimlanes SET cluster = COALESCE(cluster, $2), nfs_server = COALESCE(nfs_server, $3), \
                       nfs_export = COALESCE(nfs_export, $4), mount_root = COALESCE(mount_root, $5) \
                     WHERE id = $1",
                )
                .bind(swimlane.as_str())
                .bind(h.cluster.as_str())
                .bind(h.nfs_server.as_str())
                .bind(h.export.as_str())
                .bind(h.mount_root.as_str())
                .execute(&mut *tx),
            )
            .await?;
        if known.rows_affected() == 0 {
            return Err(RegistryError::NotFound);
        }
        self.db
            .run(
                "hello_agent",
                sqlx::query(&format!(
                    "INSERT INTO agents (swimlane_id, agent_version, joined_at, last_hello_at) VALUES ($1, $2, {now}, {now}) \
                     ON CONFLICT (swimlane_id) DO UPDATE SET agent_version = EXCLUDED.agent_version, \
                       last_hello_at = EXCLUDED.last_hello_at, joined_at = COALESCE(agents.joined_at, EXCLUDED.joined_at)",
                    now = ts("$3")
                ))
                .bind(swimlane.as_str())
                .bind(h.agent_version.as_str())
                .bind(now_ms)
                .execute(&mut *tx),
            )
            .await?;
        self.db
            .run(
                "hello_rows",
                sqlx::query("INSERT INTO agent_acks (swimlane_id) VALUES ($1) ON CONFLICT DO NOTHING")
                    .bind(swimlane.as_str())
                    .execute(&mut *tx),
            )
            .await?;
        self.db
            .run(
                "hello_rows",
                sqlx::query("INSERT INTO swimlane_registry (swimlane_id) VALUES ($1) ON CONFLICT DO NOTHING")
                    .bind(swimlane.as_str())
                    .execute(&mut *tx),
            )
            .await?;
        let tenants: Vec<String> = self
            .db
            .run(
                "hello_tenants",
                sqlx::query_scalar(
                    "SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = $1 ORDER BY tenant_id LIMIT 129",
                )
                .bind(swimlane.as_str())
                .fetch_all(&mut *tx),
            )
            .await?;
        self.db.commit(tx).await?;

        // A new stream starts clean: parts of an earlier one that never completed apply nothing (the agent sends the whole
        // delta again), and the old ones are swept (A14).
        let ttl = i64::try_from(self.config.staging_ttl.as_millis()).unwrap_or(i64::MAX);
        staging::discard(&self.db, swimlane, now_ms.saturating_sub(ttl)).await?;
        self.agent_config(&tenants)
    }

    fn agent_config(&self, tenants: &[String]) -> Result<AgentConfig, RegistryError> {
        let text = |s: &str| ShortText::parse(s).map_err(|_| RegistryError::Invalid);
        Ok(AgentConfig {
            scan_interval_secs: self.config.scan_interval_secs,
            heartbeat_interval_secs: self.config.heartbeat_interval_secs,
            max_file_bytes: self.config.max_file_bytes,
            deny_globs: self
                .config
                .deny_globs()
                .iter()
                .map(|g| text(g))
                .collect::<Result<_, _>>()?,
            env_allowlist: self
                .config
                .env_allowlist
                .iter()
                .map(|n| text(n))
                .collect::<Result<_, _>>()?,
            tenants: tenants.iter().filter_map(|t| TenantId::parse(t).ok()).collect(),
        })
    }

    pub(crate) async fn sweep_staging(&self) -> Result<u64, RegistryError> {
        let now_ms = self.clock.now().unix_millis();
        let ttl = i64::try_from(self.config.staging_ttl.as_millis()).unwrap_or(i64::MAX);
        // No swimlane is named: the sweep drops by age only.
        let done = self
            .db
            .run(
                "staging_sweep",
                sqlx::query(&format!(
                    "DELETE FROM delta_staging WHERE staged_at < {}",
                    ts("$1")
                ))
                .bind(now_ms.saturating_sub(ttl))
                .execute(self.db.pool()),
            )
            .await?;
        Ok(done.rows_affected())
    }
}
