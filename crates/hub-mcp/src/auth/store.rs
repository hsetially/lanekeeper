//! Where personal tokens are kept (S7): the trait, its records, and an in-memory store for tests.
//!
//! The store holds `HMAC-SHA-256(key, token)` and never the token (`pat`). Creating or revoking a token is one unit of work with its
//! audit event: the store opens the transaction, writes the row, hands the same transaction to [`AuditLog::record`] last (the chain
//! lock is held to the commit), and commits, so the row and its one event exist together or not at all (rule 8, D80).

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use domain::{Timestamp, UserId};
use ports::{AuditEvent, AuditLog};

use super::token::TokenHmac;

/// The most characters a label may have (`mcp_tokens_label_length` in migration 0005).
pub const MAX_LABEL_CHARS: usize = 64;

/// The id of a token row: a positive number the database generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenId(i64);

impl TokenId {
    /// `None` for zero and negative numbers, which no row has.
    #[must_use]
    pub const fn new(id: i64) -> Option<Self> {
        if id >= 1 { Some(Self(id)) } else { None }
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Why a label was refused. Carries nothing of the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LabelError {
    #[error("the label is empty")]
    Empty,
    #[error("the label is longer than 64 characters")]
    TooLong,
    #[error("the label has a control character")]
    Control,
}

/// What the user calls a token ("laptop"): 1 to 64 characters, no control character, not only whitespace. It is shown in the list
/// and written to the audit event, so it is text a person reads, never a place to put a secret (the form says so).
#[derive(Clone, PartialEq, Eq)]
pub struct Label(String);

impl Label {
    pub fn parse(text: &str) -> Result<Self, LabelError> {
        if text.trim().is_empty() {
            return Err(LabelError::Empty);
        }
        // Count characters without walking an unbounded string: more than 64 bytes' worth of 4-byte characters is already too long.
        if text.len() > MAX_LABEL_CHARS * 4 || text.chars().count() > MAX_LABEL_CHARS {
            return Err(LabelError::TooLong);
        }
        if text.chars().any(char::is_control) {
            return Err(LabelError::Control);
        }
        Ok(Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Label({:?})", self.0)
    }
}

/// One row of the token table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRecord {
    pub id: TokenId,
    pub owner: UserId,
    pub hmac: TokenHmac,
    pub label: Label,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
    pub last_used_at: Option<Timestamp>,
}

impl TokenRecord {
    /// True at `now`: not revoked and not past its end.
    #[must_use]
    pub fn is_live(&self, now: Timestamp) -> bool {
        self.revoked_at.is_none() && self.expires_at > now
    }
}

/// A token to be written.
#[derive(Debug, Clone)]
pub struct NewToken {
    pub owner: UserId,
    pub hmac: TokenHmac,
    pub label: Label,
    /// How long the token lives, from the moment the store writes it.
    pub lifetime: Duration,
    /// The most live tokens the owner may hold, this one included.
    pub max_live: usize,
}

/// What a revocation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeOutcome {
    /// The token was live (or expired but not revoked) and is now revoked. One audit event was written.
    Revoked(TokenRecord),
    /// It was revoked already. Nothing changed and nothing was audited.
    AlreadyRevoked,
    /// No such token, or it is someone else's.
    NotFound,
}

/// Why the store could not do what was asked. Carries no input and no database text (S21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("the owner already holds the most live tokens")]
    LimitReached,
    #[error("the token store is unavailable")]
    Unavailable,
}

/// How the event of a unit of work is built once its row exists (the event names the row's id).
pub type MakeEvent<'a> = &'a (dyn Fn(&TokenRecord) -> AuditEvent + Send + Sync);

#[async_trait]
pub trait TokenStore: Send + Sync + 'static {
    /// Write a token and its audit event in one transaction. Refuses with [`StoreError::LimitReached`] when the owner already has
    /// `max_live` live tokens; the check and the insert are serialised per owner, so two requests at once cannot both pass it.
    async fn create(
        &self,
        new: NewToken,
        audit: &dyn AuditLog,
        event: MakeEvent<'_>,
    ) -> Result<TokenRecord, StoreError>;

    /// Revoke the owner's token `id` and write the audit event, in one transaction. Revoking twice is
    /// [`RevokeOutcome::AlreadyRevoked`], and writes nothing.
    async fn revoke(
        &self,
        owner: &UserId,
        id: TokenId,
        audit: &dyn AuditLog,
        event: MakeEvent<'_>,
    ) -> Result<RevokeOutcome, StoreError>;

    /// The owner's tokens, newest first, those with an id below `before` when it is given. At most `limit`.
    async fn list(
        &self,
        owner: &UserId,
        before: Option<TokenId>,
        limit: usize,
    ) -> Result<Vec<TokenRecord>, StoreError>;

    /// The token with this hash, revoked or expired ones included (the caller decides), and note that it was used: a live token's
    /// `last_used_at` is moved to `now` when it is more than a few seconds old, which keeps a busy token from writing on every call.
    async fn find_for_auth(
        &self,
        hmac: &TokenHmac,
        now: Timestamp,
    ) -> Result<Option<TokenRecord>, StoreError>;
}

/// How stale `last_used_at` may be before a lookup moves it, in milliseconds.
pub const TOUCH_AFTER_MILLIS: i64 = 10_000;

#[cfg(feature = "fakes")]
pub use memory::MemoryTokenStore;

#[cfg(feature = "fakes")]
mod memory {
    //! An in-memory [`TokenStore`](super::TokenStore) with the behaviour of the Postgres one that callers depend on: the live limit,
    //! one audit event per change in the same (fake) transaction, a revoked token that stays revoked, ownership, keyset order.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};

    use async_trait::async_trait;
    use domain::{Timestamp, UserId};
    use ports::fakes::FakeTxFactory;
    use ports::{AuditLog, TxFactory};

    use super::{
        MakeEvent, NewToken, RevokeOutcome, StoreError, TOUCH_AFTER_MILLIS, TokenId, TokenRecord, TokenStore,
    };
    use crate::auth::token::TokenHmac;
    use crate::clock::Clock;

    /// Most rows the fake holds.
    const MAX_ROWS: usize = 100_000;

    #[derive(Default)]
    struct State {
        next_id: i64,
        rows: Vec<TokenRecord>,
    }

    pub struct MemoryTokenStore {
        clock: Arc<dyn Clock>,
        state: Mutex<State>,
        /// Held for the whole of a creation or a revocation, as the advisory lock of the Postgres store is: two creations at once
        /// cannot both pass the limit.
        writer: tokio::sync::Mutex<()>,
        lookups: AtomicUsize,
    }

    impl std::fmt::Debug for MemoryTokenStore {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MemoryTokenStore").finish_non_exhaustive()
        }
    }

    impl MemoryTokenStore {
        #[must_use]
        pub fn new(clock: Arc<dyn Clock>) -> Self {
            Self {
                clock,
                state: Mutex::new(State::default()),
                writer: tokio::sync::Mutex::new(()),
                lookups: AtomicUsize::new(0),
            }
        }

        /// Every row, oldest first. For tests that look at what was stored.
        #[must_use]
        pub fn rows(&self) -> Vec<TokenRecord> {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .rows
                .clone()
        }

        /// How many times [`TokenStore::find_for_auth`] ran: a cache that works keeps this low.
        #[must_use]
        pub fn lookups(&self) -> usize {
            self.lookups.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl TokenStore for MemoryTokenStore {
        async fn create(
            &self,
            request: NewToken,
            audit: &dyn AuditLog,
            event: MakeEvent<'_>,
        ) -> Result<TokenRecord, StoreError> {
            let _writer = self.writer.lock().await;
            let now = self.clock.now();
            let lifetime = i64::try_from(request.lifetime.as_millis()).unwrap_or(i64::MAX);
            let record = {
                let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                if state.rows.len() >= MAX_ROWS {
                    return Err(StoreError::Unavailable);
                }
                let live = state
                    .rows
                    .iter()
                    .filter(|r| r.owner == request.owner && r.is_live(now))
                    .count();
                if live >= request.max_live {
                    return Err(StoreError::LimitReached);
                }
                TokenRecord {
                    id: TokenId::new(state.next_id + 1).ok_or(StoreError::Unavailable)?,
                    owner: request.owner,
                    hmac: request.hmac,
                    label: request.label,
                    created_at: now,
                    expires_at: Timestamp::from_unix_millis(now.unix_millis().saturating_add(lifetime)),
                    revoked_at: None,
                    last_used_at: None,
                }
            };
            let mut tx = FakeTxFactory::new()
                .begin()
                .await
                .map_err(|_| StoreError::Unavailable)?;
            audit
                .record(&mut tx.tx(), event(&record))
                .await
                .map_err(|_| StoreError::Unavailable)?;
            // Committing the audit event and inserting the row happen together; nothing in between can fail.
            {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                state.next_id = record.id.get();
                state.rows.push(record.clone());
            }
            tx.commit().await.map_err(|_| StoreError::Unavailable)?;
            Ok(record)
        }

        async fn revoke(
            &self,
            owner: &UserId,
            id: TokenId,
            audit: &dyn AuditLog,
            event: MakeEvent<'_>,
        ) -> Result<RevokeOutcome, StoreError> {
            let _writer = self.writer.lock().await;
            let now = self.clock.now();
            let updated = {
                let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                let Some(row) = state.rows.iter().find(|r| r.id == id && &r.owner == owner) else {
                    return Ok(RevokeOutcome::NotFound);
                };
                if row.revoked_at.is_some() {
                    return Ok(RevokeOutcome::AlreadyRevoked);
                }
                let mut row = row.clone();
                row.revoked_at = Some(now);
                row
            };
            let mut tx = FakeTxFactory::new()
                .begin()
                .await
                .map_err(|_| StoreError::Unavailable)?;
            audit
                .record(&mut tx.tx(), event(&updated))
                .await
                .map_err(|_| StoreError::Unavailable)?;
            {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(row) = state.rows.iter_mut().find(|r| r.id == id) {
                    row.revoked_at = updated.revoked_at;
                }
            }
            tx.commit().await.map_err(|_| StoreError::Unavailable)?;
            Ok(RevokeOutcome::Revoked(updated))
        }

        async fn list(
            &self,
            owner: &UserId,
            before: Option<TokenId>,
            limit: usize,
        ) -> Result<Vec<TokenRecord>, StoreError> {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let mut rows: Vec<TokenRecord> = state
                .rows
                .iter()
                .filter(|r| &r.owner == owner && before.is_none_or(|b| r.id < b))
                .cloned()
                .collect();
            rows.sort_by_key(|r| std::cmp::Reverse(r.id));
            rows.truncate(limit);
            Ok(rows)
        }

        async fn find_for_auth(
            &self,
            hmac: &TokenHmac,
            now: Timestamp,
        ) -> Result<Option<TokenRecord>, StoreError> {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(row) = state.rows.iter_mut().find(|r| r.hmac.ct_matches(hmac)) else {
                return Ok(None);
            };
            let found = row.clone();
            let needs_touch = row
                .last_used_at
                .is_none_or(|at| now.unix_millis() - at.unix_millis() > TOUCH_AFTER_MILLIS);
            if row.is_live(now) && needs_touch {
                row.last_used_at = Some(now);
            }
            Ok(Some(found))
        }
    }
}
