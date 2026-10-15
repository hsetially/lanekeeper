//! `edit`: replace or create a file (T1, T2). The reference executor of the pipeline; `upload` and `revert` share its tail
//! (`super::file`).

use domain::{EditRequest, Role, WriteCtx, WriteOutcome};
use ports::WriteError;

use super::file::{Kind, Write, finish, load_old, prepare, screen};
use crate::idempotency::RequestHash;
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::{Done, Step};
use crate::service::Inner;

pub(crate) async fn edit(hub: &Inner, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError> {
    let user = authorize(hub, ctx, Role::Editor).await?;
    let claim = match claim(hub, ctx, RequestHash::of_edit(&r)).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = run(hub, ctx, &user, &claim, r, None).await;
    settle(hub, &claim, result).await
}

/// Steps 3 to 6 for an edit. `draft` is the draft the edit applies, when it applies one (`apply_draft`): the audit event says so,
/// and the draft is marked with the write.
pub(crate) async fn run(
    hub: &Inner,
    ctx: &WriteCtx,
    user: &domain::User,
    claim: &Claim,
    r: EditRequest,
    draft: Option<&str>,
) -> Step<Done> {
    // 3 and the cheap part of 4: the lock, the size, the deny globs.
    if let Some(done) = screen(hub, &r.swimlane, &r.path, r.content.len()).await? {
        return Ok(done);
    }
    // 4. The content is put into the form of the file it replaces and checked, on the blocking pool.
    let old = match load_old(hub, user, &r.swimlane, &r.path, &r.expected).await? {
        Ok(old) => old,
        Err(done) => return Ok(done),
    };
    let (prepared, checked) =
        match prepare(hub, user, &r.swimlane, &r.path, &r.content, r.change_eol, old).await? {
            Ok(ok) => ok,
            Err(done) => return Ok(done),
        };
    // 5 and 6.
    finish(
        hub,
        Write {
            kind: if draft.is_some() { Kind::Draft } else { Kind::Edit },
            ctx,
            user,
            claim,
            swimlane: &r.swimlane,
            path: &r.path,
            expected: &r.expected,
            restores: None,
            draft,
        },
        prepared,
        checked,
    )
    .await
}
