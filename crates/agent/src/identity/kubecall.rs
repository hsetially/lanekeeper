//! One Kubernetes API call, with a timeout and an error that carries no response text (S10, S21).

use std::future::Future;
use std::time::Duration;

use super::error::KubeError;

/// Every call to the API server gets this long (rule 5). The server is in the cluster, so a slow answer means trouble.
pub(super) const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Run `call`, and turn a failure into a [`KubeError`] that names `op` and the HTTP status, never the server's message.
pub(super) async fn kube_call<T>(
    op: &'static str,
    call: impl Future<Output = Result<T, kube::Error>>,
) -> Result<T, KubeError> {
    match tokio::time::timeout(CALL_TIMEOUT, call).await {
        Err(_elapsed) => Err(KubeError::Timeout { op }),
        Ok(Ok(value)) => Ok(value),
        Ok(Err(kube::Error::Api(status))) => Err(KubeError::Status {
            op,
            status: status.code,
        }),
        Ok(Err(_)) => Err(KubeError::Transport { op }),
    }
}
