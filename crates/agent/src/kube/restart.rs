//! Restarting a Deployment (D28, S17): the same merge patch `kubectl rollout restart` sends.
//!
//! The agent sets the `kubectl.kubernetes.io/restartedAt` annotation on the pod template to the current time, and the
//! Deployment controller rolls the pods. Only Deployments in the configured namespaces are touched; a request for any
//! other namespace is refused before a single call is made to the API server.

use std::collections::BTreeSet;
use std::sync::Arc;

use ::kube::Client;
use ::kube::api::{Api, Patch, PatchParams};
use domain::ServiceRef;
use k8s_openapi::api::apps::v1::Deployment;
use serde_json::json;
use tracing::info;

use super::error::RestartError;
use crate::clock::Clock;
use crate::identity::KubeError;
use crate::identity::kubecall::kube_call;

/// The annotation `kubectl rollout restart` sets.
pub const RESTARTED_AT: &str = "kubectl.kubernetes.io/restartedAt";
/// The field manager recorded on the patch, so that an operator can see who restarted a Deployment.
const FIELD_MANAGER: &str = "lanekeeper-agent";
const PATCH_DEPLOYMENT: &str = "restart a Deployment";

/// Restarts Deployments.
pub struct Restarter {
    client: Client,
    namespaces: BTreeSet<String>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Restarter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Restarter")
            .field("namespaces", &self.namespaces)
            .finish_non_exhaustive()
    }
}

impl Restarter {
    pub fn new<'a>(
        client: Client,
        namespaces: impl IntoIterator<Item = &'a str>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            client,
            namespaces: namespaces.into_iter().map(str::to_owned).collect(),
            clock,
        }
    }

    pub async fn restart(&self, service: &ServiceRef) -> Result<(), RestartError> {
        if !self.namespaces.contains(service.namespace()) {
            info!(
                namespace = service.namespace(),
                name = service.name(),
                "restart refused: namespace not configured"
            );
            return Err(RestartError::NamespaceNotAllowed);
        }
        let stamp = restarted_at(self.clock.now().unix_millis());
        let patch = json!({
            "spec": { "template": { "metadata": { "annotations": { RESTARTED_AT: stamp } } } }
        });
        let api: Api<Deployment> = Api::namespaced(self.client.clone(), service.namespace());
        let params = PatchParams {
            field_manager: Some(FIELD_MANAGER.to_owned()),
            ..PatchParams::default()
        };
        match kube_call(
            PATCH_DEPLOYMENT,
            api.patch(service.name(), &params, &Patch::Merge(&patch)),
        )
        .await
        {
            Ok(_) => {
                info!(
                    namespace = service.namespace(),
                    name = service.name(),
                    "restart requested"
                );
                Ok(())
            }
            Err(KubeError::Status { status: 404, .. }) => Err(RestartError::NotFound),
            Err(error) => Err(RestartError::Api(error)),
        }
    }
}

/// RFC 3339 in UTC to the second, such as `2026-10-10T12:00:00Z`, as `kubectl` writes it.
fn restarted_at(unix_millis: i64) -> String {
    k8s_openapi::jiff::Timestamp::from_second(unix_millis.div_euclid(1000))
        .map_or_else(|_| "1970-01-01T00:00:00Z".to_owned(), |t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_rfc3339_utc_to_the_second() {
        assert_eq!(restarted_at(1_791_633_600_000), "2026-10-10T12:00:00Z");
        assert_eq!(restarted_at(1_791_633_600_999), "2026-10-10T12:00:00Z");
        assert_eq!(restarted_at(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_time_before_1970_still_makes_a_stamp() {
        assert_eq!(restarted_at(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn a_time_jiff_cannot_represent_makes_the_epoch_and_not_a_panic() {
        assert_eq!(restarted_at(i64::MAX), "1970-01-01T00:00:00Z");
    }
}
