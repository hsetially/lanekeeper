//! The statements a test ran, counted from sqlx's own statement log (its `sqlx::query` events), for the tests that prove how many
//! round trips, inserts and transactions a code path takes. A decorator that forwarded only the single-event calls of a port would
//! still pass every test of what is written; only counting the statements tells the batch from a loop.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::log_capture::LogCapture;

/// One statement that sqlx logged: its text and the rows it touched or returned.
pub struct Statement {
    pub text: String,
    pub rows: u64,
}

fn number_after(line: &str, key: &str) -> u64 {
    line.split_once(key)
        .map(|(_, rest)| rest.chars().take_while(char::is_ascii_digit).collect::<String>())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0)
}

/// The statements sqlx logged on this thread (its `sqlx::query` events), which is every statement of a test on the current-thread
/// runtime that `#[tokio::test]` makes.
pub fn statements(capture: &LogCapture) -> Vec<Statement> {
    capture
        .text()
        .lines()
        .filter(|l| l.contains(" sqlx::query "))
        .map(|l| Statement {
            text: l.to_owned(),
            rows: number_after(l, "rows_affected=").max(number_after(l, "rows_returned=")),
        })
        .collect()
}

/// The statements that start with `what` (after sqlx's summary of four words), with their row counts.
pub fn inserts(all: &[Statement], what: &str) -> Vec<u64> {
    all.iter()
        .filter(|s| s.text.contains(&format!("summary=\"{what}")))
        .map(|s| s.rows)
        .collect()
}
