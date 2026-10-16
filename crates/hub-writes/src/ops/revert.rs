//! `revert`: write a stored version of a file back (T3). The version has to be one this file had: a hash of some other file's
//! content is refused, or a revert would be a way to copy a file the caller may not read (D79) into one they may edit. The
//! current hash is the precondition, like any write.

use domain::{Page, RejectReason, RevertRequest, Role, User, WriteCtx, WriteOutcome};
use ports::{BlobError, ReadError, WriteError};

use super::file::{Kind, Write, finish, load_old, rejected, screen};
use crate::fidelity::prepare_revert;
use crate::idempotency::RequestHash;
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::{Abort, Done, Step};
use crate::service::Inner;

/// The most pages of history looked through for the version: 2,000 versions. A file with more is not one a revert reaches back
/// through.
const MAX_HISTORY_PAGES: usize = 4;

pub(crate) async fn revert(
    hub: &Inner,
    ctx: &WriteCtx,
    r: RevertRequest,
) -> Result<WriteOutcome, WriteError> {
    let user = authorize(hub, ctx, Role::Editor).await?;
    let claim = match claim(hub, ctx, RequestHash::of_revert(&r)).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = run(hub, ctx, &user, &claim, r).await;
    settle(hub, &claim, result).await
}

/// Whether `to` is one of the versions the registry lists for the file.
async fn is_a_version(hub: &Inner, user: &User, r: &RevertRequest) -> Step<bool> {
    let mut cursor = None;
    for _ in 0..MAX_HISTORY_PAGES {
        let page = Page::new(Page::MAX_LIMIT, cursor).map_err(|_| Abort::release(WriteError::Unavailable))?;
        let listed = hub
            .within(
                "revert_history",
                hub.registry.history(user, &r.swimlane, &r.path, page),
            )
            .await?;
        let listed = match listed {
            Ok(listed) => listed,
            // A file with no history has no version to go back to.
            Err(ReadError::NotFound) => return Ok(false),
            Err(_) => return Err(Abort::release(WriteError::Unavailable)),
        };
        if listed.items.iter().any(|v| v.hash == r.to) {
            return Ok(true);
        }
        match listed.next {
            Some(next) => cursor = Some(next),
            None => return Ok(false),
        }
    }
    Ok(false)
}

async fn run(hub: &Inner, ctx: &WriteCtx, user: &User, claim: &Claim, r: RevertRequest) -> Step<Done> {
    if let Some(done) = screen(hub, &r.swimlane, &r.path, 0).await? {
        return Ok(done);
    }
    if !is_a_version(hub, user, &r).await? {
        return Ok(Done::Store(rejected(RejectReason::InvalidContent)));
    }
    let stored = match hub.within("revert_blob", hub.blobs.get(&r.to)).await? {
        Ok(Some(bytes)) => bytes,
        // The registry lists it and the store does not have it: the version was collected (D78).
        Ok(None) => return Ok(Done::Store(rejected(RejectReason::InvalidContent))),
        Err(BlobError::Unavailable | BlobError::Full | BlobError::TooLarge | BlobError::TooMany) => {
            return Err(Abort::release(WriteError::Unavailable));
        }
    };
    if stored.len() > hub.cfg.max_content_bytes {
        return Ok(Done::Store(rejected(RejectReason::TooLarge)));
    }

    let old = match load_old(hub, user, &r.swimlane, &r.path, &r.expected).await? {
        Ok(old) => old,
        Err(done) => return Ok(done),
    };
    let prepared = hub
        .blocking("validate", {
            let (path, deny, limits) = (r.path.clone(), hub.deny.clone(), hub.limits.clone());
            let (change_eol, max_bytes) = (r.change_eol, hub.cfg.max_content_bytes);
            move || prepare_revert(&path, &stored, change_eol, max_bytes, &old, &deny, &limits)
        })
        .await?;
    let (prepared, checked) = match prepared {
        Ok(ok) => ok,
        Err(reason) => return Ok(Done::Store(rejected(reason))),
    };
    finish(
        hub,
        Write {
            kind: Kind::Revert,
            ctx,
            user,
            claim,
            swimlane: &r.swimlane,
            path: &r.path,
            expected: &r.expected,
            restores: Some(r.to),
            draft: None,
        },
        prepared,
        checked,
    )
    .await
}
