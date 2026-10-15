//! Holding drift alerts while a sync window is open (T8, D75).
//!
//! A sync Job rewrites hundreds of files, so for a while NFS differs from Git and from the baseline in ways nobody should be told about
//! one file at a time. D75: "the hub holds drift alerts until the Job finishes, then evaluates once". Drift is **recorded** all the
//! while (the pipeline does not know about windows); what is held is the notification. [`Gate::state`] is the one question the live
//! path asks: is an alert for this swimlane held right now?
//!
//! # What holds
//!
//! A window holds while it has not been **processed** ([`super::window`]): from its opening, through the close, until the hub has
//! recorded the sync and evaluated the alerts for it. The close is not the end of the hold on purpose: the closing scan is applied
//! after the close is reported, and the drift it produces before the pipeline settles must not slip out as alerts of their own.
//!
//! Two bounds keep a hold from silencing a swimlane for good (rule 5; a window is reported by the cluster, and a Job that never ends or a
//! window nobody should have reported would otherwise switch drift alerts off):
//!
//! | Window | Holds while | Then |
//! |---|---|---|
//! | open | `opened_at` is newer than `sync_window_stale_after` (2 h) | stale: the watch job warns once, alerts flow again |
//! | closed, not processed | `closed_at` is newer than `sync_close_give_up` (30 min) | given up: the evaluation runs without the sync's processing |
//!
//! `opened_at` and `closed_at` are the agent's times, clamped to the hub's clock at ingestion (`max_clock_skew`), so a wrong or hostile
//! agent clock can make a window look younger by at most that, never open for ever.
//!
//! # Races, and what bounds them
//!
//! The live path reads the gate after the pipeline's commit, and the window's evaluation reads the drift after the window was processed.
//! A finding committed between those two reads can be neither held nor evaluated (or both). The evaluation therefore reads what was
//! written in the **spans** the window held ([`held_spans`]), each widened by `max_clock_skew` (the agent's clock and the hub's differ),
//! so that a loss is closed at the price of an occasional second alert for the same drift; a duplicate is the safe side for an alert.
//! A window that was open for longer than the stale limit held in two spans, not one: what was written between them was alerted live.

use crate::clock::Clock;
use crate::db::{Db, ts};
use crate::error::RegistryError;

/// What the gate says about a swimlane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct State {
    /// An admin has confirmed the swimlane's baseline: before that every file is "unknown" and nothing is drift yet.
    pub adopted: bool,
    /// A window holds the swimlane's alerts.
    pub held: bool,
}

/// A span of the hub's time in which a swimlane's alerts were held, as Unix milliseconds, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub from_ms: i64,
    pub to_ms: i64,
}

/// The spans in which the alerts of closed windows were held, given each window's `(opened, closed)` times, so that the evaluation looks
/// at exactly what was held and not at what was already told.
///
/// A window holds from its opening (less `skew_ms`: the times are the agent's) until it is stale, and again from its close (less the
/// skew) until it is processed, which is `now_ms` for a window that is being processed. A window that was closed before it was stale
/// is one span. A window that was open for longer than `stale_after_ms` is two: what happened between was alerted live, because the
/// hold had ended, and is not evaluated again. Overlapping spans are merged and the result is sorted.
pub(crate) fn held_spans(
    windows: &[(i64, i64)],
    now_ms: i64,
    stale_after_ms: i64,
    skew_ms: i64,
) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for &(opened, closed) in windows {
        let from = opened.saturating_sub(skew_ms);
        let open_end = opened.saturating_add(stale_after_ms);
        if closed <= open_end {
            spans.push(Span {
                from_ms: from,
                to_ms: now_ms,
            });
        } else {
            spans.push(Span {
                from_ms: from,
                to_ms: open_end.saturating_add(skew_ms),
            });
            spans.push(Span {
                from_ms: closed.saturating_sub(skew_ms),
                to_ms: now_ms,
            });
        }
    }
    spans.retain(|s| s.from_ms <= s.to_ms);
    spans.sort_by_key(|s| (s.from_ms, s.to_ms));
    let mut merged: Vec<Span> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.from_ms <= last.to_ms.saturating_add(1) => {
                last.to_ms = last.to_ms.max(span.to_ms);
            }
            _ => merged.push(span),
        }
    }
    merged
}

/// The statement behind [`Gate::state`]. The two cut-offs are bound as Unix milliseconds.
pub(crate) fn state_sql() -> String {
    format!(
        "SELECT s.baseline_confirmed_at IS NOT NULL, \
                EXISTS (SELECT 1 FROM sync_windows w WHERE w.swimlane_id = s.id AND w.processed_at IS NULL \
                          AND ((w.closed_at IS NULL AND w.opened_at > {open}) \
                            OR (w.closed_at IS NOT NULL AND w.closed_at > {closed}))) \
         FROM swimlanes s WHERE s.id = $1",
        open = ts("$2"),
        closed = ts("$3"),
    )
}

/// The question the live path asks.
#[derive(Clone)]
pub(crate) struct Gate {
    db: Db,
    clock: std::sync::Arc<dyn Clock>,
    stale_after_ms: i64,
    give_up_ms: i64,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate").finish_non_exhaustive()
    }
}

impl Gate {
    pub(crate) fn new(
        db: Db,
        clock: std::sync::Arc<dyn Clock>,
        stale_after: std::time::Duration,
        give_up: std::time::Duration,
    ) -> Self {
        let ms = |d: std::time::Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
        Self {
            db,
            clock,
            stale_after_ms: ms(stale_after),
            give_up_ms: ms(give_up),
        }
    }

    /// Whether the swimlane is adopted and whether a window holds its alerts. A swimlane the hub does not have is neither.
    pub(crate) async fn state(&self, swimlane: &domain::SwimlaneId) -> Result<State, RegistryError> {
        let now = self.clock.now().unix_millis();
        let sql = state_sql();
        let row: Option<(bool, bool)> = self
            .db
            .run(
                "alert_gate",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(now.saturating_sub(self.stale_after_ms))
                    .bind(now.saturating_sub(self.give_up_ms))
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row.map_or(
            State {
                adopted: false,
                held: false,
            },
            |(adopted, held)| State { adopted, held },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;
    const MIN: i64 = 60_000;

    #[test]
    fn a_window_closed_before_it_was_stale_held_from_its_opening_to_now() {
        let now = 10 * HOUR;
        assert_eq!(
            held_spans(&[(5 * HOUR, 5 * HOUR + 10 * MIN)], now, 2 * HOUR, 5 * MIN),
            vec![Span {
                from_ms: 5 * HOUR - 5 * MIN,
                to_ms: now
            }],
            "a window closed after ten minutes: held from its opening (less the skew) until it is processed"
        );
        assert_eq!(
            held_spans(&[(5 * HOUR, 7 * HOUR)], now, 2 * HOUR, 5 * MIN),
            vec![Span {
                from_ms: 5 * HOUR - 5 * MIN,
                to_ms: now
            }],
            "closed at exactly the stale limit is still one span"
        );
    }

    #[test]
    fn a_window_open_past_the_stale_limit_did_not_hold_in_between() {
        let now = 20 * HOUR;
        let spans = held_spans(&[(5 * HOUR, 9 * HOUR)], now, 2 * HOUR, 5 * MIN);
        assert_eq!(
            spans,
            vec![
                Span {
                    from_ms: 5 * HOUR - 5 * MIN,
                    to_ms: 7 * HOUR + 5 * MIN
                },
                Span {
                    from_ms: 9 * HOUR - 5 * MIN,
                    to_ms: now
                },
            ],
            "held for the first two hours, alerted live for the next two, held again from the close"
        );
        assert!(
            !spans.iter().any(|s| s.from_ms <= 8 * HOUR && 8 * HOUR <= s.to_ms),
            "a finding from the live stretch is not in any span"
        );
    }

    #[test]
    fn spans_that_overlap_are_one_and_the_result_is_sorted() {
        let now = 10 * HOUR;
        let spans = held_spans(
            &[
                (6 * HOUR, 6 * HOUR + MIN),
                (5 * HOUR, 5 * HOUR + MIN),
                (20 * HOUR, 20 * HOUR + MIN),
            ],
            now,
            2 * HOUR,
            0,
        );
        // The third window opens after `now` (a wrong clock): nothing of it can have been held yet.
        assert_eq!(
            spans,
            vec![Span {
                from_ms: 5 * HOUR,
                to_ms: now
            }]
        );
        assert!(held_spans(&[], now, 2 * HOUR, 0).is_empty());
    }

    #[test]
    fn the_statement_binds_what_the_gate_binds() {
        let sql = state_sql();
        assert!(sql.contains("$1") && sql.contains("$2") && sql.contains("$3"));
        assert!(!sql.contains("$4"));
        assert!(!sql.contains("OFFSET"));
        assert!(sql.contains("processed_at IS NULL"));
    }
}
