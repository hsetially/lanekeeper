//! Personal tokens (D52, S7): the second way to authenticate to `/mcp`, never combined with OAuth.
//!
//! A token is `lkpat_` plus 256 random bits, valid for 7 days, shown once when it is created and never listed. The store holds only
//! `HMAC-SHA-256(key, token)`. Every call looks the token up by that hash, compares it in constant time, and checks expiry and
//! revocation before anyone asks whether the user is still active.
//!
//! **Cache.** A lookup that found a live token is kept for at most five seconds (`TokenSettings::cache_ttl`), so a busy session does
//! not read the database on every call. Revoking evicts the entry at once on the replica that serves the revocation, and a replica
//! that did not hear of it stops accepting the token within the five seconds. A token that was not found, or was revoked or expired,
//! is never cached, so a flood of made-up tokens cannot push good entries out. Expiry is checked on every call, cached or not.

use std::sync::Arc;
use std::time::Duration;

use domain::{RequestId, Secret, Timestamp, UserId};
use moka::sync::Cache;
use ports::{AuditAction, AuditActor, AuditEvent, AuditLog, AuditVia, AuthError, SecretError, SecretSource};

use super::store::{Label, NewToken, RevokeOutcome, StoreError, TokenId, TokenRecord, TokenStore};
use super::token::{HmacKey, KeyError, PersonalToken, TokenHmac, generate};
use crate::clock::Clock;
use crate::error::McpError;

/// The name of the secret that holds the HMAC key (Secret Manager).
pub const KEY_SECRET_NAME: &str = "mcp-token-hmac-key";

/// How long a token lives.
pub const DEFAULT_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The longest life the schema allows (`mcp_tokens_lifetime`).
pub const MAX_LIFETIME: Duration = Duration::from_secs(31 * 24 * 60 * 60);
/// The most live tokens a user holds, unless configured otherwise.
pub const DEFAULT_MAX_LIVE: usize = 5;
/// How long a lookup is remembered.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(5);
/// The most lookups remembered at once.
pub const DEFAULT_CACHE_CAPACITY: u64 = 10_000;
/// The longest the key secret may take to read.
const SECRET_TIMEOUT: Duration = Duration::from_secs(10);

/// What the operator may tune. [`TokenSettings::validate`] says what is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSettings {
    /// How long a new token lives: 1 second to 31 days.
    pub lifetime: Duration,
    /// The most live tokens one user holds: 1 to 100.
    pub max_live: usize,
    /// How long a lookup is remembered: at most 60 seconds, and zero turns the cache off.
    pub cache_ttl: Duration,
    /// The most lookups remembered at once.
    pub cache_capacity: u64,
}

impl Default for TokenSettings {
    fn default() -> Self {
        Self {
            lifetime: DEFAULT_LIFETIME,
            max_live: DEFAULT_MAX_LIVE,
            cache_ttl: DEFAULT_CACHE_TTL,
            cache_capacity: DEFAULT_CACHE_CAPACITY,
        }
    }
}

/// A setting out of range. Names the setting only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the personal-token setting {0} is out of range")]
pub struct SettingsError(pub &'static str);

impl TokenSettings {
    /// # Errors
    ///
    /// [`SettingsError`] naming the first setting that is out of range.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.lifetime.is_zero() || self.lifetime > MAX_LIFETIME {
            return Err(SettingsError("lifetime"));
        }
        if !(1..=100).contains(&self.max_live) {
            return Err(SettingsError("max_live"));
        }
        if self.cache_ttl > Duration::from_secs(60) {
            return Err(SettingsError("cache_ttl"));
        }
        if self.cache_capacity == 0 || self.cache_capacity > 1_000_000 {
            return Err(SettingsError("cache_capacity"));
        }
        Ok(())
    }
}

/// Why a token operation failed. Maps onto [`McpError`] for the REST edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("the user already holds the most live tokens")]
    LimitReached,
    #[error("no such token")]
    NotFound,
    #[error("the token store is unavailable")]
    Unavailable,
    #[error("the token could not be made")]
    Internal,
}

impl From<TokenError> for McpError {
    fn from(error: TokenError) -> Self {
        match error {
            TokenError::LimitReached => Self::TokenLimit,
            TokenError::NotFound => Self::NotFound,
            TokenError::Unavailable => Self::Unavailable,
            TokenError::Internal => Self::Internal,
        }
    }
}

/// Why the key could not be loaded at start-up. The server does not start without it in personal-token mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyLoadError {
    #[error("the token key secret could not be read")]
    Secret(SecretError),
    #[error("the token key secret is not a usable key")]
    Key(KeyError),
    #[error("the token key secret took too long to read")]
    Timeout,
}

/// A token as it is shown once, when it is created.
pub struct CreatedToken {
    pub record: TokenRecord,
    /// The bearer token. Only ever sent in the response that creates it.
    pub token: Secret<String>,
}

impl std::fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedToken")
            .field("id", &self.record.id)
            .finish_non_exhaustive()
    }
}

struct Cached {
    record: TokenRecord,
    fetched_at: Timestamp,
}

/// Personal tokens: creating, listing, revoking and authenticating.
pub struct PersonalTokens {
    store: Arc<dyn TokenStore>,
    key: HmacKey,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
    settings: TokenSettings,
    cache: Cache<TokenHmac, Arc<Cached>>,
}

impl std::fmt::Debug for PersonalTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonalTokens")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl PersonalTokens {
    /// Tokens over `store`, with `key` already in hand. Settings are checked by the caller ([`TokenSettings::validate`]).
    #[must_use]
    pub fn new(
        store: Arc<dyn TokenStore>,
        key: HmacKey,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
        settings: TokenSettings,
    ) -> Self {
        let cache = Cache::builder()
            .max_capacity(settings.cache_capacity)
            // moka drops an entry that nobody asked for in a minute; freshness itself is judged against `Clock`.
            .time_to_idle(Duration::from_secs(60))
            .build();
        Self {
            store,
            key,
            audit,
            clock,
            settings,
            cache,
        }
    }

    /// Read the key from Secret Manager and build the service.
    ///
    /// # Errors
    ///
    /// [`KeyLoadError`] when the secret cannot be read, is not a key, or takes more than ten seconds.
    pub async fn load(
        store: Arc<dyn TokenStore>,
        secrets: &dyn SecretSource,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
        settings: TokenSettings,
    ) -> Result<Self, KeyLoadError> {
        let secret = tokio::time::timeout(SECRET_TIMEOUT, secrets.get(KEY_SECRET_NAME))
            .await
            .map_err(|_| KeyLoadError::Timeout)?
            .map_err(KeyLoadError::Secret)?;
        let key = HmacKey::from_secret_text(secret.expose()).map_err(KeyLoadError::Key)?;
        Ok(Self::new(store, key, audit, clock, settings))
    }

    #[must_use]
    pub fn settings(&self) -> &TokenSettings {
        &self.settings
    }

    /// Make a token for `owner` and write its row and its audit event in one transaction.
    ///
    /// # Errors
    ///
    /// [`TokenError::LimitReached`] when the owner already has the most live tokens, [`TokenError::Unavailable`] when the store or
    /// the audit log cannot write, [`TokenError::Internal`] when the random generator fails or the token cannot be hashed.
    pub async fn create(
        &self,
        owner: &UserId,
        label: Label,
        request_id: Option<RequestId>,
    ) -> Result<CreatedToken, TokenError> {
        let text = generate().map_err(|_| TokenError::Internal)?;
        let token = Secret::new(text);
        let parsed = PersonalToken::parse(token.expose()).ok_or(TokenError::Internal)?;
        let new_token = NewToken {
            owner: owner.clone(),
            hmac: self.key.lookup(&parsed).map_err(|_| TokenError::Internal)?,
            label,
            lifetime: self.settings.lifetime,
            max_live: self.settings.max_live,
        };
        let now = self.clock.now();
        let actor = owner.clone();
        let make = move |record: &TokenRecord| {
            audit_event(
                now,
                &actor,
                AuditAction::McpTokenCreated,
                format!(
                    "token={}; label={}; expires_at={}",
                    record.id,
                    record.label.as_str(),
                    record.expires_at.unix_millis()
                ),
                request_id.clone(),
            )
        };
        let record = self
            .store
            .create(new_token, self.audit.as_ref(), &make)
            .await
            .map_err(token_error)?;
        Ok(CreatedToken { record, token })
    }

    /// Revoke one of `owner`'s tokens. Revoking a token that is revoked already succeeds and changes nothing (no second event).
    ///
    /// # Errors
    ///
    /// [`TokenError::NotFound`] for a token that does not exist or belongs to someone else, [`TokenError::Unavailable`] when the
    /// store or the audit log cannot write.
    pub async fn revoke(
        &self,
        owner: &UserId,
        id: TokenId,
        request_id: Option<RequestId>,
    ) -> Result<(), TokenError> {
        let now = self.clock.now();
        let actor = owner.clone();
        let make = move |record: &TokenRecord| {
            audit_event(
                now,
                &actor,
                AuditAction::McpTokenRevoked,
                format!("token={}; label={}", record.id, record.label.as_str()),
                request_id.clone(),
            )
        };
        match self
            .store
            .revoke(owner, id, self.audit.as_ref(), &make)
            .await
            .map_err(token_error)?
        {
            RevokeOutcome::Revoked(record) => {
                // The replica that revoked stops accepting the token now. The others stop within the cache's time to live.
                self.cache.invalidate(&record.hmac);
                Ok(())
            }
            RevokeOutcome::AlreadyRevoked => Ok(()),
            RevokeOutcome::NotFound => Err(TokenError::NotFound),
        }
    }

    /// The owner's tokens, newest first.
    ///
    /// # Errors
    ///
    /// [`TokenError::Unavailable`] when the store cannot answer.
    pub async fn list(
        &self,
        owner: &UserId,
        before: Option<TokenId>,
        limit: usize,
    ) -> Result<Vec<TokenRecord>, TokenError> {
        self.store.list(owner, before, limit).await.map_err(token_error)
    }

    /// The user a presented token belongs to, if the token is one this server issued, has not expired and was not revoked. Whether
    /// the user is still active is the caller's next question.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidToken`] for anything that is not a live token (the same answer for a token that never existed and one
    /// that was revoked), [`AuthError::Expired`] for a token past its end, [`AuthError::Unavailable`] when the store cannot answer
    /// or the token cannot be hashed.
    pub async fn authenticate(&self, presented: &str) -> Result<UserId, AuthError> {
        let token = PersonalToken::parse(presented).ok_or(AuthError::InvalidToken)?;
        // A digest of the wrong length cannot be looked up: the token is refused, as an outage and not as a wrong token.
        let hmac = self.key.lookup(&token).map_err(|_| AuthError::Unavailable)?;
        let now = self.clock.now();
        let record = if let Some(hit) = self.fresh(&hmac, now) {
            hit
        } else {
            let found = self
                .store
                .find_for_auth(&hmac, now)
                .await
                .map_err(|_| AuthError::Unavailable)?
                .ok_or(AuthError::InvalidToken)?;
            // The row was found by this hash; compare again, in constant time, before believing it.
            if !found.hmac.ct_matches(&hmac) {
                return Err(AuthError::InvalidToken);
            }
            let found = Arc::new(Cached {
                record: found,
                fetched_at: now,
            });
            if found.record.is_live(now) && !self.settings.cache_ttl.is_zero() {
                self.cache.insert(hmac, Arc::clone(&found));
            }
            found
        };
        if record.record.revoked_at.is_some() {
            return Err(AuthError::InvalidToken);
        }
        if record.record.expires_at <= now {
            return Err(AuthError::Expired);
        }
        Ok(record.record.owner.clone())
    }

    /// A remembered lookup that is still fresh at `now`.
    fn fresh(&self, hmac: &TokenHmac, now: Timestamp) -> Option<Arc<Cached>> {
        let hit = self.cache.get(hmac)?;
        let age = now.unix_millis().checked_sub(hit.fetched_at.unix_millis())?;
        let ttl = i64::try_from(self.settings.cache_ttl.as_millis()).unwrap_or(i64::MAX);
        // A clock that went backwards (a negative age) is not fresh: ask the store.
        (0..ttl).contains(&age).then_some(hit)
    }
}

fn token_error(error: StoreError) -> TokenError {
    match error {
        StoreError::LimitReached => TokenError::LimitReached,
        StoreError::Unavailable => TokenError::Unavailable,
    }
}

/// One audit event of a token change. `diff` names the token and its label, and never the token or its hash (S7, S21).
fn audit_event(
    at: Timestamp,
    owner: &UserId,
    action: AuditAction,
    diff: String,
    request_id: Option<RequestId>,
) -> AuditEvent {
    let mut event = AuditEvent::new(at, AuditActor::User { user: owner.clone() }, action, AuditVia::Ui);
    event.diff = Some(diff);
    event.request_id = request_id;
    event
}
