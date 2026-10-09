use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AuditId, ContentHash, DeleteRequest, Draft, DraftId, EditRequest, Expected, IdempotencyKey, NfsPath,
    PrRequest, ProposalId, ProposalStatus, RejectReason, RestartRequest, RevertRequest, Role, ServiceRef,
    SwimlaneId, Timestamp, UploadRequest, UserId, WriteCtx, WriteOutcome,
};

use super::util::lock;
use crate::conformance::WriteScenario;
use crate::{WriteError, WriteService, content_hash};

/// Largest file the fake accepts (`docs/performance.md`: files up to 2 MiB).
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const MAX_VERSIONS: usize = 10_000;
const MAX_KEYS: usize = 10_000;
const MAX_DRAFTS: usize = 1_000;
/// Drafts live for a day.
const DRAFT_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// The fake's clock stands still, so tests are deterministic.
const NOW_MS: i64 = 1_700_000_000_000;

#[derive(Default)]
struct SwimlaneData {
    files: BTreeMap<NfsPath, Bytes>,
    /// Every content that was ever on NFS here, for revert.
    versions: HashMap<ContentHash, Bytes>,
}

struct StoredDraft {
    draft: Draft,
    content: Bytes,
}

#[derive(Default)]
struct State {
    swimlanes: HashMap<SwimlaneId, SwimlaneData>,
    idempotency: HashMap<(UserId, IdempotencyKey), (ContentHash, WriteOutcome)>,
    drafts: HashMap<DraftId, StoredDraft>,
    next_audit: i64,
    next_proposal: i64,
    next_draft: u64,
    restarts: Vec<(SwimlaneId, ServiceRef)>,
    proposals: usize,
}

/// An in-memory NFS per swimlane behind the write rules: role floors, approvals become proposals, no blind
/// writes (a hash mismatch is a `Conflict` and writes nothing), idempotent retries.
#[derive(Clone, Default)]
pub struct FakeWriteService {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for FakeWriteService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeWriteService").finish_non_exhaustive()
    }
}

impl FakeWriteService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make `s` exist with exactly these files.
    pub fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]) {
        let mut data = SwimlaneData::default();
        for (p, b) in files {
            data.versions.insert(content_hash(b), b.clone());
            data.files.insert(p.clone(), b.clone());
        }
        lock(&self.state).swimlanes.insert(s.clone(), data);
    }

    /// The current content of a file, if any.
    pub fn nfs_content(&self, s: &SwimlaneId, p: &NfsPath) -> Option<Bytes> {
        lock(&self.state)
            .swimlanes
            .get(s)
            .and_then(|sw| sw.files.get(p).cloned())
    }

    /// Restarts that were carried out, oldest first.
    pub fn restarts(&self) -> Vec<(SwimlaneId, ServiceRef)> {
        lock(&self.state).restarts.clone()
    }

    /// How many proposals were created instead of writes.
    pub fn proposal_count(&self) -> usize {
        lock(&self.state).proposals
    }
}

impl State {
    fn audit_id(&mut self) -> AuditId {
        self.next_audit += 1;
        AuditId::from(self.next_audit)
    }

    fn proposal(&mut self) -> WriteOutcome {
        self.next_proposal += 1;
        self.proposals += 1;
        WriteOutcome::ProposalCreated {
            id: ProposalId::from(self.next_proposal),
        }
    }

    fn write(
        &mut self,
        ctx: &WriteCtx,
        s: &SwimlaneId,
        path: &NfsPath,
        expected: Expected,
        content: &Bytes,
    ) -> WriteOutcome {
        if content.len() > MAX_FILE_BYTES {
            return WriteOutcome::Rejected {
                reason: RejectReason::TooLarge,
            };
        }
        if !self.swimlanes.contains_key(s) {
            return WriteOutcome::Rejected {
                reason: RejectReason::UnknownSwimlane,
            };
        }
        if ctx.user.requires_approval {
            return self.proposal();
        }
        let audit = self.audit_id();
        let Some(sw) = self.swimlanes.get_mut(s) else {
            return WriteOutcome::Rejected {
                reason: RejectReason::UnknownSwimlane,
            };
        };
        let current = sw.files.get(path).map(|b| content_hash(b));
        if !expected.matches(current.as_ref()) {
            return WriteOutcome::Conflict { current };
        }
        if (current.is_none() && sw.files.len() >= MAX_FILES) || sw.versions.len() >= MAX_VERSIONS {
            return WriteOutcome::Rejected {
                reason: RejectReason::NotAllowed,
            };
        }
        let hash = content_hash(content);
        sw.versions.insert(hash, content.clone());
        sw.files.insert(path.clone(), content.clone());
        WriteOutcome::Applied {
            hash: Some(hash),
            audit_id: audit,
        }
    }
}

impl FakeWriteService {
    /// Role floor, then idempotency, then the operation. Only outcomes are remembered, never errors.
    fn execute(
        &self,
        ctx: &WriteCtx,
        min: Role,
        fingerprint: &str,
        op: impl FnOnce(&mut State) -> Result<WriteOutcome, WriteError>,
    ) -> Result<WriteOutcome, WriteError> {
        if !ctx.user.allows(min) {
            return Err(WriteError::Forbidden);
        }
        let fp = content_hash(fingerprint.as_bytes());
        let key = (ctx.user.id.clone(), ctx.idempotency_key.clone());
        let mut st = lock(&self.state);
        if let Some((stored, outcome)) = st.idempotency.get(&key) {
            return if *stored == fp {
                Ok(outcome.clone())
            } else {
                Err(WriteError::IdempotencyConflict)
            };
        }
        let outcome = op(&mut st)?;
        if st.idempotency.len() >= MAX_KEYS {
            st.idempotency.clear();
        }
        st.idempotency.insert(key, (fp, outcome.clone()));
        Ok(outcome)
    }
}

#[async_trait]
impl WriteService for FakeWriteService {
    async fn edit(&self, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("edit {r:?}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            Ok(st.write(ctx, &r.swimlane, &r.path, r.expected, &r.content))
        })
    }

    async fn upload(&self, ctx: &WriteCtx, r: UploadRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("upload {r:?}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            Ok(st.write(ctx, &r.swimlane, &r.path, r.expected, &r.content))
        })
    }

    async fn delete(&self, ctx: &WriteCtx, r: DeleteRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("delete {r:?}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            if !st.swimlanes.contains_key(&r.swimlane) {
                return Ok(WriteOutcome::Rejected {
                    reason: RejectReason::UnknownSwimlane,
                });
            }
            if ctx.user.requires_approval {
                return Ok(st.proposal());
            }
            let audit = st.audit_id();
            let Some(sw) = st.swimlanes.get_mut(&r.swimlane) else {
                return Err(WriteError::Unavailable);
            };
            let current = sw.files.get(&r.path).map(|b| content_hash(b));
            if current != Some(r.expected) {
                return Ok(WriteOutcome::Conflict { current });
            }
            sw.files.remove(&r.path);
            Ok(WriteOutcome::Applied {
                hash: None,
                audit_id: audit,
            })
        })
    }

    async fn revert(&self, ctx: &WriteCtx, r: RevertRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("revert {r:?}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            let Some(sw) = st.swimlanes.get(&r.swimlane) else {
                return Ok(WriteOutcome::Rejected {
                    reason: RejectReason::UnknownSwimlane,
                });
            };
            let Some(content) = sw.versions.get(&r.to).cloned() else {
                return Ok(WriteOutcome::Rejected {
                    reason: RejectReason::InvalidContent,
                });
            };
            Ok(st.write(ctx, &r.swimlane, &r.path, r.expected, &content))
        })
    }

    async fn restart(&self, ctx: &WriteCtx, r: RestartRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("restart {r:?}");
        self.execute(ctx, Role::Operator, &fp, |st| {
            if !st.swimlanes.contains_key(&r.swimlane) {
                return Ok(WriteOutcome::Rejected {
                    reason: RejectReason::UnknownSwimlane,
                });
            }
            if ctx.user.requires_approval {
                return Ok(st.proposal());
            }
            let audit = st.audit_id();
            st.restarts.push((r.swimlane.clone(), r.service.clone()));
            Ok(WriteOutcome::Applied {
                hash: None,
                audit_id: audit,
            })
        })
    }

    async fn raise_pr(&self, ctx: &WriteCtx, r: PrRequest) -> Result<WriteOutcome, WriteError> {
        if r.paths.is_empty() || r.paths.len() > PrRequest::MAX_PATHS {
            // Checked before the idempotency record: a malformed request is never remembered.
            if !ctx.user.allows(Role::Editor) {
                return Err(WriteError::Forbidden);
            }
            return Err(WriteError::BadRequest);
        }
        let fp = format!("raise_pr {r:?}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            if !st.swimlanes.contains_key(&r.swimlane) {
                return Ok(WriteOutcome::Rejected {
                    reason: RejectReason::UnknownSwimlane,
                });
            }
            if ctx.user.requires_approval {
                return Ok(st.proposal());
            }
            Ok(WriteOutcome::Applied {
                hash: None,
                audit_id: st.audit_id(),
            })
        })
    }

    async fn propose_draft(&self, ctx: &WriteCtx, r: EditRequest) -> Result<Draft, WriteError> {
        if !ctx.user.allows(Role::Editor) {
            return Err(WriteError::Forbidden);
        }
        if r.content.len() > MAX_FILE_BYTES {
            return Err(WriteError::BadRequest);
        }
        let mut st = lock(&self.state);
        if !st.swimlanes.contains_key(&r.swimlane) || st.drafts.len() >= MAX_DRAFTS {
            return Err(WriteError::BadRequest);
        }
        st.next_draft += 1;
        let id = DraftId::parse(&format!("draft-{}", st.next_draft)).map_err(|_| WriteError::Unavailable)?;
        let draft = Draft {
            id: id.clone(),
            author: ctx.user.id.clone(),
            swimlane: r.swimlane,
            path: r.path,
            expected: r.expected,
            new_hash: content_hash(&r.content),
            hunks: Vec::new(),
            setting_changes: Vec::new(),
            status: ProposalStatus::Draft,
            expires_at: Timestamp::from_unix_millis(NOW_MS + DRAFT_TTL_MS),
        };
        st.drafts.insert(
            id,
            StoredDraft {
                draft: draft.clone(),
                content: r.content,
            },
        );
        Ok(draft)
    }

    async fn apply_draft(&self, ctx: &WriteCtx, id: DraftId) -> Result<WriteOutcome, WriteError> {
        if !ctx.user.allows(Role::Editor) {
            return Err(WriteError::Forbidden);
        }
        let fp = format!("apply_draft {id}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            let stored = st
                .drafts
                .get(&id)
                .filter(|d| d.draft.author == ctx.user.id)
                .ok_or(WriteError::NotFound)?;
            let (s, path, expected, content) = (
                stored.draft.swimlane.clone(),
                stored.draft.path.clone(),
                stored.draft.expected,
                stored.content.clone(),
            );
            let outcome = st.write(ctx, &s, &path, expected, &content);
            if matches!(outcome, WriteOutcome::Applied { .. })
                && let Some(d) = st.drafts.get_mut(&id)
            {
                d.draft.status = ProposalStatus::Applied;
            }
            Ok(outcome)
        })
    }
}

#[async_trait]
impl WriteScenario for FakeWriteService {
    async fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]) {
        Self::seed(self, s, files);
    }

    async fn nfs_content(&self, s: &SwimlaneId, p: &NfsPath) -> Option<Bytes> {
        Self::nfs_content(self, s, p)
    }
}
