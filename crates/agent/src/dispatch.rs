//! The hub's commands, and what the agent answers (T5, S11).
//!
//! A command reaches [`CommandHandler::handle`] only after `proto::convert` has validated it: the path is an
//! [`NfsPath`], the hash is 32 bytes, the request id is a token. The handler turns it into an operation and the outcome
//! into exactly one answer for the hub.
//!
//! # The answer codes
//!
//! The wire has five error codes, so every outcome maps to one of them. No answer carries a path or the text of an OS
//! error: the hub learns the code (and, for a conflict, the current hash), the log learns the path and the kind.
//!
//! | Outcome | Code |
//! |---|---|
//! | the hash on disk is not the expected one | `CONFLICT`, with the current hash |
//! | the file, or its directory, does not exist | `NOT_FOUND` |
//! | a symbolic link, a reserved name, a deny glob (D79), a path that failed validation | `DENIED` |
//! | not a regular file, over 2 MiB, a command this build does not carry out | `UNSUPPORTED` |
//! | anything else from the file system, a timeout, too many requests at once | `IO` |
//! | a restart in a namespace the agent was not given | `DENIED` |
//! | a restart of a Deployment that does not exist | `NOT_FOUND` |
//! | the Kubernetes API refused, timed out, or the watchers have not listed yet | `IO` |
//!
//! A command whose part of the agent is not there is answered `UNSUPPORTED` at once rather than left to time out at the
//! hub. The cluster commands (`RequestClusterReport`, `RestartDeployment`) are carried out by a [`ClusterOps`] given to
//! [`Dispatcher::with_cluster`], and the config-server commands (`NotifyConfigServer`, `FetchServed`) by a
//! [`ConfigServerClient`] given to [`Dispatcher::with_config_server`] (only when `LK_CONFIG_SERVER_URL` is set); without
//! one they are `UNSUPPORTED` too.
//!
//! # The config-server commands (T12)
//!
//! | Outcome | Answer |
//! |---|---|
//! | the config-server answered a refresh, with any status | `NotifyResult` with that status, unchanged |
//! | the config-server served a file, or answered with any status | `ServedResponse` with the status, and the bytes of a `2xx` |
//! | more than five fetches in a second | `ServedResponse` `429`, no body, the config-server is not asked |
//! | the path is covered by the deny list (D79) | `DENIED`, with no path in it |
//! | no connection, a timeout (after the retries of a refresh), a peer that is not HTTP | `IO` |
//! | a response over 2 MiB | `UNSUPPORTED` |

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use domain::{
    AgentReply, ContentHash, HubCommand, NfsPath, OpError, OpResult, RequestId, ServeRequest, ServiceRef,
};
use proto::convert::FromAgent;
use tracing::{debug, info, warn};

use crate::configserver::{ConfigServerClient, ConfigServerError};
use crate::fileops::{FileContent, FileError, FileOps, OpOutcome, WriteHooks};
use crate::kube::ClusterOps;
use crate::ops::{Metrics, Operation, Outcome};

/// What a command may do, from the hub's settings as they are now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpLimits {
    /// The largest file the agent reads into an answer or accepts in a write.
    pub max_file_bytes: u64,
}

/// Carries out the commands the scan loop does not.
#[async_trait]
pub trait CommandHandler: Send + Sync + fmt::Debug + 'static {
    /// The answer to `command`. `None` for the commands that are not answered with a reply (`RequestDelta` and
    /// `RequestFullScan` are answered with deltas, by the scanner).
    async fn handle(&self, command: HubCommand, limits: OpLimits) -> Option<FromAgent>;
}

/// The request id of a command that has one.
pub fn request_id_of(command: &HubCommand) -> Option<&RequestId> {
    match command {
        HubCommand::RequestDelta { .. } | HubCommand::RequestFullScan => None,
        HubCommand::ReadFile { request_id, .. }
        | HubCommand::WriteFile { request_id, .. }
        | HubCommand::DeleteFile { request_id, .. }
        | HubCommand::RestartDeployment { request_id, .. }
        | HubCommand::RequestClusterReport { request_id }
        | HubCommand::NotifyConfigServer { request_id, .. }
        | HubCommand::FetchServed { request_id, .. } => Some(request_id),
    }
}

/// The operation a command is counted as, for the commands that have an answer.
pub fn operation_of(command: &HubCommand) -> Option<Operation> {
    match command {
        HubCommand::RequestDelta { .. } | HubCommand::RequestFullScan => None,
        HubCommand::ReadFile { .. } => Some(Operation::Read),
        HubCommand::WriteFile { .. } => Some(Operation::Write),
        HubCommand::DeleteFile { .. } => Some(Operation::Delete),
        HubCommand::RestartDeployment { .. } => Some(Operation::Restart),
        HubCommand::RequestClusterReport { .. } => Some(Operation::ClusterReport),
        HubCommand::NotifyConfigServer { .. } => Some(Operation::Notify),
        HubCommand::FetchServed { .. } => Some(Operation::FetchServed),
    }
}

/// A failed `OpResult`: the code, and the current hash when there is one.
pub fn failure(request_id: RequestId, code: OpError, current_hash: Option<ContentHash>) -> FromAgent {
    FromAgent::Reply(AgentReply::Op(OpResult {
        request_id,
        ok: false,
        error: Some(code),
        current_hash,
    }))
}

/// The answer to a write or a delete.
pub fn op_reply(request_id: RequestId, result: Result<OpOutcome, FileError>) -> FromAgent {
    match result {
        Ok(done) => FromAgent::Reply(AgentReply::Op(OpResult {
            request_id,
            ok: true,
            error: None,
            current_hash: done.current_hash,
        })),
        Err(error) => failure(request_id, error.code(), error.current_hash()),
    }
}

/// The answer to a read.
pub fn read_reply(request_id: RequestId, path: NfsPath, result: Result<FileContent, FileError>) -> FromAgent {
    match result {
        Ok(content) => FromAgent::Reply(AgentReply::File {
            request_id,
            path,
            hash: content.hash,
            bytes: content.bytes,
        }),
        Err(error) => failure(request_id, error.code(), error.current_hash()),
    }
}

/// Carries out the file commands with [`FileOps`].
pub struct Dispatcher<H: WriteHooks = crate::fileops::NoHooks> {
    ops: FileOps<H>,
    cluster: Option<Arc<dyn ClusterOps>>,
    config_server: Option<Arc<ConfigServerClient>>,
    metrics: Arc<Metrics>,
}

impl<H: WriteHooks> fmt::Debug for Dispatcher<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("ops", &self.ops)
            .field("cluster", &self.cluster.is_some())
            .field("config_server", &self.config_server.is_some())
            .finish_non_exhaustive()
    }
}

impl<H: WriteHooks> Dispatcher<H> {
    pub fn new(ops: FileOps<H>) -> Self {
        Self {
            ops,
            cluster: None,
            config_server: None,
            metrics: Metrics::detached(),
        }
    }

    /// Count every command by operation and outcome in `metrics` (T7).
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// Count the command and hand its answer back.
    fn counted(&self, operation: Operation, reply: FromAgent) -> FromAgent {
        self.metrics.operation(operation, outcome_of(&reply));
        reply
    }

    /// Let the dispatcher answer `RequestClusterReport` and `RestartDeployment` (T6).
    #[must_use]
    pub fn with_cluster(mut self, cluster: Arc<dyn ClusterOps>) -> Self {
        self.cluster = Some(cluster);
        self
    }

    /// Let the dispatcher answer `NotifyConfigServer` and `FetchServed` (T12). Without one they are `UNSUPPORTED`.
    #[must_use]
    pub fn with_config_server(mut self, client: Arc<ConfigServerClient>) -> Self {
        self.config_server = Some(client);
        self
    }

    /// As [`Dispatcher::with_config_server`], when there is a client (`LK_CONFIG_SERVER_URL` is set).
    #[must_use]
    pub fn with_config_server_opt(mut self, client: Option<Arc<ConfigServerClient>>) -> Self {
        self.config_server = client;
        self
    }

    /// Ask the config-server to refresh `paths`, and tell the hub what it said.
    async fn notify(
        &self,
        client: &ConfigServerClient,
        request_id: RequestId,
        paths: &[NfsPath],
    ) -> FromAgent {
        match client.notify(paths).await {
            Ok(status) => {
                info!(%request_id, paths = paths.len(), status, "the config-server answered a refresh");
                FromAgent::Reply(AgentReply::Notify { request_id, status })
            }
            Err(error) => {
                warn!(%request_id, paths = paths.len(), %error, "a refresh of the config-server failed");
                failure(request_id, error.code(), None)
            }
        }
    }

    /// Ask the config-server for what it serves, and hand the hub the status and the bytes.
    async fn fetch_served(
        &self,
        client: &ConfigServerClient,
        request_id: RequestId,
        request: &ServeRequest,
    ) -> FromAgent {
        match client.fetch_served(request).await {
            Ok(served) => {
                debug!(%request_id, status = served.status, bytes = served.body.len(), "a served file");
                FromAgent::Reply(AgentReply::Served {
                    request_id,
                    status: served.status,
                    bytes: served.body,
                })
            }
            // A refused path is named nowhere: not in the answer, and not here.
            Err(error @ ConfigServerError::Denied) => {
                info!(%request_id, %error, "a fetch of a served file was refused");
                failure(request_id, error.code(), None)
            }
            Err(error) => {
                warn!(%request_id, %error, "a fetch of a served file failed");
                failure(request_id, error.code(), None)
            }
        }
    }

    async fn cluster_report(&self, cluster: &dyn ClusterOps, request_id: RequestId) -> FromAgent {
        match cluster.full_report().await {
            Ok(report) => {
                debug!(%request_id, deployments = report.deployments.len(), "cluster report answered");
                FromAgent::Reply(AgentReply::Cluster { request_id, report })
            }
            Err(error) => {
                warn!(%request_id, %error, "cluster report refused");
                failure(request_id, error.code(), None)
            }
        }
    }

    async fn restart(
        &self,
        cluster: &dyn ClusterOps,
        request_id: RequestId,
        service: &ServiceRef,
    ) -> FromAgent {
        match cluster.restart(service).await {
            Ok(()) => FromAgent::Reply(AgentReply::Op(OpResult {
                request_id,
                ok: true,
                error: None,
                current_hash: None,
            })),
            Err(error) => {
                info!(%request_id, namespace = service.namespace(), name = service.name(), code = ?error.code(), %error, "restart refused");
                failure(request_id, error.code(), None)
            }
        }
    }
}

/// Log how an operation went: paths, ids and codes only (S10). A conflict is routine; a failure is not.
fn log_outcome<T>(operation: &str, request_id: &RequestId, path: &NfsPath, result: &Result<T, FileError>) {
    match result {
        Ok(_) => debug!(operation, %request_id, %path, "file operation done"),
        Err(error @ FileError::Conflict { .. }) => {
            info!(operation, %request_id, %path, current = ?error.current_hash(), "file operation refused: conflict");
        }
        Err(error @ (FileError::NotFound | FileError::Denied(_) | FileError::Unsupported(_))) => {
            info!(operation, %request_id, %path, code = ?error.code(), reason = %error, "file operation refused");
        }
        Err(error @ FileError::Io(kind)) => {
            warn!(operation, %request_id, %path, code = ?error.code(), kind = ?kind, "file operation failed");
        }
    }
}

#[async_trait]
impl<H: WriteHooks> CommandHandler for Dispatcher<H> {
    async fn handle(&self, command: HubCommand, limits: OpLimits) -> Option<FromAgent> {
        match command {
            HubCommand::RequestDelta { .. } | HubCommand::RequestFullScan => None,
            HubCommand::ReadFile { request_id, path } => {
                let result = self.ops.read(&path, limits.max_file_bytes).await;
                log_outcome("read", &request_id, &path, &result);
                Some(self.counted(Operation::Read, read_reply(request_id, path, result)))
            }
            HubCommand::WriteFile {
                request_id,
                path,
                expected,
                bytes,
            } => {
                let result = self
                    .ops
                    .write(&path, expected, bytes, limits.max_file_bytes)
                    .await;
                log_outcome("write", &request_id, &path, &result);
                Some(self.counted(Operation::Write, op_reply(request_id, result)))
            }
            HubCommand::DeleteFile {
                request_id,
                path,
                expected,
            } => {
                let result = self.ops.delete(&path, expected).await;
                log_outcome("delete", &request_id, &path, &result);
                Some(self.counted(Operation::Delete, op_reply(request_id, result)))
            }
            HubCommand::RequestClusterReport { request_id } => {
                let reply = match &self.cluster {
                    Some(cluster) => self.cluster_report(cluster.as_ref(), request_id).await,
                    None => unsupported(request_id),
                };
                Some(self.counted(Operation::ClusterReport, reply))
            }
            HubCommand::RestartDeployment { request_id, service } => {
                let reply = match &self.cluster {
                    Some(cluster) => self.restart(cluster.as_ref(), request_id, &service).await,
                    None => unsupported(request_id),
                };
                Some(self.counted(Operation::Restart, reply))
            }
            HubCommand::NotifyConfigServer { request_id, paths } => {
                let reply = match &self.config_server {
                    Some(client) => self.notify(client, request_id, &paths).await,
                    None => unsupported(request_id),
                };
                Some(self.counted(Operation::Notify, reply))
            }
            HubCommand::FetchServed { request_id, request } => {
                let reply = match &self.config_server {
                    Some(client) => self.fetch_served(client, request_id, &request).await,
                    None => unsupported(request_id),
                };
                Some(self.counted(Operation::FetchServed, reply))
            }
        }
    }
}

/// How an answer ended, for the operation counters.
fn outcome_of(reply: &FromAgent) -> Outcome {
    match reply {
        FromAgent::Reply(AgentReply::Op(result)) => Outcome::of(if result.ok {
            None
        } else {
            Some(result.error.unwrap_or(OpError::Io))
        }),
        // The config-server answered, but a refresh it refused, or a file it did not serve, did not go well.
        FromAgent::Reply(AgentReply::Notify { status, .. }) if !(200..300).contains(status) => Outcome::Io,
        FromAgent::Reply(AgentReply::Served { status: 404, .. }) => Outcome::NotFound,
        FromAgent::Reply(AgentReply::Served { status, .. }) if !(200..300).contains(status) => Outcome::Io,
        _ => Outcome::Ok,
    }
}

/// The answer to a command this build does not carry out.
fn unsupported(request_id: RequestId) -> FromAgent {
    info!(%request_id, "a command this build does not carry out; answered UNSUPPORTED");
    failure(request_id, OpError::Unsupported, None)
}

#[cfg(test)]
mod tests {
    use std::io;

    use bytes::Bytes;
    use domain::ContentHash;

    use super::*;
    use crate::fileops::{DeniedReason, UnsupportedReason};

    fn id(text: &str) -> RequestId {
        RequestId::parse(text).unwrap()
    }

    fn only_op(reply: FromAgent) -> OpResult {
        match reply {
            FromAgent::Reply(AgentReply::Op(result)) => result,
            other => panic!("an OpResult, got {other:?}"),
        }
    }

    #[test]
    fn every_error_maps_to_one_code() {
        let hash = ContentHash::from_bytes([3; 32]);
        let table = [
            (
                FileError::Conflict { current: Some(hash) },
                OpError::Conflict,
                Some(hash),
            ),
            (FileError::Conflict { current: None }, OpError::Conflict, None),
            (FileError::NotFound, OpError::NotFound, None),
            (FileError::Denied(DeniedReason::Symlink), OpError::Denied, None),
            (
                FileError::Denied(DeniedReason::ReservedName),
                OpError::Denied,
                None,
            ),
            (FileError::Denied(DeniedReason::DenyGlob), OpError::Denied, None),
            (
                FileError::Unsupported(UnsupportedReason::NotRegular),
                OpError::Unsupported,
                None,
            ),
            (
                FileError::Unsupported(UnsupportedReason::TooLarge),
                OpError::Unsupported,
                None,
            ),
            (FileError::Io(io::ErrorKind::TimedOut), OpError::Io, None),
            (FileError::Io(io::ErrorKind::PermissionDenied), OpError::Io, None),
        ];
        for (error, code, current) in table {
            let result = only_op(op_reply(id("r1"), Err(error)));
            assert!(!result.ok);
            assert_eq!(result.error, Some(code), "{error:?}");
            assert_eq!(result.current_hash, current, "{error:?}");
        }
    }

    #[test]
    fn a_done_write_carries_the_new_hash_and_a_done_delete_none() {
        let hash = ContentHash::from_bytes([4; 32]);
        let written = only_op(op_reply(
            id("r2"),
            Ok(OpOutcome {
                current_hash: Some(hash),
            }),
        ));
        assert!(written.ok);
        assert_eq!((written.error, written.current_hash), (None, Some(hash)));
        let deleted = only_op(op_reply(id("r3"), Ok(OpOutcome { current_hash: None })));
        assert!(deleted.ok);
        assert_eq!((deleted.error, deleted.current_hash), (None, None));
    }

    #[test]
    fn a_read_answers_with_the_file_or_with_a_code() {
        let hash = ContentHash::from_bytes([5; 32]);
        let path = NfsPath::parse("svc/a.yml").unwrap();
        let ok = read_reply(
            id("r4"),
            path.clone(),
            Ok(FileContent {
                bytes: Bytes::from_static(b"x"),
                hash,
            }),
        );
        assert!(matches!(
            ok,
            FromAgent::Reply(AgentReply::File { hash: h, ref bytes, .. }) if h == hash && &bytes[..] == b"x"
        ));
        let missing = only_op(read_reply(id("r5"), path, Err(FileError::NotFound)));
        assert_eq!(missing.error, Some(OpError::NotFound));
    }

    #[test]
    fn every_command_with_a_reply_has_a_request_id() {
        let rid = id("r6");
        let path = NfsPath::parse("a").unwrap();
        let commands = [
            HubCommand::ReadFile {
                request_id: rid.clone(),
                path: path.clone(),
            },
            HubCommand::DeleteFile {
                request_id: rid.clone(),
                path: path.clone(),
                expected: ContentHash::from_bytes([0; 32]),
            },
            HubCommand::RequestClusterReport {
                request_id: rid.clone(),
            },
            HubCommand::NotifyConfigServer {
                request_id: rid.clone(),
                paths: vec![path],
            },
        ];
        for command in &commands {
            assert_eq!(request_id_of(command), Some(&rid));
        }
        assert_eq!(request_id_of(&HubCommand::RequestFullScan), None);
        assert_eq!(
            request_id_of(&HubCommand::RequestDelta {
                since_root: ContentHash::from_bytes([1; 32])
            }),
            None
        );
    }
}
