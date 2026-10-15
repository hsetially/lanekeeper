//! Cluster reports and the pickup state, for the tests of T10: a Deployment as an agent would report it, the report around it, and what
//! `service_state` and the outbox hold afterwards.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{
    ClusterReport, DeploymentInfo, DomainEvent, EnvValue, PodInfo, ReleaseHint, ServiceRef, Timestamp,
};
use hub_registry::ManualClock;
use sqlx::{PgPool, Row};

use super::rig::{Rig, text};

pub const T0: i64 = ManualClock::T0_MS;
pub const MIN: i64 = 60_000;
pub const HOUR: i64 = 60 * MIN;

/// A Deployment in the namespace `csp`.
pub struct Dep {
    name: String,
    env: Vec<(String, String)>,
    pods: Vec<(String, i64)>,
}

pub fn dep(name: &str) -> Dep {
    Dep {
        name: name.to_owned(),
        env: Vec::new(),
        pods: Vec::new(),
    }
}

impl Dep {
    /// `SPRING_APPLICATION_NAME`.
    pub fn app(self, app: &str) -> Self {
        self.env("SPRING_APPLICATION_NAME", app)
    }

    /// `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED`.
    pub fn listens(self) -> Self {
        self.env("CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED", "true")
    }

    /// `CONFIG_CLIENT_CACHE_TTL`, in seconds.
    pub fn ttl(self, secs: u32) -> Self {
        self.env("CONFIG_CLIENT_CACHE_TTL", &secs.to_string())
    }

    pub fn env(mut self, name: &str, value: &str) -> Self {
        self.env.push((name.to_owned(), value.to_owned()));
        self
    }

    /// A pod that started at `started_ms`.
    pub fn pod(mut self, name: &str, started_ms: i64) -> Self {
        self.pods.push((name.to_owned(), started_ms));
        self
    }

    pub fn build(self) -> DeploymentInfo {
        DeploymentInfo {
            service: ServiceRef::new("csp", &self.name).unwrap(),
            pods: self
                .pods
                .iter()
                .map(|(name, at)| PodInfo {
                    name: text(name),
                    started_at: Timestamp::from_unix_millis(*at),
                })
                .collect(),
            env_values: self
                .env
                .iter()
                .map(|(n, v)| EnvValue {
                    name: text(n),
                    value: text(v),
                })
                .collect(),
            env_names: Vec::new(),
            removed: false,
        }
    }
}

/// A report of the Deployments, with the config-server's start when it is known.
pub fn report(
    full: bool,
    deployments: Vec<DeploymentInfo>,
    config_server_started: Option<i64>,
) -> ClusterReport {
    ClusterReport {
        full,
        deployments,
        release_hints: Vec::<ReleaseHint>::new(),
        config_server_started_at: config_server_started.map(Timestamp::from_unix_millis),
        sync_windows: Vec::new(),
    }
}

/// What `service_state` holds for a Deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub state: String,
    pub live_by: Option<i64>,
    pub pending_since: Option<i64>,
    pub files: Vec<String>,
}

pub async fn stored(pool: &PgPool, name: &str) -> Option<Stored> {
    let row = sqlx::query(
        "SELECT st.pickup_state, (EXTRACT(EPOCH FROM st.live_by) * 1000)::bigint AS live_by, \
                (EXTRACT(EPOCH FROM st.pending_since) * 1000)::bigint AS pending, st.pending_files \
         FROM service_state st JOIN services s ON s.id = st.service_id WHERE s.name = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .unwrap()?;
    let files: serde_json::Value = row.get("pending_files");
    Some(Stored {
        state: row.get("pickup_state"),
        live_by: row.get("live_by"),
        pending_since: row.get("pending"),
        files: files
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect(),
    })
}

/// The services each `PickupChanged` of the outbox names, in order.
pub async fn pickup_events(rig: &Rig) -> Vec<Vec<String>> {
    rig.bus
        .published()
        .await
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::PickupChanged { services, .. } => {
                Some(services.iter().map(|s| s.name().to_owned()).collect())
            }
            _ => None,
        })
        .collect()
}

/// How many `PendingRestartChanged` the outbox holds: none, since T10 (A2).
pub async fn old_events(rig: &Rig) -> usize {
    rig.bus
        .published()
        .await
        .iter()
        .filter(|e| matches!(e, DomainEvent::PendingRestartChanged { .. }))
        .count()
}
