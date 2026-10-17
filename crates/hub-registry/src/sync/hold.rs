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
//! # The budget: a hold cannot be chained
//!
//! Each of those bounds is per window, and a window is whatever the agent reports: a compromised or buggy agent could report window
//! after window, each with a new Job uid, and hold a swimlane's alerts for about two hours each, for ever. So the hold has a **budget**
//! for each swimlane: alerts may be held by windows for at most `sync_hold_budget` (4 h) in total within any rolling
//! `sync_hold_budget_window` (24 h). The time counted is the time the windows were in a holding phase (open and not stale, or closed and
//! not yet processed and not given up: the table above), the phases of different windows counted once where they overlap. It is read from
//! the rows of `sync_windows` on every question ([`spent`]), never from the memory of a process, so a restart or another replica cannot
//! reset it. While it is spent ([`exhausted`]) [`Gate::state`] says "not held" whatever windows are open, so alerts flow as they happen,
//! and the closing evaluation of a window still runs as it does without a budget. Time that was in a holding phase keeps counting while
//! the budget is spent, so a chain of windows keeps the budget spent for as long as it goes on, and it refills only as the spans slide out
//! of the rolling window. The watch job says so once for each **episode** of exhaustion ([`exhaustion_start`] is when it began; see
//! `window.rs` for how "once" is kept durable). The evaluation of a window reads the spans the window held, as before, so after the budget
//! is spent it can repeat an alert that was sent live (a duplicate, never a loss).
//!
//! # Races, and what bounds them
//!
//! The live path reads the gate after the pipeline's commit, and the window's evaluation reads the drift after the window was processed.
//! A finding committed between those two reads can be neither held nor evaluated (or both). The evaluation therefore reads what was
//! written in the **spans** the window held ([`held_spans`]), each widened by `max_clock_skew` (the agent's clock and the hub's differ),
//! so that a loss is closed at the price of an occasional second alert for the same drift; a duplicate is the safe side for an alert.
//! A window that was open for longer than the stale limit held in two spans, not one: what was written between them was alerted live.

use tracing::debug;

use crate::clock::Clock;
use crate::db::{Db, ms, ts};
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

/// The most windows read for one swimlane's budget. A swimlane with more windows than this in two rolling windows is not reporting
/// Jobs: its budget counts as spent (rule 5).
pub(crate) const MAX_BUDGET_WINDOWS: i64 = 1_000;

/// The times of one window as the budget reads them, as Unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Times {
    pub opened: i64,
    pub closed: Option<i64>,
    pub processed: Option<i64>,
}

/// The rules of the hold, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Policy {
    /// An open window stops holding this long after it opened.
    pub stale_after: i64,
    /// A closed window that is not processed stops holding this long after it closed.
    pub give_up: i64,
    /// The most that windows may hold a swimlane's alerts in any `within`.
    pub budget: i64,
    pub within: i64,
}

impl Policy {
    pub(crate) fn of(config: &crate::config::RegistryConfig) -> Self {
        let ms = |d: std::time::Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
        Self {
            stale_after: ms(config.sync_window_stale_after),
            give_up: ms(config.sync_close_give_up),
            budget: ms(config.sync_hold_budget),
            within: ms(config.sync_hold_budget_window),
        }
    }
}

/// The holding phases of one window up to `now`, as half-open spans `[from, to)`: from its opening until it closes, goes stale or `now`
/// (whichever is first), and from its close until it is processed, is given up or `now`.
fn phases(w: &Times, now: i64, p: &Policy) -> Vec<(i64, i64)> {
    let mut out = Vec::with_capacity(2);
    let open_end = w
        .closed
        .unwrap_or(now)
        .min(w.opened.saturating_add(p.stale_after))
        .min(now);
    if w.opened < open_end {
        out.push((w.opened, open_end));
    }
    if let Some(closed) = w.closed {
        let end = w
            .processed
            .unwrap_or(now)
            .min(closed.saturating_add(p.give_up))
            .min(now);
        if closed < end {
            out.push((closed, end));
        }
    }
    out
}

/// The time any window held, as sorted spans that do not overlap or touch.
fn union(times: &[Times], now: i64, p: &Policy) -> Vec<(i64, i64)> {
    let mut all: Vec<(i64, i64)> = times.iter().flat_map(|w| phases(w, now, p)).collect();
    all.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (from, to) in all {
        match merged.last_mut() {
            Some(last) if from <= last.1 => last.1 = last.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    merged
}

/// How much of `spans` lies in `[lo, hi)`.
fn measure(spans: &[(i64, i64)], lo: i64, hi: i64) -> i64 {
    spans
        .iter()
        .map(|&(from, to)| (to.min(hi) - from.max(lo)).max(0))
        .sum()
}

/// How long the windows held the swimlane's alerts in the rolling window that ends at `now`: the budget that is used.
pub(crate) fn spent(times: &[Times], now: i64, p: &Policy) -> i64 {
    measure(&union(times, now, p), now.saturating_sub(p.within), now)
}

/// The budget is spent: the hold has ended, whatever windows are open.
pub(crate) fn exhausted(times: &[Times], now: i64, p: &Policy) -> bool {
    spent(times, now, p) >= p.budget
}

/// When the current episode of exhaustion began (the instant the spent time reached the budget), or `None` while the budget is not spent.
/// An episode that began before the rolling window that ends at `now` began is said to begin at the start of that window.
///
/// The spent time is linear between the instants where a span of a window begins or ends or leaves the rolling window, and rises by one
/// millisecond for each millisecond only where it is crossing the budget from below, so the instant is found exactly from the last such
/// instant at which the budget was not spent.
pub(crate) fn exhaustion_start(times: &[Times], now: i64, p: &Policy) -> Option<i64> {
    let held = union(times, now, p);
    let at = |t: i64| measure(&held, t.saturating_sub(p.within), t);
    if at(now) < p.budget {
        return None;
    }
    let lo = now.saturating_sub(p.within);
    let mut points = vec![lo, now];
    for &(from, to) in &held {
        for e in [
            from,
            to,
            from.saturating_add(p.within),
            to.saturating_add(p.within),
        ] {
            if (lo..=now).contains(&e) {
                points.push(e);
            }
        }
    }
    points.sort_unstable();
    points.dedup();
    for &t in points.iter().rev() {
        let used = at(t);
        if used < p.budget {
            return Some(t + (p.budget - used));
        }
    }
    Some(lo)
}

/// The windows that can have held the swimlane's alerts in the two rolling windows that end at `now`, newest first.
///
/// # Errors
///
/// `Capacity` when there are [`MAX_BUDGET_WINDOWS`] or more: the caller counts the budget as spent. A database error is the database's.
pub(crate) async fn load_times(
    db: &Db,
    swimlane: &domain::SwimlaneId,
    now: i64,
    p: &Policy,
) -> Result<Vec<Times>, RegistryError> {
    // A phase ends at most `stale_after` after an opening and `give_up` after a close, so a window older than that cannot reach back.
    let from = now.saturating_sub(p.within.saturating_mul(2));
    let sql = format!(
        "SELECT {opened}, {closed}, {processed} FROM sync_windows \
         WHERE swimlane_id = $1 AND (opened_at > {open_cut} OR closed_at > {close_cut}) \
         ORDER BY opened_at DESC, id DESC LIMIT $4",
        opened = ms("opened_at"),
        closed = ms("closed_at"),
        processed = ms("processed_at"),
        open_cut = ts("$2"),
        close_cut = ts("$3"),
    );
    let rows: Vec<(i64, Option<i64>, Option<i64>)> = db
        .run(
            "hold_budget",
            sqlx::query_as(&sql)
                .bind(swimlane.as_str())
                .bind(from.saturating_sub(p.stale_after))
                .bind(from.saturating_sub(p.give_up))
                .bind(MAX_BUDGET_WINDOWS)
                .fetch_all(db.pool()),
        )
        .await?;
    if rows.len() >= usize::try_from(MAX_BUDGET_WINDOWS).unwrap_or(usize::MAX) {
        return Err(RegistryError::Capacity);
    }
    Ok(rows
        .into_iter()
        .map(|(opened, closed, processed)| Times {
            opened,
            closed,
            processed,
        })
        .collect())
}

/// The spans in which the alerts of closed windows were held, given each window's `(opened, closed)` times, so that the evaluation looks
/// at exactly what was held and not at what was already told.
///
/// A window holds from its opening (less `skew_ms`: the times are the agent's) until it is stale, and again from its close (less the
/// skew) until it is processed, which is `now_ms` for a window that is being processed. A window that was closed before it was stale
/// is one span. A window that was open for longer than `stale_after` is two: what happened between was alerted live, because the
/// hold had ended, and is not evaluated again. Overlapping spans are merged and the result is sorted.
pub(crate) fn held_spans(windows: &[(i64, i64)], now_ms: i64, stale_after: i64, skew_ms: i64) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for &(opened, closed) in windows {
        let from = opened.saturating_sub(skew_ms);
        let open_end = opened.saturating_add(stale_after);
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

/// What the hold budget says about a swimlane at one instant: the rows it read, the time that windows held in the rolling window, whether
/// that spends the budget, and when the episode of exhaustion began.
///
/// **Not part of the API** (hidden from the docs, as `ingest::fuzzing` is): the hub asks through `Gate`. It exists so that a test in
/// `tests/` can compare, to the millisecond, what the budget says before and after the retention job (T9, D117) prunes `sync_windows`,
/// which is the proof that the prune never takes a row the budget still reads. It reads, never writes, and returns numbers about one
/// swimlane's own windows, nothing a caller could not already see through the alerts.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetProbe {
    pub windows_read: usize,
    pub spent_ms: i64,
    pub exhausted: bool,
    pub exhaustion_start_ms: Option<i64>,
}

/// The hold budget of `swimlane` at `now_ms`, from the rows of `sync_windows` (see [`BudgetProbe`]). A swimlane with too many windows to
/// read counts as exhausted, as it does for the gate.
///
/// # Errors
///
/// The database failed.
#[doc(hidden)]
pub async fn probe(
    pool: sqlx::PgPool,
    config: &crate::config::RegistryConfig,
    swimlane: &domain::SwimlaneId,
    now_ms: i64,
) -> Result<BudgetProbe, RegistryError> {
    let db = Db::new(pool, config.query_timeout);
    let policy = Policy::of(config);
    match load_times(&db, swimlane, now_ms, &policy).await {
        Ok(times) => Ok(BudgetProbe {
            windows_read: times.len(),
            spent_ms: spent(&times, now_ms, &policy),
            exhausted: exhausted(&times, now_ms, &policy),
            exhaustion_start_ms: exhaustion_start(&times, now_ms, &policy),
        }),
        Err(RegistryError::Capacity) => Ok(BudgetProbe {
            windows_read: usize::try_from(MAX_BUDGET_WINDOWS).unwrap_or(usize::MAX),
            spent_ms: policy.budget,
            exhausted: true,
            exhaustion_start_ms: None,
        }),
        Err(e) => Err(e),
    }
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
    policy: Policy,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate").finish_non_exhaustive()
    }
}

impl Gate {
    pub(crate) fn new(db: Db, clock: std::sync::Arc<dyn Clock>, policy: Policy) -> Self {
        Self { db, clock, policy }
    }

    /// Whether the swimlane is adopted and whether a window holds its alerts: a window is in a holding phase **and** the budget is not
    /// spent. A swimlane the hub does not have is neither.
    pub(crate) async fn state(&self, swimlane: &domain::SwimlaneId) -> Result<State, RegistryError> {
        let now = self.clock.now().unix_millis();
        let sql = state_sql();
        let row: Option<(bool, bool)> = self
            .db
            .run(
                "alert_gate",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(now.saturating_sub(self.policy.stale_after))
                    .bind(now.saturating_sub(self.policy.give_up))
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        let Some((adopted, mut held)) = row else {
            return Ok(State {
                adopted: false,
                held: false,
            });
        };
        if held {
            // The budget is read only when a window holds, so a swimlane with no window costs one statement. A budget that cannot be read
            // does not hold: an alert that is not wanted is better than one that was swallowed.
            held = match load_times(&self.db, swimlane, now, &self.policy).await {
                Ok(times) => {
                    let spent_out = exhausted(&times, now, &self.policy);
                    if spent_out {
                        debug!(swimlane = %swimlane, "the hold budget is spent: alerts are not held");
                    }
                    !spent_out
                }
                Err(_) => false,
            };
        }
        Ok(State { adopted, held })
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

    fn policy() -> Policy {
        Policy {
            stale_after: 2 * HOUR,
            give_up: 30 * MIN,
            budget: 4 * HOUR,
            within: 24 * HOUR,
        }
    }

    fn open(opened: i64) -> Times {
        Times {
            opened,
            closed: None,
            processed: None,
        }
    }

    fn closed(opened: i64, closed: i64, processed: Option<i64>) -> Times {
        Times {
            opened,
            closed: Some(closed),
            processed,
        }
    }

    #[test]
    fn an_open_window_counts_until_it_is_stale() {
        let p = policy();
        assert_eq!(spent(&[open(0)], HOUR, &p), HOUR);
        assert_eq!(spent(&[open(0)], 2 * HOUR, &p), 2 * HOUR);
        assert_eq!(
            spent(&[open(0)], 9 * HOUR, &p),
            2 * HOUR,
            "a stale window holds no more, whatever its age"
        );
        assert_eq!(
            spent(&[open(5 * HOUR)], HOUR, &p),
            0,
            "a window that opens after now has held nothing"
        );
    }

    #[test]
    fn a_closed_window_counts_until_it_is_processed_or_given_up() {
        let p = policy();
        // Open for an hour, closed, never processed: an hour, then the half hour of the give-up.
        assert_eq!(spent(&[closed(0, HOUR, None)], 10 * HOUR, &p), HOUR + 30 * MIN);
        // Processed ten minutes after the close.
        assert_eq!(
            spent(&[closed(0, HOUR, Some(HOUR + 10 * MIN))], 10 * HOUR, &p),
            HOUR + 10 * MIN
        );
        // Processed after the give-up (the degraded processing): the hold had ended at the give-up.
        assert_eq!(
            spent(&[closed(0, HOUR, Some(5 * HOUR))], 10 * HOUR, &p),
            HOUR + 30 * MIN
        );
        // Not yet processed and the give-up not yet reached: up to now.
        assert_eq!(
            spent(&[closed(0, HOUR, None)], HOUR + 10 * MIN, &p),
            HOUR + 10 * MIN
        );
    }

    #[test]
    fn windows_that_overlap_count_once() {
        let p = policy();
        let a = closed(0, HOUR, Some(HOUR));
        let b = closed(30 * MIN, 90 * MIN, Some(90 * MIN));
        assert_eq!(
            spent(&[a, b], 10 * HOUR, &p),
            90 * MIN,
            "the union [0, 90 min], not 2 h"
        );
        assert_eq!(spent(&[b, a], 10 * HOUR, &p), 90 * MIN, "in any order");
        assert_eq!(spent(&[a, a], 10 * HOUR, &p), HOUR, "a window twice is one");
    }

    #[test]
    fn only_the_rolling_window_counts() {
        let p = policy();
        // [0, 2 h] held; at 25 h the rolling window is (1 h, 25 h]: one hour of it is left.
        assert_eq!(spent(&[open(0)], 25 * HOUR, &p), HOUR);
        assert_eq!(
            spent(&[open(0)], 26 * HOUR, &p),
            0,
            "and at 26 h it has all slid out"
        );
    }

    #[test]
    fn a_chain_of_windows_spends_the_budget_and_it_is_spent_from_that_instant() {
        let p = policy();
        // Two stuck Jobs one after the other: [0, 2 h] and [2 h 01, 4 h 01]. The budget (4 h) is reached at 4 h 01 and stays.
        let chain = [open(0), open(2 * HOUR + MIN)];
        assert!(!exhausted(&chain, 4 * HOUR, &p), "3 h 59 min is held at 4 h");
        assert_eq!(spent(&chain, 4 * HOUR, &p), 4 * HOUR - MIN);
        assert!(exhausted(&chain, 4 * HOUR + MIN, &p));
        assert_eq!(exhaustion_start(&chain, 4 * HOUR, &p), None);
        assert_eq!(exhaustion_start(&chain, 4 * HOUR + MIN, &p), Some(4 * HOUR + MIN));
        // Later, with the chain still going on or not, the episode began where it did.
        assert_eq!(exhaustion_start(&chain, 9 * HOUR, &p), Some(4 * HOUR + MIN));
        let more = [open(0), open(2 * HOUR + MIN), open(4 * HOUR + 2 * MIN)];
        assert_eq!(exhaustion_start(&more, 5 * HOUR, &p), Some(4 * HOUR + MIN));
    }

    #[test]
    fn the_budget_refills_as_the_rolling_window_moves() {
        let p = policy();
        let chain = [
            open(0),
            open(2 * HOUR),
            closed(4 * HOUR, 5 * HOUR, Some(5 * HOUR)),
        ];
        // Held [0, 5 h]: five hours, so spent from 4 h on.
        assert!(exhausted(&chain, 6 * HOUR, &p));
        assert!(exhausted(&chain, 24 * HOUR, &p), "(0, 24 h] holds all five hours");
        // At 26 h the rolling window is (2 h, 26 h]: three hours are left, under the 4 h.
        assert!(!exhausted(&chain, 26 * HOUR, &p));
        assert_eq!(exhaustion_start(&chain, 26 * HOUR, &p), None);
        assert_eq!(spent(&chain, 26 * HOUR, &p), 3 * HOUR);
        // The instant it refills: 25 h, when spent drops below 4 h.
        assert!(exhausted(&chain, 25 * HOUR, &p));
        assert!(!exhausted(&chain, 25 * HOUR + MIN, &p));
    }

    #[test]
    fn a_second_episode_is_dated_from_its_own_start() {
        let p = policy();
        let first = [open(0), open(2 * HOUR)];
        let second = [open(30 * HOUR), open(32 * HOUR)];
        let both: Vec<Times> = first.iter().chain(&second).copied().collect();
        assert_eq!(exhaustion_start(&both, 5 * HOUR, &p), Some(4 * HOUR));
        // At 35 h the first chain has slid out; the second reached 4 h at 34 h.
        assert_eq!(exhaustion_start(&both, 35 * HOUR, &p), Some(34 * HOUR));
        assert!(
            !exhausted(&both, 29 * HOUR, &p),
            "between the episodes the budget was free"
        );
    }

    #[test]
    fn an_episode_older_than_the_rolling_window_begins_at_its_start() {
        let p = policy();
        let day_and_a_half: Vec<Times> = (0..30).map(|i| open(i * 2 * HOUR)).collect();
        let now = 50 * HOUR;
        assert_eq!(exhaustion_start(&day_and_a_half, now, &p), Some(now - 24 * HOUR));
    }

    mod property {
        use proptest::prelude::*;

        use super::*;

        fn times() -> impl Strategy<Value = Vec<Times>> {
            prop::collection::vec(
                (
                    0_i64..2_880,
                    0_i64..400,
                    prop::option::of(0_i64..120),
                    prop::option::of(0_i64..400),
                )
                    .prop_map(|(open_min, open_for, close_after, processed_after)| {
                        let opened = open_min * MIN;
                        let closed = close_after.map(|_| opened + open_for * MIN);
                        Times {
                            opened,
                            closed,
                            processed: closed.zip(processed_after).map(|(c, after)| c + after * MIN),
                        }
                    }),
                0..12,
            )
        }

        proptest! {
            #[test]
            fn the_episode_start_is_where_the_spent_time_reaches_the_budget(
                windows in times(),
                now_min in 0_i64..4_000,
            ) {
                let p = policy();
                let now = now_min * MIN;
                let started = exhaustion_start(&windows, now, &p);
                prop_assert_eq!(started.is_some(), exhausted(&windows, now, &p));
                if let Some(t) = started {
                    prop_assert!(t <= now);
                    let held = union(&windows, now, &p);
                    let at = |t: i64| measure(&held, t - p.within, t);
                    prop_assert!(at(t) >= p.budget, "spent at the start: {}", at(t));
                    // Unless the episode began before the rolling window, one millisecond earlier it was not spent.
                    if t > now - p.within {
                        prop_assert!(at(t - 1) < p.budget);
                    }
                    // And from then to now it never dipped below the budget (it is one episode).
                    for k in 0..=20 {
                        let probe = t + (now - t) * k / 20;
                        prop_assert!(at(probe) >= p.budget, "dipped at {probe}: {}", at(probe));
                    }
                }
            }

            #[test]
            fn more_windows_never_spend_less(
                windows in times(),
                extra in times(),
                now_min in 0_i64..4_000,
            ) {
                let p = policy();
                let now = now_min * MIN;
                let all: Vec<Times> = windows.iter().chain(&extra).copied().collect();
                prop_assert!(spent(&all, now, &p) >= spent(&windows, now, &p));
                prop_assert!(spent(&windows, now, &p) <= p.within);
            }
        }
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
