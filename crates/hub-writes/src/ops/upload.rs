//! `upload`: put a file, possibly binary, on NFS (T3, D46). It is an `edit` that the person confirmed: it needs the token that
//! `preview_effect` issued for this user, swimlane, path, expected state and file, less than ten minutes ago.

use domain::{Role, UploadRequest, WriteCtx, WriteOutcome};
use ports::WriteError;

use super::file::{Kind, Write, finish, load_old, prepare, rejected, screen};
use crate::idempotency::RequestHash;
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::{Done, Step};
use crate::preview::token::{Binding, PreviewAction};
use crate::service::Inner;

pub(crate) async fn upload(
    hub: &Inner,
    ctx: &WriteCtx,
    r: UploadRequest,
) -> Result<WriteOutcome, WriteError> {
    let user = authorize(hub, ctx, Role::Editor).await?;
    let claim = match claim(hub, ctx, RequestHash::of_upload(&r)).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = run(hub, ctx, &user, &claim, r).await;
    settle(hub, &claim, result).await
}

async fn run(
    hub: &Inner,
    ctx: &WriteCtx,
    user: &domain::User,
    claim: &Claim,
    r: UploadRequest,
) -> Step<Done> {
    if let Some(done) = screen(hub, &r.swimlane, &r.path, r.content.len()).await? {
        return Ok(done);
    }
    // The confirmation. A refusal is not an outcome of the request (a retry with a fresh token is the same request), so the key
    // is freed.
    let content_hash = ports::content_hash(&r.content);
    let binding = Binding {
        user: &user.id,
        swimlane: &r.swimlane,
        path: &r.path,
        action: PreviewAction::Upload,
        expected: &r.expected,
        content: Some(&content_hash),
    };
    if !hub.preview_ok(&binding, r.preview.as_ref()).await? {
        return Ok(Done::Release(rejected(domain::RejectReason::NotAllowed)));
    }

    let old = match load_old(hub, user, &r.swimlane, &r.path, &r.expected).await? {
        Ok(old) => old,
        Err(done) => return Ok(done),
    };
    // An upload keeps the form of the file it replaces like an edit does; it never converts line endings on its own (D17).
    let (prepared, checked) = match prepare(hub, user, &r.swimlane, &r.path, &r.content, None, old).await? {
        Ok(ok) => ok,
        Err(done) => return Ok(done),
    };
    finish(
        hub,
        Write {
            kind: Kind::Upload,
            ctx,
            user,
            claim,
            swimlane: &r.swimlane,
            path: &r.path,
            expected: &r.expected,
            restores: None,
            draft: None,
        },
        prepared,
        checked,
    )
    .await
}
