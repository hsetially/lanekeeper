//! A small bounded cache with a time to live, on the tokio clock.
//!
//! The Secret Manager source needs "at most five minutes" to be provable without waiting five minutes, which a cache
//! that reads the operating system's clock cannot give: this one reads `tokio::time::Instant`, so a test with a paused
//! clock moves it. It is bounded in entries (rule 5); when it is full it drops what has expired, then the oldest entry
//! (entries stored in the same instant leave in the order they were stored, so "oldest" never depends on hash order).
//! Values drop when they leave, which is when a `Secret` wipes itself.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

pub struct TtlCache<V> {
    state: Mutex<State<V>>,
    ttl: Duration,
    max_entries: usize,
}

struct State<V> {
    /// The value, when it was stored and the number of the insertion (which orders entries stored in the same instant).
    entries: HashMap<String, (V, Instant, u64)>,
    next_insertion: u64,
}

impl<V> fmt::Debug for TtlCache<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Sizes only: the values are secrets.
        f.debug_struct("TtlCache")
            .field("ttl", &self.ttl)
            .field("max_entries", &self.max_entries)
            .finish_non_exhaustive()
    }
}

impl<V> TtlCache<V> {
    #[must_use]
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            state: Mutex::new(State {
                entries: HashMap::new(),
                next_insertion: 0,
            }),
            ttl,
            max_entries: max_entries.max(1),
        }
    }

    /// Run `read` on the value for `key`, if there is one that has not expired. An expired entry is removed.
    pub fn get<R>(&self, key: &str, read: impl FnOnce(&V) -> R) -> Option<R> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        match state.entries.get(key) {
            Some((value, stored, _)) if now.saturating_duration_since(*stored) < self.ttl => {
                Some(read(value))
            }
            Some(_) => {
                state.entries.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn insert(&self, key: &str, value: V) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if !state.entries.contains_key(key) && state.entries.len() >= self.max_entries {
            state
                .entries
                .retain(|_, (_, stored, _)| now.saturating_duration_since(*stored) < self.ttl);
            if state.entries.len() >= self.max_entries {
                let oldest = state
                    .entries
                    .iter()
                    .min_by_key(|(_, (_, stored, insertion))| (*stored, *insertion))
                    .map(|(name, _)| name.clone());
                if let Some(oldest) = oldest {
                    state.entries.remove(&oldest);
                }
            }
        }
        let insertion = state.next_insertion;
        state.next_insertion = insertion.wrapping_add(1);
        state.entries.insert(key.to_owned(), (value, now, insertion));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn an_entry_lives_for_the_ttl_and_no_longer() {
        let cache = TtlCache::new(Duration::from_secs(300), 8);
        cache.insert("a", 1);
        tokio::time::advance(Duration::from_secs(299)).await;
        assert_eq!(cache.get("a", |v| *v), Some(1));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(cache.get("a", |v| *v), None, "gone at exactly the ttl");
        assert!(cache.is_empty(), "an expired entry is removed when it is found");
    }

    #[tokio::test(start_paused = true)]
    async fn the_cache_never_holds_more_than_its_bound() {
        let cache = TtlCache::new(Duration::from_secs(300), 3);
        for (i, name) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
            cache.insert(name, i);
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(cache.len() <= 3);
        }
        assert_eq!(cache.get("a", |v| *v), None, "the oldest went first");
        assert_eq!(cache.get("b", |v| *v), None);
        assert_eq!(cache.get("e", |v| *v), Some(4));
    }

    #[tokio::test(start_paused = true)]
    async fn entries_stored_in_the_same_instant_leave_in_the_order_they_were_stored() {
        // With a paused clock every insert has the same instant; the oldest must still be the first one stored, every time.
        for _ in 0..50 {
            let cache = TtlCache::new(Duration::from_secs(300), 2);
            for (i, name) in ["a", "b", "c"].into_iter().enumerate() {
                cache.insert(name, i);
            }
            assert_eq!(cache.get("a", |v| *v), None, "the first stored left first");
            assert_eq!(cache.get("b", |v| *v), Some(1));
            assert_eq!(cache.get("c", |v| *v), Some(2));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn expired_entries_leave_before_live_ones_when_the_cache_is_full() {
        let cache = TtlCache::new(Duration::from_secs(10), 2);
        cache.insert("old", 0);
        tokio::time::advance(Duration::from_secs(8)).await;
        cache.insert("young", 1);
        tokio::time::advance(Duration::from_secs(3)).await; // old is expired, young is not
        cache.insert("new", 2);
        assert_eq!(cache.get("young", |v| *v), Some(1));
        assert_eq!(cache.get("new", |v| *v), Some(2));
    }
}
