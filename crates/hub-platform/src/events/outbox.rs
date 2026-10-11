//! The `outbox` table (D80): the one place events are written and read.
//!
//! An event is a row, written with a plain `INSERT` on the caller's connection, so it commits or rolls back with the change that
//! caused it. The table's trigger sends `NOTIFY lanekeeper_outbox` when the inserting transaction commits and not before, so a
//! rolled-back change tells nobody anything. The row's `id` is the event's id for the whole cluster.
//!
//! Every read is bounded by a `LIMIT` or by an id list the caller bounded, and every call has a timeout (rule 5).

use std::time::Duration;

use domain::DomainEvent;
use ports::BusError;
use sqlx::{PgConnection, PgPool};

use super::wire::{self, EnvelopeError, EventId};

/// One database call.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// The outbox could not be read or written. Carries no SQL and no event content: the cause goes to the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    #[error("the outbox is unavailable")]
    Unavailable,
}

/// A stored row: its id and its envelope as JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: EventId,
    pub text: String,
}

async fn within<T>(
    call: impl std::future::Future<Output = Result<T, sqlx::Error>>,
) -> Result<T, OutboxError> {
    match tokio::time::timeout(QUERY_TIMEOUT, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) | Err(_) => Err(OutboxError::Unavailable),
    }
}

fn row(id: i64, text: String) -> Result<Row, OutboxError> {
    // An identity column never gives zero or a negative id.
    EventId::new(id)
        .map(|id| Row { id, text })
        .ok_or(OutboxError::Unavailable)
}

/// The envelope to store for `event`, and whether it had to be replaced: an event over `max` bytes is stored as a
/// [`DomainEvent::Resync`], which makes every subscriber refetch instead of reading a payload that was too big to keep.
pub(crate) fn prepare(event: &DomainEvent, max: usize) -> Result<(String, bool), BusError> {
    match wire::encode(event, max) {
        Ok(text) => Ok((text, false)),
        Err(EnvelopeError::TooLarge) => wire::encode(&DomainEvent::Resync, max)
            .map(|text| (text, true))
            .map_err(|_| BusError::Unavailable),
        Err(_) => Err(BusError::Unavailable),
    }
}

/// Insert one envelope on `conn`. The row is visible, and the notification sent, when the connection's transaction commits.
pub(crate) async fn insert(conn: &mut PgConnection, text: &str) -> Result<EventId, OutboxError> {
    // `text` then `jsonb`: the envelope was checked by `prepare`, and Postgres checks that it is JSON.
    let id: i64 = within(
        sqlx::query_scalar("INSERT INTO outbox (event) VALUES ($1::text::jsonb) RETURNING id")
            .bind(text)
            .fetch_one(conn),
    )
    .await?;
    EventId::new(id).ok_or(OutboxError::Unavailable)
}

/// Insert one envelope in its own statement (and so its own transaction).
pub(crate) async fn insert_autocommit(pool: &PgPool, text: &str) -> Result<EventId, OutboxError> {
    let id: i64 = within(
        sqlx::query_scalar("INSERT INTO outbox (event) VALUES ($1::text::jsonb) RETURNING id")
            .bind(text)
            .fetch_one(pool),
    )
    .await?;
    EventId::new(id).ok_or(OutboxError::Unavailable)
}

/// The id of the newest row, or 0 for an empty table.
pub(crate) async fn newest_id(pool: &PgPool) -> Result<i64, OutboxError> {
    within(sqlx::query_scalar("SELECT COALESCE(max(id), 0)::bigint FROM outbox").fetch_one(pool)).await
}

/// At most `limit` rows with an id above `after`, ascending.
pub(crate) async fn read_after(pool: &PgPool, after: i64, limit: i64) -> Result<Vec<Row>, OutboxError> {
    let rows: Vec<(i64, String)> = within(
        sqlx::query_as("SELECT id, event::text FROM outbox WHERE id > $1 ORDER BY id LIMIT $2")
            .bind(after)
            .bind(limit)
            .fetch_all(pool),
    )
    .await?;
    rows.into_iter().map(|(id, text)| row(id, text)).collect()
}

/// The rows with these ids, ascending. The caller bounds the list.
pub(crate) async fn read_ids(pool: &PgPool, ids: &[i64]) -> Result<Vec<Row>, OutboxError> {
    let rows: Vec<(i64, String)> = within(
        sqlx::query_as("SELECT id, event::text FROM outbox WHERE id = ANY($1) ORDER BY id")
            .bind(ids)
            .fetch_all(pool),
    )
    .await?;
    rows.into_iter().map(|(id, text)| row(id, text)).collect()
}

/// Whether the row `id` exists and is younger than `window`: `None` if there is no such row (never written, or pruned).
pub(crate) async fn within_window(
    pool: &PgPool,
    id: EventId,
    window: Duration,
) -> Result<Option<bool>, OutboxError> {
    within(
        sqlx::query_scalar(
            "SELECT created_at > now() - make_interval(secs => $2::double precision) FROM outbox WHERE id = $1",
        )
        .bind(id.get())
        .bind(window.as_secs_f64())
        .fetch_optional(pool),
    )
    .await
}
