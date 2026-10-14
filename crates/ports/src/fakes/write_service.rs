use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AuditId, ChangeEol, ChangeKind, ChangePreview, ContentHash, DeleteRequest, Draft, DraftId, EditRequest,
    Expected, IdempotencyKey, NfsPath, NotifyRequest, PrRequest, PreviewChange, PreviewId, PreviewRequest,
    PreviewScope, PreviewedChange, ProposalId, ProposalStatus, RejectReason, RestartRequest, RevertRequest,
    Role, ServiceRef, SettingChange, SettingPath, SettingValue, SwimlaneId, Timestamp, UploadRequest, UserId,
    ValueType, WriteCtx, WriteOutcome,
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
/// Where the fake's clock starts. It stands still until a test moves it with [`FakeWriteService::advance_clock`], so
/// tests are deterministic.
const START_MS: i64 = 1_700_000_000_000;
/// Setting changes a preview lists (`changes_total` counts the rest).
const MAX_PREVIEW_CHANGES: usize = 50;

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

struct State {
    swimlanes: HashMap<SwimlaneId, SwimlaneData>,
    idempotency: HashMap<(UserId, IdempotencyKey), (ContentHash, WriteOutcome)>,
    drafts: HashMap<DraftId, StoredDraft>,
    next_audit: i64,
    next_proposal: i64,
    next_draft: u64,
    restarts: Vec<(SwimlaneId, ServiceRef)>,
    proposals: usize,
    /// Issued preview tokens and when each runs out (unix milliseconds).
    previews: HashMap<PreviewId, i64>,
    notifications: Vec<(SwimlaneId, Vec<NfsPath>)>,
    now_ms: i64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            swimlanes: HashMap::new(),
            idempotency: HashMap::new(),
            drafts: HashMap::new(),
            next_audit: 0,
            next_proposal: 0,
            next_draft: 0,
            restarts: Vec::new(),
            proposals: 0,
            previews: HashMap::new(),
            notifications: Vec::new(),
            now_ms: START_MS,
        }
    }
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

    /// Move the fake's clock forward: previews run out [`ChangePreview::TTL_SECS`] after they were issued.
    pub fn advance_clock(&self, secs: u64) {
        let ms = i64::try_from(secs.saturating_mul(1000)).unwrap_or(i64::MAX);
        let mut st = lock(&self.state);
        st.now_ms = st.now_ms.saturating_add(ms);
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

/// A line terminator, by the engine's rule (D17): CRLF, LF, or a CR that is not followed by LF.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Term {
    Crlf,
    Lf,
    Cr,
}

/// Where each terminator of `text` is and how many bytes it takes.
fn terminators(text: &[u8]) -> impl Iterator<Item = (usize, Term)> + '_ {
    let mut at = 0;
    std::iter::from_fn(move || {
        while let Some(b) = text.get(at) {
            let here = at;
            at += 1;
            match b {
                b'\r' if text.get(at) == Some(&b'\n') => {
                    at += 1;
                    return Some((here, Term::Crlf));
                }
                b'\r' => return Some((here, Term::Cr)),
                b'\n' => return Some((here, Term::Lf)),
                _ => {}
            }
        }
        None
    })
}

/// How many terminators of each kind: `(crlf, lf, cr)`.
fn count_terminators(text: &[u8]) -> (usize, usize, usize) {
    terminators(text).fold((0, 0, 0), |(crlf, lf, cr), (_, t)| match t {
        Term::Crlf => (crlf + 1, lf, cr),
        Term::Lf => (crlf, lf + 1, cr),
        Term::Cr => (crlf, lf, cr + 1),
    })
}

/// The style a file is written with, like the engine's `dominant_eol`: CRLF when CRLF terminators outnumber the rest
/// together (LF and lone CR), LF otherwise. A file with no line ending, or as many of each, is LF.
fn dominant((crlf, lf, cr): (usize, usize, usize)) -> ChangeEol {
    if crlf > lf + cr {
        ChangeEol::Crlf
    } else {
        ChangeEol::Lf
    }
}

/// The style a write is converted to: `change` wins; otherwise, when `restore` is set, the dominant style of the file
/// that is there, or for a new file the dominant style of the files in its folder; otherwise none (the content as it
/// is). A folder with no line endings in it gives none either.
fn target_style(
    sw: &SwimlaneData,
    path: &NfsPath,
    change: Option<ChangeEol>,
    restore: bool,
) -> Option<ChangeEol> {
    if change.is_some() || !restore {
        return change;
    }
    if let Some(existing) = sw.files.get(path) {
        return Some(dominant(count_terminators(existing)));
    }
    let folder = path.parent();
    let total = sw
        .files
        .iter()
        .filter(|(p, _)| p.parent() == folder)
        .map(|(_, b)| count_terminators(b))
        .fold((0, 0, 0), |a, c| (a.0 + c.0, a.1 + c.1, a.2 + c.2));
    (total != (0, 0, 0)).then(|| dominant(total))
}

/// What the file's content becomes in the style `target`. Binary content (it has a NUL byte) is never touched. The real
/// service also restores the encoding and the BOM; the fake handles line endings only.
fn restyle(content: &Bytes, target: Option<ChangeEol>) -> Bytes {
    let Some(target) = target else {
        return content.clone();
    };
    if content.contains(&0) {
        return content.clone();
    }
    let mut out = Vec::with_capacity(content.len() + content.len() / 16);
    let mut copied = 0;
    for (at, term) in terminators(content) {
        out.extend_from_slice(content.get(copied..at).unwrap_or_default());
        out.extend_from_slice(if target == ChangeEol::Crlf { b"\r\n" } else { b"\n" });
        copied = at + if term == Term::Crlf { 2 } else { 1 };
    }
    out.extend_from_slice(content.get(copied..).unwrap_or_default());
    Bytes::from(out)
}

/// `content` as it will be written to `path` of `sw`.
fn styled(
    sw: &SwimlaneData,
    path: &NfsPath,
    content: &Bytes,
    change: Option<ChangeEol>,
    restore: bool,
) -> Bytes {
    restyle(content, target_style(sw, path, change, restore))
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
            // Good until `expires_at`, which is `issued + TTL_SECS`: at that moment it is gone.
            (Some(p), Some(e)) => {
                p == e && self.previews.get(p).is_some_and(|expires| self.now_ms < *expires)
            }
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
        let Some(sw) = self.swimlanes.get(s) else {
            return rejected(RejectReason::UnknownSwimlane);
        };
        let content = &styled(sw, path, content, change, restore);
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
        let Some(sw) = st.swimlanes.get(&r.swimlane) else {
            return Err(WriteError::BadRequest);
        };
        let content = styled(sw, &r.path, &r.content, r.change_eol, true);
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
            expires_at: Timestamp::from_unix_millis(st.now_ms + DRAFT_TTL_MS),
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

/// The `key: value` pairs of a flat YAML-like text: unindented lines with a `: `. The fake computes no real facts (the
/// engine does); this is enough for a preview to have changes to list and to mask.
fn flat_settings(text: &[u8]) -> BTreeMap<String, String> {
    let text = String::from_utf8_lossy(text);
    text.split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.starts_with(['#', ' ', '\t']))
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// The marker `[REDACTED: N chars, rule]` when `value` is something the secret scan flags. The fake knows one rule, the
/// AWS access key id (`AKIA` and 16 upper case letters or digits); the real service uses the engine's whole scan.
fn flagged_marker(value: &str) -> Option<String> {
    let rest = value.strip_prefix("AKIA")?;
    let is_key = rest.len() == 16 && rest.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    is_key.then(|| format!("[REDACTED: {} chars, aws_access_key]", value.chars().count()))
}

/// One side of a change; a flagged value is its marker and nothing else.
fn side(value: Option<&String>) -> (Option<SettingValue>, bool) {
    let Some(value) = value else {
        return (None, false);
    };
    let marker = flagged_marker(value);
    let flagged = marker.is_some();
    let text = marker.unwrap_or_else(|| value.clone());
    (
        Some(SettingValue {
            text,
            ty: ValueType::String,
            flag: None,
        }),
        flagged,
    )
}

/// The settings that differ between two texts, sorted by path, with flagged values masked.
fn preview_changes(old: &[u8], new: &[u8]) -> Vec<PreviewedChange> {
    let (old, new) = (flat_settings(old), flat_settings(new));
    let paths: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    let mut out = Vec::new();
    for key in paths {
        let (before, after) = (old.get(key), new.get(key));
        let kind = match (before, after) {
            (Some(b), Some(a)) if b == a => continue,
            (Some(_), Some(_)) => ChangeKind::Changed,
            (None, Some(_)) => ChangeKind::Added,
            _ => ChangeKind::Removed,
        };
        let Ok(path) = SettingPath::parse(key) else {
            continue;
        };
        let (left, left_flagged) = side(before);
        let (right, right_flagged) = side(after);
        out.push(PreviewedChange {
            change: SettingChange {
                path,
                kind,
                left,
                right,
                left_location: None,
                right_location: None,
            },
            flagged: left_flagged || right_flagged,
        });
    }
    out
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
        let current = sw.files.get(&r.path);
        // The same limit as an upload, on the content as it would be written: the preview and the upload agree.
        let (action, expected, new_hash, changes, effective_changed) = match &r.change {
            PreviewChange::Upload { expected, content } => {
                if content.len() > MAX_FILE_BYTES {
                    return Err(WriteError::BadRequest);
                }
                let new = styled(sw, &r.path, content, None, true);
                if new.len() > MAX_FILE_BYTES {
                    return Err(WriteError::BadRequest);
                }
                let changes = preview_changes(current.map_or(&[], |b| &b[..]), &new);
                (
                    "upload",
                    expected_label(expected),
                    content_hash(content).to_string(),
                    changes,
                    current != Some(&new),
                )
            }
            PreviewChange::Delete { expected } => (
                "delete",
                expected.to_string(),
                "none".to_owned(),
                preview_changes(current.map_or(&[], |b| &b[..]), &[]),
                // A file that is not there cannot change anything by going away.
                current.is_some(),
            ),
        };
        let id = preview_token(&ctx.user.id, &r.swimlane, &r.path, action, &expected, &new_hash)
            .ok_or(WriteError::Unavailable)?;
        let expires_ms = st
            .now_ms
            .saturating_add(i64::try_from(ChangePreview::TTL_SECS * 1000).unwrap_or(0));
        if st.previews.len() >= MAX_PREVIEWS {
            st.previews.clear();
        }
        st.previews.insert(id.clone(), expires_ms);
        let changes_total = u32::try_from(changes.len()).unwrap_or(u32::MAX);
        Ok(ChangePreview {
            preview_id: id,
            expires_at: Timestamp::from_unix_millis(expires_ms),
            // The fake computes no facts: the real service takes these from the engine.
            scope: PreviewScope::EveryService,
            warnings: Vec::new(),
            changes: changes.into_iter().take(MAX_PREVIEW_CHANGES).collect(),
            changes_total,
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

    async fn advance_clock(&self, secs: u64) {
        Self::advance_clock(self, secs);
    }
}
