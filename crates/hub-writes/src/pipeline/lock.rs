//! Step 3: the editing lock (D2). A swimlane's files are read-only until an admin confirms its baseline in the adoption pass.

use domain::SwimlaneId;

use crate::error::{Internal, Res};
use crate::service::Inner;

pub(crate) enum LockState {
    Open,
    Locked,
    UnknownSwimlane,
}

pub(crate) const SWIMLANE_LOCK: &str = "SELECT editing_unlocked FROM swimlanes WHERE id = $1";

pub(crate) async fn check(hub: &Inner, swimlane: &SwimlaneId) -> Res<LockState> {
    let row: Option<bool> = hub
        .within(
            "lock",
            sqlx::query_scalar(SWIMLANE_LOCK)
                .bind(swimlane.as_str())
                .fetch_optional(&hub.pool),
        )
        .await?
        .map_err(|e| Internal::db("lock", &e))?;
    Ok(match row {
        None => LockState::UnknownSwimlane,
        Some(false) => LockState::Locked,
        Some(true) => LockState::Open,
    })
}
