//! The GitHub token vault (T5, S8): each user's fine-grained token, stored encrypted and used only for that user's own requests.
//!
//! - **Saving** ([`TokenVault::put`]): the token must look like a fine-grained token (a classic one is refused, and the answer
//!   cites the policy). GitHub is asked whom it belongs to and whether it can push to both repositories. The first save links the
//!   GitHub login to the user; a later save for another login is refused until an admin resets the link
//!   ([`TokenVault::reset_github_link`]). Then, in one transaction, a fresh AES-256-GCM data key encrypts the token, KMS wraps the
//!   key with the user id and the credential id as associated data, and one audit event is recorded.
//! - **Using** ([`TokenVault::with_token`]): the token is decrypted inside the owner's request, handed to the caller's closure as
//!   a `Secret<String>`, and zeroized when the closure's future ends. There is no way to ask for another user's token: the owner is
//!   the user of the request.
//! - **Lifecycle** ([`CredentialCleanup`]): the expiry GitHub reports is stored, so a token that expires within a week can be
//!   flagged, and a leased nightly job deletes the credentials of users not seen for 30 days.
//!
//! A token never reaches a log, an error, an audit event or a response: the types here that touch it are `Secret`s, the errors are
//! fixed words, and the idempotency row of a save holds a digest of the request, not the token.

mod crypto;
mod lifecycle;
mod store;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use domain::{Role, Secret, ShortText, Timestamp, UserId, WriteCtx, WriteOutcome};
use ports::{AuditAction, AuditActor, AuditEvent, KmsEnvelope, Tx, WriteError};

pub use crypto::{CryptoError, associated_data};
pub use lifecycle::{CleanupReport, CredentialCleanup};

use crate::github::{GithubApi, GithubError, GithubUser, RepoRef, TokenShapeError, types::check_token_shape};
use crate::idempotency::RequestHash;
use crate::pipeline::approval::via_of;
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claimed, claim};
use crate::service::{Inner, WriteHub};

/// A token that expires within this many days is flagged (T5).
pub const EXPIRY_WARNING_DAYS: i64 = 7;

/// A credential of a user not seen for this many days is deleted by the nightly job (T5).
pub const UNSEEN_DAYS: i64 = 30;

const DAY_MS: i64 = 86_400_000;

/// Why a token operation did not happen. A fixed word each: nothing of the token, of GitHub's answer or of the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    /// The user may not hold a token (not active, or below Editor).
    #[error("not allowed")]
    Forbidden,
    /// The idempotency key was used for another request.
    #[error("idempotency key used for another request")]
    IdempotencyConflict,
    /// What was presented does not have the shape of a fine-grained token.
    #[error("the token is not a fine-grained token")]
    Shape(TokenShapeError),
    /// GitHub does not accept the token.
    #[error("github rejected the token")]
    Rejected,
    /// The token cannot push to one of the two repositories; the flags say which can.
    #[error("the token cannot push to both repositories")]
    NoPushAccess { base: bool, tenant: bool },
    /// The token belongs to another GitHub account than the one linked to the user.
    #[error("the token belongs to another github account")]
    LoginMismatch,
    /// The user has no stored token.
    #[error("no token is stored")]
    NoToken,
    /// The stored token has passed the expiry GitHub reported.
    #[error("the stored token has expired")]
    Expired,
    /// The stored credential does not decrypt (it was changed, or moved to another user).
    #[error("the stored token cannot be opened")]
    Undecryptable,
    /// GitHub did not answer in time or is rate limiting.
    #[error("github is unavailable")]
    GithubUnavailable,
    /// The store, KMS or the user directory did not answer.
    #[error("the token vault is unavailable")]
    Unavailable,
}

impl TokenError {
    fn from_write(e: WriteError) -> Self {
        match e {
            WriteError::Forbidden => Self::Forbidden,
            WriteError::IdempotencyConflict => Self::IdempotencyConflict,
            WriteError::NotFound | WriteError::BadRequest | WriteError::Unavailable => Self::Unavailable,
        }
    }

    fn from_github(e: GithubError) -> Self {
        match e {
            GithubError::Unauthorized => Self::Rejected,
            GithubError::RateLimited | GithubError::Unavailable => Self::GithubUnavailable,
            // A 403 or 404 on `GET /user`, a body that does not read, or a client that is not set up: not the user's to fix.
            GithubError::Forbidden
            | GithubError::NotFound
            | GithubError::Unexpected
            | GithubError::Config => {
                tracing::warn!("github gave an answer the token check cannot use");
                Self::Unavailable
            }
        }
    }

    fn from_crypto(e: CryptoError) -> Self {
        match e {
            CryptoError::Decrypt => Self::Undecryptable,
            CryptoError::Kms | CryptoError::Random => Self::Unavailable,
        }
    }
}

impl From<crate::error::Internal> for TokenError {
    fn from(_: crate::error::Internal) -> Self {
        Self::Unavailable
    }
}

/// What the vault tells the user about the token it holds. Never the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredToken {
    pub github_login: String,
    /// When GitHub last confirmed the token.
    pub verified_at: Timestamp,
    /// What was verified: `push:<owner>/<repo>` for each of the two repositories.
    pub scopes: Vec<String>,
    /// When the token expires, if GitHub said.
    pub expires_at: Option<Timestamp>,
    /// Whether the token expires within [`EXPIRY_WARNING_DAYS`] days.
    pub expires_soon: bool,
}

/// The two repositories a token must be able to push to (D41).
#[derive(Debug, Clone)]
pub struct PolicyRepos {
    /// `configuration-base-saas`.
    pub base: RepoRef,
    /// `csp-tenant-data`.
    pub tenant: RepoRef,
}

/// What the vault is built from, besides the write hub it belongs to.
pub struct VaultDeps {
    pub kms: Arc<dyn KmsEnvelope>,
    pub github: Arc<dyn GithubApi>,
    pub repos: PolicyRepos,
}

impl std::fmt::Debug for VaultDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultDeps").finish_non_exhaustive()
    }
}

/// The future a [`TokenVault::with_token`] closure returns. It may borrow the token it is given.
pub type TokenFuture<'a, R> = Pin<Box<dyn Future<Output = R> + Send + 'a>>;

/// The token routes call this. [`TokenVault`] implements it.
#[async_trait]
pub trait TokenDesk: Send + Sync + 'static {
    /// Store or replace the caller's token. Editor and above.
    async fn put_token(&self, ctx: &WriteCtx, token: Secret<String>) -> Result<StoredToken, TokenError>;

    /// Remove the caller's token. Done (and silent) when there is none. Editor and above.
    async fn delete_token(&self, ctx: &WriteCtx) -> Result<(), TokenError>;
}

/// The vault.
#[derive(Clone)]
pub struct TokenVault {
    hub: Arc<Inner>,
    kms: Arc<dyn KmsEnvelope>,
    github: Arc<dyn GithubApi>,
    repos: PolicyRepos,
}

impl std::fmt::Debug for TokenVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenVault").finish_non_exhaustive()
    }
}

impl WriteHub {
    /// The token vault over this hub's store, audit log and clock.
    #[must_use]
    pub fn token_vault(&self, deps: VaultDeps) -> TokenVault {
        TokenVault {
            hub: Arc::clone(&self.inner),
            kms: deps.kms,
            github: deps.github,
            repos: deps.repos,
        }
    }

    /// The nightly job that deletes the credentials of users not seen for [`UNSEEN_DAYS`] days. Run it under a lease (D63).
    #[must_use]
    pub fn credential_cleanup(&self) -> CredentialCleanup {
        CredentialCleanup::new(Arc::clone(&self.inner))
    }
}

/// The vault's statements by name, for the tests that run `EXPLAIN` on them (P12).
pub(crate) fn store_queries() -> Vec<(&'static str, &'static str)> {
    store::queries()
}

fn short(text: &str) -> Option<ShortText> {
    ShortText::parse(text).ok()
}

impl TokenVault {
    fn scopes(&self) -> Vec<String> {
        vec![
            format!("push:{}", self.repos.base),
            format!("push:{}", self.repos.tenant),
        ]
    }

    fn stored(&self, c: &store::Credential, now: Timestamp) -> StoredToken {
        let soon = c.expires_at.is_some_and(|at| {
            at.unix_millis().saturating_sub(now.unix_millis()) < EXPIRY_WARNING_DAYS * DAY_MS
        });
        StoredToken {
            github_login: c.github_login.clone(),
            verified_at: c.validated_at.unwrap_or(now),
            scopes: self.scopes(),
            expires_at: c.expires_at,
            expires_soon: soon,
        }
    }

    /// What the vault holds for the caller, without the token. Editor and above.
    pub async fn status(&self, ctx: &WriteCtx) -> Result<Option<StoredToken>, TokenError> {
        authorize(&self.hub, ctx, Role::Editor)
            .await
            .map_err(TokenError::from_write)?;
        self.load_status(&ctx.user.id).await
    }

    async fn load_status(&self, user: &UserId) -> Result<Option<StoredToken>, TokenError> {
        let hub = &*self.hub;
        let mut conn = hub
            .within("credential_load", hub.pool.acquire())
            .await?
            .map_err(|e| crate::error::Internal::db("credential_load", &e))?;
        let found = hub
            .within("credential_load", store::load(&mut conn, user))
            .await??;
        Ok(found.map(|c| self.stored(&c, hub.clock.now())))
    }

    /// Save the caller's token. See the module documentation for the steps.
    pub async fn put(&self, ctx: &WriteCtx, token: Secret<String>) -> Result<StoredToken, TokenError> {
        let hub = &*self.hub;
        authorize(hub, ctx, Role::Editor)
            .await
            .map_err(TokenError::from_write)?;
        // The shape is judged before anything is claimed, sent or stored: the same input always gets the same answer.
        check_token_shape(token.expose()).map_err(TokenError::Shape)?;
        let claim = match claim(hub, ctx, RequestHash::of_put_github_token(&token))
            .await
            .map_err(TokenError::from_write)?
        {
            Claimed::Replay(_) => {
                return self
                    .load_status(&ctx.user.id)
                    .await?
                    .ok_or(TokenError::Unavailable);
            }
            Claimed::Mine(claim) => claim,
        };
        let result = self.put_claimed(ctx, &claim, &token).await;
        if result.is_err() {
            // Nothing was stored: the key is free for the next try, after the user has fixed what was wrong.
            if !matches!(
                hub.within("release", hub.idem.release(&claim.key)).await,
                Ok(Ok(()))
            ) {
                tracing::warn!("could not release an idempotency claim");
            }
        }
        result
    }

    async fn put_claimed(
        &self,
        ctx: &WriteCtx,
        claim: &crate::pipeline::claim::Claim,
        token: &Secret<String>,
    ) -> Result<StoredToken, TokenError> {
        let hub = &*self.hub;
        let user = &ctx.user.id;
        let me = self.github.user(token).await.map_err(TokenError::from_github)?;

        // The cheap refusal first; the decisive check is the one made under the user's row lock below.
        let linked = {
            let mut conn = hub
                .within("credential_link", hub.pool.acquire())
                .await?
                .map_err(|e| crate::error::Internal::db("credential_link", &e))?;
            hub.within("credential_link", store::linked_login(&mut conn, user))
                .await??
        };
        match linked {
            None => return Err(TokenError::Unavailable),
            Some(Some(login)) if !me.login.same_as(&login) => return Err(TokenError::LoginMismatch),
            Some(_) => {}
        }

        let (base, tenant) = tokio::join!(
            self.github.can_push(token, &self.repos.base),
            self.github.can_push(token, &self.repos.tenant)
        );
        let (base, tenant) = (
            base.map_err(TokenError::from_github)?,
            tenant.map_err(TokenError::from_github)?,
        );
        if !(base && tenant) {
            return Err(TokenError::NoPushAccess { base, tenant });
        }

        hub.within("vault_put", self.store(ctx, claim, token, &me))
            .await?
    }

    /// The transaction of a save. The token is encrypted inside it because the credential id, which the associated data needs,
    /// is only known once the row is reserved; the call to KMS is the only slow step, and the whole transaction has a timeout.
    async fn store(
        &self,
        ctx: &WriteCtx,
        claim: &crate::pipeline::claim::Claim,
        token: &Secret<String>,
        me: &GithubUser,
    ) -> Result<StoredToken, TokenError> {
        let hub = &*self.hub;
        let user = &ctx.user.id;
        let mut tx = hub
            .pool
            .begin()
            .await
            .map_err(|e| crate::error::Internal::db("vault_put", &e))?;
        if crate::idempotency::lock_claim(&mut tx, &claim.key)
            .await?
            .is_some()
        {
            // Finished by somebody else meanwhile (a reconcile run): there is nothing left to record.
            let _ = tx.rollback().await;
            return self.load_status(user).await?.ok_or(TokenError::Unavailable);
        }
        let Some(linked) = store::lock_user(&mut tx, user).await? else {
            return Err(TokenError::Unavailable);
        };
        if let Some(login) = &linked {
            if !me.login.same_as(login) {
                return Err(TokenError::LoginMismatch);
            }
        }
        let id = store::reserve(&mut tx, user, me.login.as_str()).await?;
        let aad = associated_data(user, id);
        let sealed = crypto::seal(&*self.kms, token, &aad)
            .await
            .map_err(TokenError::from_crypto)?;
        let now = hub.clock.now();
        store::seal(&mut tx, id, me.login.as_str(), &sealed, me.expires_at, now).await?;
        if linked.is_none() {
            store::link_login(&mut tx, user, me.login.as_str()).await?;
        }

        let mut event = AuditEvent::new(
            now,
            AuditActor::User { user: user.clone() },
            AuditAction::GithubTokenStored,
            via_of(ctx.via),
        );
        event.github_login = short(me.login.as_str());
        event.request_id = Some(ctx.request_id.clone());
        let audit_id = hub
            .audit
            .record(&mut Tx::from_pg(&mut tx), event)
            .await
            .map_err(|_| crate::error::Internal::new("vault_put", "audit log"))?;
        let outcome = WriteOutcome::Applied {
            hash: None,
            audit_id,
            warnings: Vec::new(),
        };
        crate::idempotency::finish_in_tx(&mut tx, &claim.key, &outcome).await?;
        tx.commit()
            .await
            .map_err(|e| crate::error::Internal::db("vault_put", &e))?;

        let stored = StoredToken {
            github_login: me.login.as_str().to_owned(),
            verified_at: now,
            scopes: self.scopes(),
            expires_at: me.expires_at,
            expires_soon: me.expires_at.is_some_and(|at| {
                at.unix_millis().saturating_sub(now.unix_millis()) < EXPIRY_WARNING_DAYS * DAY_MS
            }),
        };
        if stored.expires_soon {
            tracing::warn!(user = %user, "a stored github token expires within a week");
        }
        Ok(stored)
    }

    /// Remove the caller's token. The GitHub login stays linked to the user (an admin resets that).
    pub async fn delete(&self, ctx: &WriteCtx) -> Result<(), TokenError> {
        let hub = &*self.hub;
        authorize(hub, ctx, Role::Editor)
            .await
            .map_err(TokenError::from_write)?;
        let claim = match claim(hub, ctx, RequestHash::of_delete_github_token())
            .await
            .map_err(TokenError::from_write)?
        {
            Claimed::Replay(_) => return Ok(()),
            Claimed::Mine(claim) => claim,
        };
        let result = hub.within("vault_delete", self.remove(ctx, &claim)).await?;
        // Something was deleted and recorded, together with the response of the key: done.
        if matches!(result, Ok(true)) {
            return Ok(());
        }
        // Nothing to delete (not a state change, so no audit event) or a failure: the key is free.
        if !matches!(
            hub.within("release", hub.idem.release(&claim.key)).await,
            Ok(Ok(()))
        ) {
            tracing::warn!("could not release an idempotency claim");
        }
        result.map(|_| ())
    }

    async fn remove(
        &self,
        ctx: &WriteCtx,
        claim: &crate::pipeline::claim::Claim,
    ) -> Result<bool, TokenError> {
        let hub = &*self.hub;
        let user = &ctx.user.id;
        let mut tx = hub
            .pool
            .begin()
            .await
            .map_err(|e| crate::error::Internal::db("vault_delete", &e))?;
        if crate::idempotency::lock_claim(&mut tx, &claim.key)
            .await?
            .is_some()
        {
            let _ = tx.rollback().await;
            return Ok(true);
        }
        let Some(login) = store::delete(&mut tx, user).await? else {
            let _ = tx.rollback().await;
            return Ok(false);
        };
        let mut event = AuditEvent::new(
            hub.clock.now(),
            AuditActor::User { user: user.clone() },
            AuditAction::GithubTokenDeleted,
            via_of(ctx.via),
        );
        event.github_login = short(&login);
        event.request_id = Some(ctx.request_id.clone());
        let audit_id = hub
            .audit
            .record(&mut Tx::from_pg(&mut tx), event)
            .await
            .map_err(|_| crate::error::Internal::new("vault_delete", "audit log"))?;
        let outcome = WriteOutcome::Applied {
            hash: None,
            audit_id,
            warnings: Vec::new(),
        };
        crate::idempotency::finish_in_tx(&mut tx, &claim.key, &outcome).await?;
        tx.commit()
            .await
            .map_err(|e| crate::error::Internal::db("vault_delete", &e))?;
        Ok(true)
    }

    /// Unlink a user's GitHub login and delete their token, so that they can link another account (T5; no route yet, plan A11).
    /// Admin only, audited as a deleted token with the user named in the event's text. Done (and silent) when the user has
    /// neither a link nor a token.
    pub async fn reset_github_link(&self, ctx: &WriteCtx, target: &UserId) -> Result<(), TokenError> {
        let hub = &*self.hub;
        authorize(hub, ctx, Role::Admin)
            .await
            .map_err(TokenError::from_write)?;
        hub.within("vault_reset", self.reset(ctx, target)).await?
    }

    async fn reset(&self, ctx: &WriteCtx, target: &UserId) -> Result<(), TokenError> {
        let hub = &*self.hub;
        let mut tx = hub
            .pool
            .begin()
            .await
            .map_err(|e| crate::error::Internal::db("vault_reset", &e))?;
        let Some(linked) = store::lock_user(&mut tx, target).await? else {
            return Err(TokenError::Unavailable);
        };
        let deleted = store::delete(&mut tx, target).await?;
        if linked.is_none() && deleted.is_none() {
            let _ = tx.rollback().await;
            return Ok(());
        }
        store::unlink_login(&mut tx, target).await?;
        let was = linked.or(deleted);
        let mut event = AuditEvent::new(
            hub.clock.now(),
            AuditActor::User {
                user: ctx.user.id.clone(),
            },
            AuditAction::GithubTokenDeleted,
            via_of(ctx.via),
        );
        event.github_login = was.as_deref().and_then(short);
        event.diff = Some(format!("reset the github link of user {target}"));
        event.request_id = Some(ctx.request_id.clone());
        hub.audit
            .record(&mut Tx::from_pg(&mut tx), event)
            .await
            .map_err(|_| crate::error::Internal::new("vault_reset", "audit log"))?;
        tx.commit()
            .await
            .map_err(|e| crate::error::Internal::db("vault_reset", &e))?;
        Ok(())
    }

    /// Run `f` with the caller's own token. The token is decrypted here, for this request and this user only (`ctx.user` is the
    /// owner; the directory is asked whether they may still act), handed to `f` and zeroized when `f`'s future ends, before this
    /// function returns. A token past its expiry is not decrypted.
    pub async fn with_token<R>(
        &self,
        ctx: &WriteCtx,
        f: impl for<'a> FnOnce(&'a Secret<String>) -> TokenFuture<'a, R>,
    ) -> Result<R, TokenError> {
        let hub = &*self.hub;
        authorize(hub, ctx, Role::Editor)
            .await
            .map_err(TokenError::from_write)?;
        let user = &ctx.user.id;
        let credential = {
            let mut conn = hub
                .within("credential_load", hub.pool.acquire())
                .await?
                .map_err(|e| crate::error::Internal::db("credential_load", &e))?;
            hub.within("credential_load", store::load(&mut conn, user))
                .await??
        };
        let Some(credential) = credential else {
            return Err(TokenError::NoToken);
        };
        if credential
            .expires_at
            .is_some_and(|at| at.unix_millis() <= hub.clock.now().unix_millis())
        {
            return Err(TokenError::Expired);
        }
        let aad = associated_data(user, credential.id);
        let token: Secret<String> = hub
            .within(
                "credential_open",
                crypto::open(&*self.kms, &credential.sealed, &aad),
            )
            .await?
            .map_err(TokenError::from_crypto)?;
        let out = f(&token).await;
        drop(token);
        Ok(out)
    }
}

#[async_trait]
impl TokenDesk for TokenVault {
    async fn put_token(&self, ctx: &WriteCtx, token: Secret<String>) -> Result<StoredToken, TokenError> {
        use tracing::Instrument;
        let span = tracing::info_span!("vault.put", request_id = %ctx.request_id, user = %ctx.user.id);
        self.put(ctx, token).instrument(span).await
    }

    async fn delete_token(&self, ctx: &WriteCtx) -> Result<(), TokenError> {
        use tracing::Instrument;
        let span = tracing::info_span!("vault.delete", request_id = %ctx.request_id, user = %ctx.user.id);
        self.delete(ctx).instrument(span).await
    }
}
