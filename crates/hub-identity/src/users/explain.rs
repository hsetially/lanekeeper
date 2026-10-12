//! The query plans of the lists, for the EXPLAIN tests (P12). Each function runs `EXPLAIN (FORMAT JSON)` on exactly the
//! statement the list runs, with the same binds, so a test that checks the plan checks the real query.
//!
//! Nothing in the hub calls these. They need the same database role as the lists and write nothing.

use sqlx::PgPool;

use super::admin::{request_list_builder, user_list_builder};
use super::{AccessError, RequestQuery, UserQuery, bounded, db_err, page_size};

const EXPLAIN: &str = "EXPLAIN (FORMAT JSON) ";

async fn plan(
    op: &'static str,
    pool: &PgPool,
    mut builder: sqlx::QueryBuilder<'static, sqlx::Postgres>,
) -> Result<serde_json::Value, AccessError> {
    bounded(op, async {
        let row: (serde_json::Value,) = builder
            .build_query_as()
            .fetch_one(pool)
            .await
            .map_err(|e| db_err(op, &e))?;
        Ok(row.0)
    })
    .await
}

/// The plan of `UserAdmin::list_users(q)`.
pub async fn list_users(pool: &PgPool, q: &UserQuery) -> Result<serde_json::Value, AccessError> {
    plan(
        "explain users",
        pool,
        user_list_builder(EXPLAIN, q, page_size(q.limit))?,
    )
    .await
}

/// The plan of `UserAdmin::list_requests(q)`.
pub async fn list_requests(pool: &PgPool, q: &RequestQuery) -> Result<serde_json::Value, AccessError> {
    plan(
        "explain requests",
        pool,
        request_list_builder(EXPLAIN, q, page_size(q.limit))?,
    )
    .await
}
