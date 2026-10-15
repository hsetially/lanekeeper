//! The reveal action (T10, D79, S14b): the one audited way to see a value the secret scan flags.
//!
//! Everything that shows settings masks the flagged ones: the registry's views, the diffs, the previews, the approvers' list
//! ([`crate::proposals::view`]) and MCP. `POST /swimlanes/{id}/files/reveal` is how an Editor or above sees one, for a reason, with
//! a record.
//!
//! A reveal does five things, in this order, and ends at the first that says no:
//!
//! 1. **Authorize.** Editor or above, read from the directory on every call (S4), and only through the web UI: a call that came
//!    in as an MCP tool is refused whatever the role, because no tool reveals a flagged value (S14b).
//! 2. **Rate limit.** At most [`crate::WriteConfig::reveals_per_minute`] a user in any 60 seconds, on this replica. Every attempt
//!    that passes the role check counts, found or not, so the route cannot be used to probe a file for secrets at a faster rate.
//! 3. **Find.** The file is read through the registry (which withholds a denied file), parsed and scanned by the engine on the
//!    blocking pool, and the setting must be one the mask hides. The same function decides both ([`crate::proposals::view`]'s
//!    `marker`), so a setting can be revealed exactly when it is masked, and a value that is not flagged is not "revealed"
//!    (there is nothing to audit and no reason to ask).
//! 4. **Audit.** One `FlaggedValueRevealed` event, committed before the value leaves this module. If the audit log cannot take
//!    the event, the value is not returned (fail closed). The event names the user, the file, the file's hash and, in `diff`, the
//!    setting, its line and the reason: never the value. A reason that repeats the value, or that the scan flags, is replaced by
//!    the fixed text, so the record cannot be made to hold the secret.
//! 5. **Answer.** The value in a [`Secret`], so that nothing that formats it prints it. It is not stored: not in the idempotency
//!    table (a reveal is not replayable, because the stored answer would be the secret), not in a cache, not in an event.
//!
//! Nothing here logs the setting, the value or the reason; the span carries the request id, the user, the swimlane and the path.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    ContentHash, ContentSource, FileClass, NfsPath, Role, Secret, SettingPath, ShortText, SwimlaneId, UserId,
    Via, WriteCtx,
};
use engine::{DenyMatcher, Limits, ScanConfig, classify, secret_scan};
use ports::{AuditAction, AuditActor, AuditEvent, ReadError, Tx, WriteError, content_hash};
use tracing::Instrument;

use crate::error::Internal;
use crate::pipeline::approval::via_of;
use crate::pipeline::authorize::authorize;
use crate::proposals::view;
use crate::service::{Inner, WriteHub};

/// What a reveal asks for: one setting of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealRequest {
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub setting: SettingPath,
    /// Why the person needs to see it. Recorded in the audit event; without one the event says [`DEFAULT_REASON`].
    pub reason: Option<ShortText>,
}

/// The reason an event carries when the request gave none, or one that could not be kept.
pub const DEFAULT_REASON: &str = "masked_value_reveal";

/// The revealed value. A [`Secret`], so a `Debug` print, a log field or an error that holds it shows `[redacted]`.
#[derive(Debug)]
pub struct Revealed {
    pub value: Secret<String>,
}

/// Why a reveal did not happen. A fixed word each: nothing of the file, the setting or the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RevealError {
    /// Below Editor, not active, or not through the web UI.
    #[error("not allowed")]
    Forbidden,
    /// There is nothing to reveal there: no such file or setting, a value the scan does not flag, or a file that is withheld.
    /// One answer for all of them, so the route is not a way to learn which.
    #[error("nothing to reveal")]
    NotFound,
    /// More than the limit in the last minute; the wait is in whole seconds, at least one.
    #[error("too many reveals")]
    Throttled { retry_after_secs: u64 },
    /// The registry, the audit log or the store did not answer, so nothing was revealed.
    #[error("reveal is unavailable")]
    Unavailable,
}

/// The route calls this. [`WriteHub`] implements it.
#[async_trait]
pub trait RevealDesk: Send + Sync + 'static {
    /// Reveal one flagged value. Editor and above, rate limited, audited.
    async fn reveal(&self, ctx: &WriteCtx, r: RevealRequest) -> Result<Revealed, RevealError>;
}

/// The length of the rate limit's window.
const WINDOW: Duration = Duration::from_secs(60);

/// A sliding-window limit per user: at most `per_minute` takes in any 60 seconds. The table of users is bounded, and a user's
/// window is never dropped while it still limits them: when the table is full, the windows whose last take is a whole window old
/// are swept out, and if none is, the newcomer is refused (fail closed) until one ends. A cache that evicts by popularity or
/// recency could push a limited user out and give them a fresh window, which is a way round the limit.
pub(crate) struct RevealLimiter {
    per_minute: usize,
    max_users: usize,
    windows: Mutex<HashMap<UserId, VecDeque<i64>>>,
}

impl RevealLimiter {
    pub fn new(per_minute: u32, tracked_users: u64) -> Self {
        Self {
            per_minute: usize::try_from(per_minute).unwrap_or(usize::MAX),
            max_users: usize::try_from(tracked_users).unwrap_or(usize::MAX),
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Count one attempt of `user` at `now_ms`. `Err` is the wait, in whole seconds, until a slot is free.
    pub fn take(&self, user: &UserId, now_ms: i64) -> Result<(), u64> {
        let window_ms = i64::try_from(WINDOW.as_millis()).unwrap_or(60_000);
        let seconds = |wait_ms: i64| {
            // At most a window, even if the clock was moved: the wait is a hint, the next take decides.
            u64::try_from(wait_ms.max(1))
                .unwrap_or(60_000)
                .div_ceil(1000)
                .clamp(1, 60)
        };
        let mut table = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if !table.contains_key(user) && table.len() >= self.max_users {
            // Full. A window whose last take is a whole window old limits nobody, so only those go.
            let horizon = now_ms.saturating_sub(window_ms);
            table.retain(|_, taken| taken.back().is_some_and(|&at| at > horizon));
            if table.len() >= self.max_users {
                // Every window is live: the newcomer waits for the first of them to end.
                let first_end = table
                    .values()
                    .filter_map(|taken| taken.back().copied())
                    .min()
                    .unwrap_or(now_ms);
                return Err(seconds(
                    first_end.saturating_add(window_ms).saturating_sub(now_ms),
                ));
            }
        }
        let taken = table.entry(user.clone()).or_default();
        // A take leaves the window when it is a whole window old.
        while taken
            .front()
            .is_some_and(|&at| at <= now_ms.saturating_sub(window_ms))
        {
            taken.pop_front();
        }
        if taken.len() >= self.per_minute {
            let oldest = taken.front().copied().unwrap_or(now_ms);
            return Err(seconds(oldest.saturating_add(window_ms).saturating_sub(now_ms)));
        }
        taken.push_back(now_ms);
        Ok(())
    }

    #[cfg(test)]
    fn tracked(&self) -> u64 {
        u64::try_from(self.windows.lock().unwrap_or_else(PoisonError::into_inner).len()).unwrap_or(u64::MAX)
    }
}

/// What was found at the setting.
#[derive(Debug)]
pub(crate) struct Found {
    /// A [`Secret`] from the moment it leaves the parse, so that nothing that prints it shows it.
    pub value: Secret<String>,
    /// The first line of the setting.
    pub line: u32,
    /// The hash of the bytes that were read.
    pub hash: ContentHash,
}

/// The value of `setting` in `bytes`, if the mask hides it. `None` for everything else.
pub(crate) fn find(
    path: &NfsPath,
    bytes: &Bytes,
    setting: &SettingPath,
    deny: &DenyMatcher,
    limits: &Limits,
) -> Option<Found> {
    // Only a structured file has settings; a denied path is never read, whatever the registry handed over.
    if classify(path, Some(bytes), deny) != FileClass::Structured {
        return None;
    }
    let side = view::side(path, FileClass::Structured, bytes, limits)?;
    let found = side.settings.iter().find(|s| &s.path == setting)?;
    // The mask's own test: the value is revealed only if it is hidden.
    view::marker(side.flags.as_ref(), found.lines)?;
    Some(Found {
        value: Secret::new(found.value.text.clone()),
        line: found.lines.start(),
        hash: content_hash(bytes),
    })
}

/// The reason the event carries.
pub(crate) fn reason_text(reason: Option<&ShortText>, value: &str, limits: &Limits) -> String {
    let Some(reason) = reason else {
        return DEFAULT_REASON.to_owned();
    };
    let text = reason.as_str().trim();
    if text.is_empty() || (!value.is_empty() && text.contains(value)) {
        return DEFAULT_REASON.to_owned();
    }
    // The record is permanent: text that the scan flags as a secret does not go into it.
    match secret_scan(text, &ScanConfig::default(), limits) {
        Ok(found) if found.is_empty() => text.to_owned(),
        _ => DEFAULT_REASON.to_owned(),
    }
}

/// The line of the event's `diff`: the setting, its line and the reason, and no value.
pub(crate) fn audit_line(setting: &SettingPath, line: u32, reason: &str) -> String {
    format!("revealed setting {setting} at line {line}; reason: {reason}")
}

#[async_trait]
impl RevealDesk for WriteHub {
    async fn reveal(&self, ctx: &WriteCtx, r: RevealRequest) -> Result<Revealed, RevealError> {
        // The span names who, where and which file; the setting and the reason are the person's words and stay out of it.
        let span = tracing::info_span!(
            "write.reveal",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
            path = %r.path,
        );
        reveal(&self.inner, ctx, r).instrument(span).await
    }
}

async fn reveal(hub: &Inner, ctx: &WriteCtx, r: RevealRequest) -> Result<Revealed, RevealError> {
    // 1. Only through the web UI, and only an Editor or above, as the directory says now.
    if ctx.via != Via::Ui {
        return Err(RevealError::Forbidden);
    }
    let user = authorize(hub, ctx, Role::Editor).await.map_err(|e| match e {
        WriteError::Forbidden => RevealError::Forbidden,
        _ => RevealError::Unavailable,
    })?;

    // 2. Every attempt by someone who may reveal counts, whether or not it finds anything.
    hub.reveals
        .take(&user.id, hub.now_ms())
        .map_err(|retry_after_secs| RevealError::Throttled { retry_after_secs })?;

    // 3. The file as the registry has it. A file it withholds, or one the user may not see, is "nothing to reveal".
    let read = hub
        .within(
            "reveal_read",
            hub.registry
                .content(&user, &r.swimlane, &r.path, ContentSource::Nfs),
        )
        .await
        .map_err(|_| RevealError::Unavailable)?;
    let content = match read {
        Ok(content) => content,
        Err(ReadError::Unavailable) => return Err(RevealError::Unavailable),
        Err(ReadError::NotFound | ReadError::Forbidden | ReadError::BadRequest) => {
            return Err(RevealError::NotFound);
        }
    };
    let (false, Some(bytes)) = (content.withheld, content.bytes) else {
        return Err(RevealError::NotFound);
    };
    let found = hub
        .blocking("reveal_find", {
            let (path, setting, reason) = (r.path.clone(), r.setting.clone(), r.reason.clone());
            let (deny, limits) = (hub.deny.clone(), hub.limits.clone());
            move || {
                let found = find(&path, &bytes, &setting, &deny, &limits)?;
                let reason = reason_text(reason.as_ref(), found.value.expose(), &limits);
                Some((found, reason))
            }
        })
        .await
        .map_err(|_| RevealError::Unavailable)?;
    let Some((found, reason)) = found else {
        return Err(RevealError::NotFound);
    };

    // 4. The record is committed before the value leaves: if it cannot be kept, nothing is shown.
    let mut event = AuditEvent::new(
        hub.clock.now(),
        AuditActor::User {
            user: user.id.clone(),
        },
        AuditAction::FlaggedValueRevealed,
        via_of(ctx.via),
    );
    event.swimlane = Some(r.swimlane.clone());
    event.path = Some(r.path.clone());
    event.hash_before = Some(found.hash);
    event.diff = Some(audit_line(&r.setting, found.line, &reason));
    event.request_id = Some(ctx.request_id.clone());
    let recorded = hub
        .within("reveal_audit", async {
            let mut tx = hub
                .pool
                .begin()
                .await
                .map_err(|e| Internal::db("reveal_audit", &e))?;
            hub.audit
                .record(&mut Tx::from_pg(&mut tx), event)
                .await
                .map_err(|_| Internal::new("reveal_audit", "audit log"))?;
            tx.commit().await.map_err(|e| Internal::db("reveal_audit", &e))?;
            Ok::<(), Internal>(())
        })
        .await;
    if !matches!(recorded, Ok(Ok(()))) {
        return Err(RevealError::Unavailable);
    }

    // 5. The value, and nothing else.
    Ok(Revealed { value: found.value })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::validate::Old;
    use crate::proposals::view::diff_view;

    fn key() -> String {
        ["AKIA", "IOSFODNN7EXAMPLE"].concat()
    }

    fn p(s: &str) -> NfsPath {
        NfsPath::parse(s).unwrap()
    }

    fn sp(s: &str) -> SettingPath {
        SettingPath::parse(s).unwrap()
    }

    fn user(n: u32) -> UserId {
        ports::conformance::sample::active(n, Role::Editor).id
    }

    fn deny() -> DenyMatcher {
        DenyMatcher::with_globs(std::iter::empty::<&str>()).unwrap()
    }

    fn look(path: &str, text: &str, setting: &str) -> Option<Found> {
        find(
            &p(path),
            &Bytes::copy_from_slice(text.as_bytes()),
            &sp(setting),
            &deny(),
            &Limits::DEFAULT,
        )
    }

    // ------------------------------------------------------------------------------------------------ the rate limit

    #[test]
    fn the_limit_is_reached_and_the_wait_is_whole_seconds_within_the_window() {
        let l = RevealLimiter::new(3, 100);
        let u = user(1);
        for _ in 0..3 {
            assert_eq!(l.take(&u, 1_000), Ok(()));
        }
        let wait = l.take(&u, 1_000).unwrap_err();
        assert!((1..=60).contains(&wait), "{wait}");
        // Still inside the window one millisecond before it ends, and refused with a wait of at least a second.
        assert_eq!(l.take(&u, 1_000 + 59_999), Err(1));
        // The window has passed.
        assert_eq!(l.take(&u, 1_000 + 60_000), Ok(()));
    }

    #[test]
    fn the_window_slides_and_does_not_reset() {
        let l = RevealLimiter::new(3, 100);
        let u = user(1);
        assert_eq!(l.take(&u, 0), Ok(()));
        assert_eq!(l.take(&u, 0), Ok(()));
        assert_eq!(l.take(&u, 50_000), Ok(()));
        assert!(l.take(&u, 59_000).is_err());
        // At 60 s the two takes at 0 have left the window, the one at 50 s has not: two slots, not three.
        assert_eq!(l.take(&u, 60_000), Ok(()));
        assert_eq!(l.take(&u, 60_000), Ok(()));
        assert_eq!(l.take(&u, 60_000), Err(50));
    }

    #[test]
    fn each_user_has_a_window_of_their_own() {
        let l = RevealLimiter::new(1, 100);
        assert_eq!(l.take(&user(1), 0), Ok(()));
        assert!(l.take(&user(1), 1).is_err());
        assert_eq!(l.take(&user(2), 1), Ok(()));
    }

    #[test]
    fn a_full_table_never_resets_the_window_of_a_user_who_is_still_limited() {
        let l = RevealLimiter::new(1, 3);
        assert_eq!(l.take(&user(0), 0), Ok(()));
        // A crowd of other users arrives while user 0's window is live. Whatever happens to them, user 0 is still limited.
        for n in 1..300 {
            let _ = l.take(&user(n), 1);
        }
        assert!(l.take(&user(0), 2).is_err(), "the window of user 0 was reset");
        assert!(l.tracked() <= 3, "{}", l.tracked());
    }

    #[test]
    fn a_new_user_is_refused_while_the_table_holds_only_live_windows_and_admitted_when_one_ends() {
        let l = RevealLimiter::new(1, 2);
        assert_eq!(l.take(&user(1), 0), Ok(()));
        assert_eq!(l.take(&user(2), 10), Ok(()));
        // Full of live windows: the newcomer fails closed with a wait to the first window that ends.
        let wait = l.take(&user(3), 20).unwrap_err();
        assert!((1..=60).contains(&wait), "{wait}");
        // Both are still limited, and the refusal did not cost them anything.
        assert!(l.take(&user(1), 30).is_err());
        assert!(l.take(&user(2), 30).is_err());
        // The window of user 1 has ended, so its slot goes to the newcomer; user 2's window is still live.
        assert_eq!(l.take(&user(3), 60_000), Ok(()));
        assert!(l.take(&user(2), 60_005).is_err());
        assert!(l.tracked() <= 2, "{}", l.tracked());
    }

    #[test]
    fn the_table_of_users_is_bounded() {
        let l = RevealLimiter::new(1, 10);
        for n in 0..500 {
            let _ = l.take(&user(n), 0);
        }
        assert!(l.tracked() <= 10, "{}", l.tracked());
    }

    // ------------------------------------------------------------------------------------------------ finding the value

    #[test]
    fn a_flagged_value_is_found_with_its_line_and_the_hash_of_the_file() {
        let text = format!("a: 1\nsecret:\n  key: {}\nz: 2\n", key());
        let found = look("c.yml", &text, "secret.key").unwrap();
        assert_eq!(found.value.expose(), &key());
        assert_eq!(found.line, 3);
        assert_eq!(found.hash, ports::content_hash(text.as_bytes()));
    }

    #[test]
    fn a_value_that_is_not_flagged_is_not_revealed() {
        let text = format!("a: 1\nsecret: {}\n", key());
        assert!(look("c.yml", &text, "a").is_none());
        assert!(look("c.yml", &text, "secret").is_some());
    }

    #[test]
    fn nothing_is_found_for_a_missing_setting_a_text_file_a_bad_file_or_a_denied_path() {
        let text = format!("a: 1\nsecret: {}\n", key());
        assert!(look("c.yml", &text, "nope").is_none());
        // A text file has lines, not settings.
        assert!(look("notes.txt", &format!("secret: {}\n", key()), "secret").is_none());
        // A file that does not parse has no settings.
        assert!(look("c.yml", &format!("a: [{}\n", key()), "a").is_none());
        // A denied path is never read, whatever the bytes are.
        assert!(look("server.pem", &text, "secret").is_none());
        assert!(look("tx/private-keys.yml", &text, "secret").is_none());
        let with_extra = DenyMatcher::with_globs(["*.vault"]).unwrap();
        assert!(
            find(
                &p("a/b.vault"),
                &Bytes::copy_from_slice(text.as_bytes()),
                &sp("secret"),
                &with_extra,
                &Limits::DEFAULT
            )
            .is_none()
        );
    }

    #[test]
    fn list_items_properties_and_block_values_are_found_by_their_setting_path() {
        let list = format!("keys:\n  - one\n  - {}\n", key());
        assert!(look("c.yml", &list, "keys[1]").is_some());
        assert!(look("c.yml", &list, "keys[0]").is_none());
        let props = format!("db.user=app\ndb.token={}\n", key());
        assert_eq!(look("c.properties", &props, "db.token").unwrap().line, 2);
        assert!(look("c.properties", &props, "db.user").is_none());
        let json = format!("{{\"a\": {{\"k\": \"{}\"}}}}", key());
        assert!(look("c.json", &json, "a.k").is_some());
        let block = format!("pem: |\n  line one\n  {}\n  line three\nafter: 1\n", key());
        let found = look("c.yml", &block, "pem").unwrap();
        assert_eq!(found.line, 1);
        assert!(found.value.expose().contains(&key()));
        assert!(look("c.yml", &block, "after").is_none());
    }

    /// The reveal and the mask are one decision: a setting can be revealed exactly when a diff would show it masked.
    #[test]
    fn a_setting_can_be_revealed_exactly_when_it_is_masked() {
        let text = format!(
            "plain: 1\nname: app\ncreds:\n  aws: {}\n  note: hello\nlist:\n  - {}\n  - two\n",
            key(),
            key()
        );
        let bytes = Bytes::copy_from_slice(text.as_bytes());
        let view = diff_view(&p("c.yml"), &Old::Absent, &bytes, &deny(), &Limits::DEFAULT);
        assert!(view.setting_changes.len() >= 5);
        let mut masked = 0;
        for change in &view.setting_changes {
            let path = change.path.as_str();
            let hidden = change
                .right
                .as_ref()
                .is_some_and(|v| v.text.starts_with("[REDACTED: "));
            assert_eq!(look("c.yml", &text, path).is_some(), hidden, "{path}");
            masked += usize::from(hidden);
        }
        assert_eq!(masked, 2);
    }

    // ------------------------------------------------------------------------------------------------ the reason and the line

    #[test]
    fn a_reason_is_kept_and_a_missing_or_blank_one_is_the_fixed_text() {
        let l = Limits::DEFAULT;
        let why = ShortText::parse("rotating the staging key, ticket 4411").unwrap();
        assert_eq!(
            reason_text(Some(&why), "the-value", &l),
            "rotating the staging key, ticket 4411"
        );
        assert_eq!(reason_text(None, "the-value", &l), DEFAULT_REASON);
        let blank = ShortText::parse("   ").unwrap();
        assert_eq!(reason_text(Some(&blank), "the-value", &l), DEFAULT_REASON);
    }

    #[test]
    fn a_reason_that_repeats_the_value_or_holds_a_secret_is_replaced() {
        let l = Limits::DEFAULT;
        let value = "hunter2-staging-password";
        let repeats = ShortText::parse("needed because the value is hunter2-staging-password").unwrap();
        assert_eq!(reason_text(Some(&repeats), value, &l), DEFAULT_REASON);
        let other_secret = ShortText::parse(&format!("compare with {}", key())).unwrap();
        assert_eq!(reason_text(Some(&other_secret), "x", &l), DEFAULT_REASON);
    }

    #[test]
    fn the_audit_line_names_the_setting_the_line_and_the_reason() {
        let line = audit_line(&sp("db.token"), 12, "rotation");
        assert_eq!(line, "revealed setting db.token at line 12; reason: rotation");
    }

    #[test]
    fn errors_are_fixed_words_that_hold_no_input() {
        for e in [
            RevealError::Forbidden,
            RevealError::NotFound,
            RevealError::Throttled { retry_after_secs: 7 },
            RevealError::Unavailable,
        ] {
            let shown = format!("{e} {e:?}");
            assert!(!shown.contains('/') && !shown.contains('='), "{shown}");
        }
        let r = Revealed {
            value: Secret::new("the-secret-value".to_owned()),
        };
        assert!(!format!("{r:?}").contains("the-secret-value"));
    }
}
