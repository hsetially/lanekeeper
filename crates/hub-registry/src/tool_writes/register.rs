//! `register` and `register_delete` (the port's methods), in the caller's transaction and inside a savepoint of their own.
//!
//! The sequence, all of it on the connection of the caller's transaction:
//!
//! 1. `SAVEPOINT lanekeeper_tool_write`. If it cannot be made (no transaction is open, or the transaction is already aborted) the answer
//!    is `Unavailable` and nothing was changed.
//! 2. Read the caller's `lock_timeout` and `statement_timeout`, and set both to `tool_write_timeout` for the next statement only
//!    (`set_config(.., true)`, which is `SET LOCAL`).
//! 3. One statement: is the key held (a repeat is `Ok`, with no count), how many registrations younger than the TTL are there (up to the
//!    bound), and, if the key is new and there is room, the insert (`ON CONFLICT ON CONSTRAINT tool_write_registrations_key DO NOTHING`).
//! 4. Put the caller's two timeouts back. A `SET LOCAL` made inside a savepoint is reverted by a rollback to it and **survives its
//!    `RELEASE`**, into the caller's next statement (300 ms were measured leaking into an audit insert), so the restore comes before
//!    the release.
//! 5. `RELEASE SAVEPOINT`. Any failure of 2 to 4 is `ROLLBACK TO SAVEPOINT` (which reverts the `SET LOCAL` too) and then `RELEASE`, and
//!    the answer is `Unavailable`: the caller's transaction is usable and records the operation's own audit event.
//!
//! Every wait is bounded: on the server by the two timeouts of step 2, on the client by `tool_write_timeout` plus a grace for each call,
//! so a connection that stops answering ends the registration and does not hold the caller's request for ever (rule 5). Nothing of the
//! path, the hash or the user is in a log line (S10).

use std::time::Duration;

use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};
use ports::{ToolWriteError, Tx};
use sqlx::PgConnection;

use super::{Inner, TTL_MS};
use crate::db::run_within;
use crate::error::RegistryError;
use crate::metrics::ToolWriteResult;

/// The savepoint and its two ends. The name is not sqlx's (`_sqlx_savepoint_N`), so a caller that opens one of its own never meets it.
const SAVEPOINT_SQL: &str = "SAVEPOINT lanekeeper_tool_write";
const RELEASE_SQL: &str = "RELEASE SAVEPOINT lanekeeper_tool_write";
const ROLLBACK_SQL: &str = "ROLLBACK TO SAVEPOINT lanekeeper_tool_write";

/// How much longer than the server-side timeouts the client waits for an answer before it gives the registration up.
const GRACE: Duration = Duration::from_secs(2);

/// Register one hash, or one deleted hash, of a path: `$1` swimlane, `$2` path, `$3` hash, `$4` request id, `$5` `is_delete`, `$6` the
/// cutoff (Unix ms: a registration stamped after it is younger than the TTL), `$7` the bound, `$8` and `$9` the user's tenant and object
/// id, `$10` the stamp (Unix ms). Answers `(held, pending, inserted)`:
///
/// - `held`: the key was already there (a repeat), so no count was made and nothing was inserted;
/// - `pending`: the registrations younger than the TTL, counted up to the bound (the age index serves it);
/// - `inserted`: the row was written. Neither `held` nor `inserted` with `pending` below the bound means that the key was written by a
///   transaction that committed since the statement's snapshot, which is a repeat too; with `pending` at the bound it is `Full`.
pub const REGISTER_SQL: &str = "WITH held AS ( \
       SELECT 1 AS one FROM tool_write_registrations \
       WHERE swimlane_id = $1 AND path = $2 AND hash = $3::bytea AND request_id = $4 AND is_delete = $5 \
     ), pending AS ( \
       SELECT count(*) AS n FROM ( \
         SELECT 1 FROM tool_write_registrations \
         WHERE registered_at > (timestamptz 'epoch' + ($6::bigint::text || ' milliseconds')::interval) LIMIT $7 \
       ) AS younger \
     ), inserted AS ( \
       INSERT INTO tool_write_registrations \
         (swimlane_id, path, hash, is_delete, user_tid, user_oid, request_id, registered_at) \
       SELECT $1::text, $2::text, $3::bytea, $5::boolean, $8::text, $9::text, $4::text, \
              (timestamptz 'epoch' + ($10::bigint::text || ' milliseconds')::interval) \
       WHERE NOT EXISTS (SELECT 1 FROM held) AND (SELECT n FROM pending) < $7 \
       ON CONFLICT ON CONSTRAINT tool_write_registrations_key DO NOTHING \
       RETURNING 1 AS one \
     ) \
     SELECT EXISTS (SELECT 1 FROM held), (SELECT n FROM pending), EXISTS (SELECT 1 FROM inserted) LIMIT 1";

/// What the statement said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Registered,
    Repeated,
    Full,
}

impl Inner {
    #[allow(clippy::too_many_arguments)] // the port's own arguments, and the kind
    pub(super) async fn register_one(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        hash: &ContentHash,
        is_delete: bool,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        let Ok(conn) = tx.pg() else {
            self.metrics.tool_write(ToolWriteResult::WrongTx, 1);
            return Err(ToolWriteError::WrongTx);
        };
        // The registry's own clock, and nothing else: the port takes no time, so there is nothing to clamp, and a registration cannot be
        // dated into the future.
        let stamp = self.clock.now().unix_millis();
        let call = Call {
            swimlane,
            path,
            hash,
            is_delete,
            user,
            request_id,
            stamp,
        };
        let outcome = self.in_savepoint(conn, &call).await;
        match outcome {
            Ok(Outcome::Registered) => {
                self.metrics.tool_write(ToolWriteResult::Registered, 1);
                Ok(())
            }
            Ok(Outcome::Repeated) => {
                self.metrics.tool_write(ToolWriteResult::Repeated, 1);
                Ok(())
            }
            Ok(Outcome::Full) => {
                tracing::warn!(why = "queue_full", "tool write registration refused");
                self.metrics.tool_write(ToolWriteResult::Full, 1);
                Err(ToolWriteError::Full)
            }
            Err(_) => {
                self.metrics.tool_write(ToolWriteResult::Unavailable, 1);
                Err(ToolWriteError::Unavailable)
            }
        }
    }

    async fn in_savepoint(&self, conn: &mut PgConnection, call: &Call<'_>) -> Result<Outcome, RegistryError> {
        let wait = self.config.tool_write_timeout + GRACE;
        run_within(
            wait,
            "tool_write_savepoint",
            sqlx::query(SAVEPOINT_SQL).execute(&mut *conn),
        )
        .await?;
        match self.statements(conn, call).await {
            Ok(outcome) => {
                run_within(
                    wait,
                    "tool_write_release",
                    sqlx::query(RELEASE_SQL).execute(&mut *conn),
                )
                .await?;
                Ok(outcome)
            }
            Err(error) => {
                // Back to the savepoint, so that the caller's transaction is not left aborted, and release it. If this fails too the
                // connection is gone, which no savepoint covers.
                let _ = run_within(
                    wait,
                    "tool_write_rollback",
                    sqlx::query(ROLLBACK_SQL).execute(&mut *conn),
                )
                .await;
                let _ = run_within(
                    wait,
                    "tool_write_release",
                    sqlx::query(RELEASE_SQL).execute(&mut *conn),
                )
                .await;
                Err(error)
            }
        }
    }

    /// Steps 2 to 4, inside the savepoint.
    async fn statements(&self, conn: &mut PgConnection, call: &Call<'_>) -> Result<Outcome, RegistryError> {
        let timeout = self.config.tool_write_timeout;
        let wait = timeout + GRACE;
        let (lock, statement): (String, String) = run_within(
            wait,
            "tool_write_settings",
            sqlx::query_as(
                "SELECT current_setting('lock_timeout'), current_setting('statement_timeout') LIMIT 1",
            )
            .fetch_one(&mut *conn),
        )
        .await?;
        let bound = i64::try_from(self.config.tool_write_max_pending).unwrap_or(i64::MAX);
        let millis = format!("{}ms", timeout.as_millis());
        set_timeouts(conn, wait, &millis, &millis).await?;

        let (held, pending, inserted): (bool, i64, bool) = run_within(
            wait,
            "tool_write_register",
            sqlx::query_as(REGISTER_SQL)
                .bind(call.swimlane.as_str())
                .bind(call.path.as_str())
                .bind(call.hash.as_bytes().as_slice())
                .bind(call.request_id.as_str())
                .bind(call.is_delete)
                .bind(call.stamp.saturating_sub(TTL_MS))
                .bind(bound)
                .bind(call.user.tid().as_str())
                .bind(call.user.oid().as_str())
                .bind(call.stamp)
                .fetch_one(&mut *conn),
        )
        .await?;

        set_timeouts(conn, wait, &lock, &statement).await?;
        Ok(if held {
            Outcome::Repeated
        } else if inserted {
            Outcome::Registered
        } else if pending >= bound {
            Outcome::Full
        } else {
            // The key was written by a transaction that committed after this statement's snapshot: a repeat.
            Outcome::Repeated
        })
    }
}

/// What one registration is made of.
struct Call<'a> {
    swimlane: &'a SwimlaneId,
    path: &'a NfsPath,
    hash: &'a ContentHash,
    is_delete: bool,
    user: &'a UserId,
    request_id: &'a RequestId,
    stamp: i64,
}

/// `SET LOCAL lock_timeout` and `SET LOCAL statement_timeout`, as one statement. The values are text of the server's own format (what
/// `current_setting` gave, or milliseconds), bound and never formatted into the SQL.
async fn set_timeouts(
    conn: &mut PgConnection,
    wait: Duration,
    lock: &str,
    statement: &str,
) -> Result<(), RegistryError> {
    run_within(
        wait,
        "tool_write_settings",
        sqlx::query(
            "SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true) LIMIT 1",
        )
        .bind(lock)
        .bind(statement)
        .execute(&mut *conn),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ts;

    #[test]
    fn the_statement_uses_the_exact_time_expressions_of_the_db_helpers() {
        assert!(REGISTER_SQL.contains(&ts("$6")));
        assert!(REGISTER_SQL.contains(&ts("$10")));
    }

    #[test]
    fn the_bound_is_checked_before_the_insert_and_a_repeat_skips_it() {
        let held = REGISTER_SQL
            .find("NOT EXISTS (SELECT 1 FROM held)")
            .unwrap_or(usize::MAX);
        let room = REGISTER_SQL
            .find("(SELECT n FROM pending) < $7")
            .unwrap_or(usize::MAX);
        let conflict = REGISTER_SQL
            .find("ON CONFLICT ON CONSTRAINT tool_write_registrations_key DO NOTHING")
            .unwrap_or(usize::MAX);
        assert!(held < room && room < conflict, "the insert is guarded by both");
        assert!(
            REGISTER_SQL.contains("LIMIT $7"),
            "the count stops at the bound: it is not a count of the table"
        );
    }
}
