//! The list of proposals (`GET /proposals`): a page, newest first (keyset on the id), each with the diff an approver reads.
//!
//! The work of a page is bounded three ways. At most [`MAX_PAGE`] proposals are read, whatever limit the caller asks for (the
//! page has a `next` cursor, so the caller only pages more often). The versions a page reads from the blob store add up to at
//! most [`MAX_PAGE_BYTES`]: a page that would read more ends early and says where it stopped. And every diff is made on the
//! blocking pool, in one hop for the page.

use std::collections::HashMap;

use bytes::Bytes;
use domain::{ContentHash, Expected, NfsPath, Paged, Role, ShortText, Timestamp, User};
use ports::WriteError;

use super::store::{self, Base, Filter, Stored};
use super::view::{DiffView, diff_view};
use super::{NO_HASH, ProposalQuery, ProposalView, cursor_below};
use crate::error::Internal;
use crate::pipeline::authorize::authorize_user;
use crate::pipeline::validate::Old;
use crate::service::Inner;

/// The most proposals one page holds.
pub const MAX_PAGE: u32 = 25;

/// The most bytes of file versions one page reads to make its diffs: 16 MiB.
pub const MAX_PAGE_BYTES: usize = 16 * 1024 * 1024;

pub(crate) async fn list(
    hub: &Inner,
    user: &User,
    q: ProposalQuery,
) -> Result<Paged<ProposalView>, WriteError> {
    authorize_user(hub, &user.id, Role::Viewer).await?;
    let below = match q.page.cursor() {
        None => None,
        Some(c) => Some(c.as_str().parse::<i64>().map_err(|_| WriteError::BadRequest)?),
    };
    let limit = q.page.limit().min(MAX_PAGE);
    let filter = Filter {
        below,
        status: q.status,
        swimlane: q.swimlane.as_ref(),
    };
    let mut conn = hub
        .within("proposal_list", hub.pool.acquire())
        .await?
        .map_err(|e| Internal::db("proposal_list", &e))?;
    // One more than the page, to know whether there is a next one.
    let mut rows = hub
        .within(
            "proposal_list",
            store::list(&mut conn, &filter, i64::from(limit) + 1),
        )
        .await??;
    drop(conn);
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);

    let (views, stopped_early) = views_of(hub, rows).await?;
    let next = match (views.last(), more || stopped_early) {
        (Some(last), true) => cursor_below(last.id),
        _ => None,
    };
    Ok(Paged { items: views, next })
}

/// The versions behind one proposal's diff.
struct Work {
    path: NfsPath,
    old: Old,
    new: Bytes,
}

/// The view of each stored proposal, in order, and whether the byte budget ended the page before its last proposal.
pub(crate) async fn views_of(hub: &Inner, rows: Vec<Stored>) -> Result<(Vec<ProposalView>, bool), Internal> {
    let mut included: Vec<(Stored, Option<usize>)> = Vec::with_capacity(rows.len());
    let mut works: Vec<Work> = Vec::new();
    let mut spent = 0_usize;
    let mut stopped = false;
    // The sizes of the versions, read from the store's own record before any version is: the budget is kept by not reading the
    // proposal that would pass it, not by reading it and throwing it away.
    let sizes = sizes_of(hub, &rows).await;
    for p in rows {
        let has_diff = matches!(p.action.as_str(), "edit" | "upload" | "revert");
        if has_diff && !included.is_empty() && spent.saturating_add(planned_cost(&sizes, &p)) > MAX_PAGE_BYTES
        {
            stopped = true;
            break;
        }
        let work = if has_diff { fetch(hub, &p).await } else { None };
        if let Some(w) = &work {
            let cost = w.new.len()
                + match &w.old {
                    Old::Known(b) => b.len(),
                    _ => 0,
                };
            // The first proposal of a page is always shown, so a page is never empty because of its own size.
            if !included.is_empty() && spent.saturating_add(cost) > MAX_PAGE_BYTES {
                stopped = true;
                break;
            }
            spent = spent.saturating_add(cost);
        }
        let at = work.map(|w| {
            works.push(w);
            works.len() - 1
        });
        included.push((p, at));
    }

    let views: Vec<DiffView> = if works.is_empty() {
        Vec::new()
    } else {
        let (deny, limits) = (hub.deny.clone(), hub.limits.clone());
        hub.blocking("proposal_diffs", move || {
            works
                .iter()
                .map(|w| diff_view(&w.path, &w.old, &w.new, &deny, &limits))
                .collect()
        })
        .await?
    };

    let out = included
        .into_iter()
        .filter_map(|(p, at)| {
            let diff = at.and_then(|i| views.get(i)).cloned().unwrap_or_default();
            view_of(&p, diff)
        })
        .collect();
    Ok((out, stopped))
}

/// The recorded size of each of the versions `$1`, read through the primary key of `blobs`.
pub(crate) const SIZES_SQL: &str = "SELECT hash, size FROM blobs WHERE hash = ANY($1::bytea[])";

/// What the blob store records of the size of each version that the diffs of `rows` need. A version it does not list is left out
/// (a blob that was collected), and so is every version when the store cannot be asked: the budget is then kept on the sizes of
/// what is read, as before.
async fn sizes_of(hub: &Inner, rows: &[Stored]) -> HashMap<ContentHash, usize> {
    let mut wanted: Vec<Vec<u8>> = Vec::new();
    for p in rows
        .iter()
        .filter(|p| matches!(p.action.as_str(), "edit" | "upload" | "revert"))
    {
        if let Some(h) = p.new_hash {
            wanted.push(h.as_bytes().to_vec());
        }
        if let Some(Base::At(h)) = p.paths.first().and_then(|path| p.expected_at(path)) {
            wanted.push(h.as_bytes().to_vec());
        }
    }
    if wanted.is_empty() {
        return HashMap::new();
    }
    let asked = async {
        sqlx::query_as::<_, (Vec<u8>, i64)>(SIZES_SQL)
            .bind(&wanted)
            .fetch_all(&hub.pool)
            .await
            .map_err(|e| Internal::db("proposal_sizes", &e))
    };
    let Ok(Ok(found)) = hub.within("proposal_sizes", asked).await else {
        return HashMap::new();
    };
    found
        .into_iter()
        .filter_map(|(hash, size)| {
            let hash = <[u8; 32]>::try_from(hash.as_slice()).ok()?;
            Some((ContentHash::from_bytes(hash), usize::try_from(size).ok()?))
        })
        .collect()
}

/// The bytes that making the diff of `p` reads: its new version and the one it replaces.
fn planned_cost(sizes: &HashMap<ContentHash, usize>, p: &Stored) -> usize {
    let new = p.new_hash.and_then(|h| sizes.get(&h)).copied().unwrap_or(0);
    let old = match p.paths.first().and_then(|path| p.expected_at(path)) {
        Some(Base::At(h)) => sizes.get(&h).copied().unwrap_or(0),
        _ => 0,
    };
    new.saturating_add(old)
}

/// The new version and the one it replaces, read from the blob store. `None` when the proposal has nothing to diff.
async fn fetch(hub: &Inner, p: &Stored) -> Option<Work> {
    let path_text = p.paths.first()?;
    let path = NfsPath::parse(path_text).ok()?;
    let new_hash = p.new_hash?;
    let Ok(Ok(Some(new))) = hub.within("proposal_new", hub.blobs.get(&new_hash)).await else {
        return None;
    };
    let old = match p.expected_at(path_text)? {
        Base::Absent => Old::Absent,
        Base::At(hash) => match hub.within("proposal_old", hub.blobs.get(&hash)).await {
            Ok(Ok(Some(b))) => Old::Known(b),
            _ => Old::Unknown,
        },
    };
    Some(Work { path, old, new })
}

/// A proposal as the API shows it. `None` for a row that cannot be described (a path that is not a path).
pub(crate) fn view_of(p: &Stored, diff: DiffView) -> Option<ProposalView> {
    let path_text = p.paths.first()?;
    let expected = match p.expected_at(path_text) {
        Some(Base::At(hash)) => Expected::Hash { hash },
        _ => Expected::Absent,
    };
    Some(ProposalView {
        id: p.id.get(),
        author: p.author.clone(),
        swimlane: p.swimlane.clone(),
        path: NfsPath::parse(path_text).ok()?,
        expected,
        new_hash: p.new_hash.unwrap_or(NO_HASH),
        status: p.status,
        hunks: diff.hunks,
        setting_changes: diff.setting_changes,
        severity: None,
        created_at: Timestamp::from_unix_millis(p.created_ms),
        expires_at: Timestamp::from_unix_millis(p.expires_ms),
        decided_by: p.decision.as_ref().map(|d| d.approver.clone()),
        decision_comment: p
            .decision
            .as_ref()
            .and_then(|d| d.comment.as_deref())
            .and_then(|c| ShortText::parse(c).ok()),
    })
}

/// The view of one proposal, with its diff.
pub(crate) async fn one(hub: &Inner, p: Stored) -> Result<Option<ProposalView>, Internal> {
    let (mut views, _) = views_of(hub, vec![p]).await?;
    Ok(views.pop())
}
