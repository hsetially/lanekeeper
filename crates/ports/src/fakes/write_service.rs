use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AuditId, ChangeEol, ChangePreview, ContentHash, DeleteRequest, Draft, DraftId, EditRequest, Expected,
    IdempotencyKey, NfsPath, NotifyRequest, PrRequest, PreviewChange, PreviewId, PreviewRequest,
    PreviewScope, ProposalId, ProposalStatus, RejectReason, RestartRequest, RevertRequest, Role, ServiceRef,
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
/// Issued preview tokens the fake remembers; at the limit it forgets all of them, like it does idempotency keys.
const MAX_PREVIEWS: usize = 10_000;
/// Notifications the fake remembers.
const MAX_NOTIFICATIONS: usize = 1_000;
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
    previews: HashSet<PreviewId>,
    notifications: Vec<(SwimlaneId, Vec<NfsPath>)>,
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

    /// The config-server notifications that were carried out, oldest first.
    pub fn notifications(&self) -> Vec<(SwimlaneId, Vec<NfsPath>)> {
        lock(&self.state).notifications.clone()
    }
}

fn rejected(reason: RejectReason) -> WriteOutcome {
    WriteOutcome::Rejected {
        reason,
        issues: Vec::new(),
    }
}

fn applied(hash: Option<ContentHash>, audit_id: AuditId) -> WriteOutcome {
    WriteOutcome::Applied {
        hash,
        audit_id,
        warnings: Vec::new(),
    }
}

/// The line ending style of text that has one: all CRLF or all LF. Mixed text and text with no line break have none.
fn style_of(text: &[u8]) -> Option<ChangeEol> {
    let (mut crlf, mut lf) = (0usize, 0usize);
    for (i, b) in text.iter().enumerate() {
        if *b == b'\n' {
            if i > 0 && text[i - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
    }
    match (crlf, lf) {
        (0, 0) => None,
        (_, 0) => Some(ChangeEol::Crlf),
        (0, _) => Some(ChangeEol::Lf),
        _ => None,
    }
}

/// What the file's content becomes: `change` wins; otherwise, when `restore` is set, the style the file already has;
/// otherwise the content as it is. Binary content (it has a NUL byte) is never touched. The real service also restores
/// the encoding and the BOM; the fake handles line endings only.
fn restyle(existing: Option<&Bytes>, content: &Bytes, change: Option<ChangeEol>, restore: bool) -> Bytes {
    if content.contains(&0) {
        return content.clone();
    }
    let target = change.or_else(|| {
        if restore {
            existing.and_then(|b| style_of(b))
        } else {
            None
        }
    });
    let Some(target) = target else {
        return content.clone();
    };
    let mut out = Vec::with_capacity(content.len() + content.len() / 16);
    for (i, b) in content.iter().enumerate() {
        match *b {
            // The CR of a CRLF pair is dropped here and written back below when the target is CRLF.
            b'\r' if content.get(i + 1) == Some(&b'\n') => {}
            b'\n' => {
                if target == ChangeEol::Crlf {
                    out.push(b'\r');
                }
                out.push(b'\n');
            }
            other => out.push(other),
        }
    }
    Bytes::from(out)
}

/// The token the fake hands out for a change: bound to the user, the swimlane, the path and the hashes.
fn preview_token(
    user: &UserId,
    s: &SwimlaneId,
    path: &NfsPath,
    action: &str,
    expected: &str,
    new_hash: &str,
) -> Option<PreviewId> {
    let fp = content_hash(format!("{user}|{s}|{path}|{action}|{expected}|{new_hash}").as_bytes());
    PreviewId::parse(&format!("pv-{fp}")).ok()
}

fn expected_label(e: &Expected) -> String {
    match e {
        Expected::Absent => "absent".to_owned(),
        Expected::Hash { hash } => hash.to_string(),
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

    /// A reason to refuse an upload or a delete: it has no preview, or one that was not issued to this user for
    /// exactly this change.
    fn preview_is_valid(&self, presented: Option<&PreviewId>, expected_id: Option<&PreviewId>) -> bool {
        match (presented, expected_id) {
            (Some(p), Some(e)) => p == e && self.previews.contains(p),
            _ => false,
        }
    }

    /// `restore`: keep the style of the file that is there. `change`: convert to this style.
    fn write(
        &mut self,
        ctx: &WriteCtx,
        s: &SwimlaneId,
        path: &NfsPath,
        expected: Expected,
        content: &Bytes,
        (restore, change): (bool, Option<ChangeEol>),
    ) -> WriteOutcome {
        if content.len() > MAX_FILE_BYTES {
            return rejected(RejectReason::TooLarge);
        }
        let Some(existing) = self.swimlanes.get(s).map(|sw| sw.files.get(path).cloned()) else {
            return rejected(RejectReason::UnknownSwimlane);
        };
        let content = &restyle(existing.as_ref(), content, change, restore);
        if content.len() > MAX_FILE_BYTES {
            return rejected(RejectReason::TooLarge);
        }
        if ctx.user.requires_approval {
            return self.proposal();
        }
        let audit = self.audit_id();
        let Some(sw) = self.swimlanes.get_mut(s) else {
            return rejected(RejectReason::UnknownSwimlane);
        };
        let current = sw.files.get(path).map(|b| content_hash(b));
        if !expected.matches(current.as_ref()) {
            return WriteOutcome::Conflict { current };
        }
        if (current.is_none() && sw.files.len() >= MAX_FILES) || sw.versions.len() >= MAX_VERSIONS {
            return rejected(RejectReason::NotAllowed);
        }
        let hash = content_hash(content);
        sw.versions.insert(hash, content.clone());
        sw.files.insert(path.clone(), content.clone());
        applied(Some(hash), audit)
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
            Ok(st.write(
                ctx,
                &r.swimlane,
                &r.path,
                r.expected,
                &r.content,
                (true, r.change_eol),
            ))
        })
    }

    async fn upload(&self, ctx: &WriteCtx, r: UploadRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("upload {r:?}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            let token = preview_token(
                &ctx.user.id,
                &r.swimlane,
                &r.path,
                "upload",
                &expected_label(&r.expected),
                &content_hash(&r.content).to_string(),
            );
            if !st.preview_is_valid(r.preview.as_ref(), token.as_ref()) {
                return Ok(rejected(RejectReason::NotAllowed));
            }
            Ok(st.write(ctx, &r.swimlane, &r.path, r.expected, &r.content, (true, None)))
        })
    }

    async fn delete(&self, ctx: &WriteCtx, r: DeleteRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("delete {r:?}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            let token = preview_token(
                &ctx.user.id,
                &r.swimlane,
                &r.path,
                "delete",
                &r.expected.to_string(),
                "none",
            );
            if !st.preview_is_valid(r.preview.as_ref(), token.as_ref()) {
                return Ok(rejected(RejectReason::NotAllowed));
            }
            if !st.swimlanes.contains_key(&r.swimlane) {
                return Ok(rejected(RejectReason::UnknownSwimlane));
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
            Ok(applied(None, audit))
        })
    }

    async fn revert(&self, ctx: &WriteCtx, r: RevertRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("revert {r:?}");
        self.execute(ctx, Role::Editor, &fp, |st| {
            let Some(sw) = st.swimlanes.get(&r.swimlane) else {
                return Ok(rejected(RejectReason::UnknownSwimlane));
            };
            let Some(content) = sw.versions.get(&r.to).cloned() else {
                return Ok(rejected(RejectReason::InvalidContent));
            };
            // A stored version is written as it was, unless the caller asks for another style.
            Ok(st.write(
                ctx,
                &r.swimlane,
                &r.path,
                r.expected,
                &content,
                (false, r.change_eol),
            ))
        })
    }

    async fn restart(&self, ctx: &WriteCtx, r: RestartRequest) -> Result<WriteOutcome, WriteError> {
        let fp = format!("restart {r:?}");
        self.execute(ctx, Role::Operator, &fp, |st| {
            if !st.swimlanes.contains_key(&r.swimlane) {
                return Ok(rejected(RejectReason::UnknownSwimlane));
            }
            if ctx.user.requires_approval {
                return Ok(st.proposal());
            }
            let audit = st.audit_id();
            st.restarts.push((r.swimlane.clone(), r.service.clone()));
            Ok(applied(None, audit))
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
                return Ok(rejected(RejectReason::UnknownSwimlane));
            }
            if ctx.user.requires_approval {
                return Ok(st.proposal());
            }
            Ok(applied(None, st.audit_id()))
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
        let existing = st
            .swimlanes
            .get(&r.swimlane)
            .and_then(|sw| sw.files.get(&r.path).cloned());
        let content = restyle(existing.as_ref(), &r.content, r.change_eol, true);
        if content.len() > MAX_FILE_BYTES {
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
            new_hash: content_hash(&content),
            hunks: Vec::new(),
            setting_changes: Vec::new(),
            status: ProposalStatus::Draft,
            expires_at: Timestamp::from_unix_millis(NOW_MS + DRAFT_TTL_MS),
        };
        st.drafts.insert(
            id,
            StoredDraft {
                draft: draft.clone(),
                content,
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
            // The draft holds the content as it was prepared when it was proposed.
            let outcome = st.write(ctx, &s, &path, expected, &content, (false, None));
            if matches!(outcome, WriteOutcome::Applied { .. })
                && let Some(d) = st.drafts.get_mut(&id)
            {
                d.draft.status = ProposalStatus::Applied;
            }
            Ok(outcome)
        })
    }

    async fn preview_effect(&self, ctx: &WriteCtx, r: PreviewRequest) -> Result<ChangePreview, WriteError> {
        self.preview(ctx, &r)
    }

    async fn notify(&self, ctx: &WriteCtx, r: NotifyRequest) -> Result<WriteOutcome, WriteError> {
        if r.paths.is_empty() || r.paths.len() > NotifyRequest::MAX_PATHS {
            // Checked before the idempotency record: a malformed request is never remembered.
            if !ctx.user.allows(Role::Operator) {
                return Err(WriteError::Forbidden);
            }
            return Err(WriteError::BadRequest);
        }
        let fp = format!("notify {r:?}");
        self.execute(ctx, Role::Operator, &fp, |st| {
            if !st.swimlanes.contains_key(&r.swimlane) {
                return Ok(rejected(RejectReason::UnknownSwimlane));
            }
            if ctx.user.requires_approval {
                return Ok(st.proposal());
            }
            let audit = st.audit_id();
            if st.notifications.len() >= MAX_NOTIFICATIONS {
                st.notifications.remove(0);
            }
            st.notifications.push((r.swimlane.clone(), r.paths.clone()));
            Ok(applied(None, audit))
        })
    }
}

impl FakeWriteService {
    fn preview(&self, ctx: &WriteCtx, r: &PreviewRequest) -> Result<ChangePreview, WriteError> {
        if !ctx.user.allows(Role::Editor) {
            return Err(WriteError::Forbidden);
        }
        let mut st = lock(&self.state);
        let Some(sw) = st.swimlanes.get(&r.swimlane) else {
            return Err(WriteError::NotFound);
        };
        let current = sw.files.get(&r.path).map(|b| content_hash(b));
        let (action, expected, new_hash, effective_changed) = match &r.change {
            PreviewChange::Upload { expected, content } => {
                if content.len() > MAX_FILE_BYTES {
                    return Err(WriteError::BadRequest);
                }
                let new = content_hash(content);
                (
                    "upload",
                    expected_label(expected),
                    new.to_string(),
                    current != Some(new),
                )
            }
            PreviewChange::Delete { expected } => ("delete", expected.to_string(), "none".to_owned(), true),
        };
        let id = preview_token(&ctx.user.id, &r.swimlane, &r.path, action, &expected, &new_hash)
            .ok_or(WriteError::Unavailable)?;
        if st.previews.len() >= MAX_PREVIEWS {
            st.previews.clear();
        }
        st.previews.insert(id.clone());
        Ok(ChangePreview {
            preview_id: id,
            expires_at: Timestamp::from_unix_millis(
                NOW_MS + i64::try_from(ChangePreview::TTL_SECS * 1000).unwrap_or(0),
            ),
            // The fake computes no facts: the real service takes these from the engine.
            scope: PreviewScope::EveryService,
            warnings: Vec::new(),
            changes: Vec::new(),
            changes_total: 0,
            effective_changed,
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
