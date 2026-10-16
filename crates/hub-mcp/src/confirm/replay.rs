//! A bounded memory of finished writes, so that a retry of the same request is answered and not asked a second time (D68).
//!
//! Correctness never depends on it: the write service keeps the idempotency key and returns the first outcome for it (D66). This
//! only spares the user a second question, on this replica, for ten minutes. It is keyed by the user and the key, never by the key
//! alone: the key of `apply_change` is the same for everyone who names a draft, and one user must never be answered with the
//! outcome of another's write. An entry also remembers a fingerprint of the request, and answers only that request.

use std::sync::Arc;
use std::time::Duration;

use domain::{ContentHash, IdempotencyKey, Timestamp, UserId};
use moka::sync::Cache;

use crate::clock::Clock;
use crate::output::envelope::Rendered;

/// How long a finished write is remembered.
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// The most the memory weighs, in bytes of stored results.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
struct Entry {
    request: ContentHash,
    stored_at: Timestamp,
    result: Rendered,
}

#[derive(Clone)]
pub struct Replay {
    cache: Cache<(UserId, IdempotencyKey), Entry>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Replay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Replay").finish_non_exhaustive()
    }
}

/// What an entry weighs: the bytes of its result and a fixed share for the key and the bookkeeping.
fn weight(entry: &Entry) -> u32 {
    u32::try_from(entry.result.bytes.saturating_add(256)).unwrap_or(u32::MAX)
}

impl Replay {
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        let cache = Cache::builder()
            .max_capacity(MAX_BYTES)
            .weigher(|_key: &(UserId, IdempotencyKey), entry: &Entry| weight(entry))
            .time_to_live(TTL)
            .build();
        Self { cache, clock }
    }

    /// The stored result of the finished write of `user` under `key`, if it was this request and is not older than [`TTL`].
    #[must_use]
    pub fn get(&self, user: &UserId, key: &IdempotencyKey, request: &ContentHash) -> Option<Rendered> {
        let entry = self.cache.get(&(user.clone(), key.clone()))?;
        // The time-to-live of the cache runs on the real clock; this one is the server's, and follows the tests.
        let age_ms = self
            .clock
            .now()
            .unix_millis()
            .saturating_sub(entry.stored_at.unix_millis());
        let fresh = u128::try_from(age_ms).is_ok_and(|ms| ms < TTL.as_millis());
        (fresh && &entry.request == request).then_some(entry.result)
    }

    /// Run the cache's pending maintenance (this module's tests, so that the weight bound is visible at once). Not part of the
    /// crate's API outside them.
    #[cfg(test)]
    pub fn run_pending_tasks(&self) {
        self.cache.run_pending_tasks();
    }

    /// The weight of what is stored now (this module's tests).
    #[cfg(test)]
    #[must_use]
    pub fn weighted_size(&self) -> u64 {
        self.cache.weighted_size()
    }

    /// Remember the result of a finished write.
    pub fn put(&self, user: &UserId, key: &IdempotencyKey, request: &ContentHash, result: &Rendered) {
        self.cache.insert(
            (user.clone(), key.clone()),
            Entry {
                request: *request,
                stored_at: self.clock.now(),
                result: result.clone(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::clock::ManualClock;

    fn user(n: u8) -> UserId {
        format!("11111111-2222-4333-8444-555555555555:aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee{n:02x}")
            .parse()
            .unwrap()
    }

    fn key(text: &str) -> IdempotencyKey {
        IdempotencyKey::parse(text).unwrap()
    }

    fn fingerprint(c: char) -> ContentHash {
        ContentHash::parse(&c.to_string().repeat(64)).unwrap()
    }

    fn result(bytes: usize) -> Rendered {
        Rendered {
            structured: json!({"result": {"outcome": "applied"}}),
            content: "done".to_owned(),
            bytes,
        }
    }

    fn clock() -> Arc<ManualClock> {
        Arc::new(ManualClock::at(domain::Timestamp::from_unix_millis(
            1_700_000_000_000,
        )))
    }

    #[test]
    fn a_stored_result_is_returned_for_the_same_user_key_and_request() {
        let replay = Replay::new(clock());
        replay.put(&user(1), &key("k-1"), &fingerprint('a'), &result(10));
        let got = replay
            .get(&user(1), &key("k-1"), &fingerprint('a'))
            .expect("stored");
        assert_eq!(got.content, "done");
        assert_eq!(got.structured["result"]["outcome"], "applied");
    }

    #[test]
    fn another_user_another_key_or_another_request_gets_nothing() {
        let replay = Replay::new(clock());
        replay.put(&user(1), &key("k-1"), &fingerprint('a'), &result(10));
        assert!(
            replay.get(&user(2), &key("k-1"), &fingerprint('a')).is_none(),
            "another user"
        );
        assert!(
            replay.get(&user(1), &key("k-2"), &fingerprint('a')).is_none(),
            "another key"
        );
        assert!(
            replay.get(&user(1), &key("k-1"), &fingerprint('b')).is_none(),
            "another request"
        );
    }

    #[test]
    fn a_result_is_forgotten_after_ten_minutes() {
        let clock = clock();
        let replay = Replay::new(Arc::clone(&clock) as Arc<dyn Clock>);
        replay.put(&user(1), &key("k-1"), &fingerprint('a'), &result(10));
        clock.advance_secs(9 * 60 + 59);
        assert!(replay.get(&user(1), &key("k-1"), &fingerprint('a')).is_some());
        clock.advance_secs(2);
        assert!(replay.get(&user(1), &key("k-1"), &fingerprint('a')).is_none());
    }

    #[test]
    fn the_memory_is_bounded_by_weight() {
        let replay = Replay::new(clock());
        // 2,000 results of 4 KB are 8 MB, twice what the memory may hold: the oldest are gone, and nothing grows without limit.
        for n in 0..2_000_u32 {
            replay.put(
                &user(1),
                &key(&format!("k-{n}")),
                &fingerprint('a'),
                &result(4 * 1024),
            );
        }
        replay.run_pending_tasks();
        let kept = (0..2_000_u32)
            .filter(|n| {
                replay
                    .get(&user(1), &key(&format!("k-{n}")), &fingerprint('a'))
                    .is_some()
            })
            .count();
        assert!(kept > 0 && kept < 2_000, "{kept}");
        assert!(replay.weighted_size() <= MAX_BYTES, "{}", replay.weighted_size());
    }
}
