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
//! | a symbolic link, a reserved name, a path that failed validation | `DENIED` |
//! | not a regular file, over 2 MiB, a command this build does not carry out | `UNSUPPORTED` |
//! | anything else from the file system, a timeout, too many requests at once | `IO` |
//!
//! Commands whose part of the agent is not built yet are answered `UNSUPPORTED` at once rather than left to time out
//! at the hub; each task that adds one (cluster, restart, config-server) takes it out of [`Dispatcher::handle`]'s
//! catch-all.

use std::fmt;

use async_trait::async_trait;
use domain::{AgentReply, ContentHash, HubCommand, NfsPath, OpError, OpResult, RequestId};
use proto::convert::FromAgent;
use tracing::{debug, info, warn};

use crate::fileops::{FileContent, FileError, FileOps, OpOutcome, WriteHooks};

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
}

impl<H: WriteHooks> fmt::Debug for Dispatcher<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher").field("ops", &self.ops).finish()
    }
}

impl<H: WriteHooks> Dispatcher<H> {
    pub fn new(ops: FileOps<H>) -> Self {
        Self { ops }
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
                Some(read_reply(request_id, path, result))
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
                Some(op_reply(request_id, result))
            }
            HubCommand::DeleteFile {
                request_id,
                path,
                expected,
            } => {
                let result = self.ops.delete(&path, expected).await;
                log_outcome("delete", &request_id, &path, &result);
                Some(op_reply(request_id, result))
            }
            // Cluster reports, restarts and the config-server calls arrive with T6 and T12. Until then the hub is told
            // so at once instead of waiting for a timeout.
            HubCommand::RestartDeployment { request_id, .. }
            | HubCommand::RequestClusterReport { request_id }
            | HubCommand::NotifyConfigServer { request_id, .. }
            | HubCommand::FetchServed { request_id, .. } => {
                info!(%request_id, "a command this build does not carry out; answered UNSUPPORTED");
                Some(failure(request_id, OpError::Unsupported, None))
            }
        }
    }
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
