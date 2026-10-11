//! "Does this Editor still need to store a GitHub token?" (`/me`, T2; S8 stays with the vault in prompt 06).
//!
//! The question is answered through a one-method trait, so prompt 06 can later back it with the vault itself. The Postgres
//! implementation asks whether a row exists and nothing else: it never selects a ciphertext, a wrapped key or a nonce.

use async_trait::async_trait;
use domain::{Role, User, UserId, UserStatus};
use ports::UserError;
use sqlx::PgPool;

use super::{bounded, db_err};

/// The only statement this module runs against `github_credentials`. A test checks the text, and runs it as a role that may
/// read nothing but the owner columns.
pub const PRESENCE_SQL: &str =
    "SELECT EXISTS (SELECT 1 FROM github_credentials WHERE user_tid = $1 AND user_oid = $2)";

#[async_trait]
pub trait GithubTokenPresence: Send + Sync + 'static {
    /// True when the user has stored a GitHub token. Says nothing about whether it still works.
    async fn has_token(&self, user: &UserId) -> Result<bool, UserError>;
}

#[derive(Clone)]
pub struct PgGithubTokenPresence {
    pool: PgPool,
}

impl std::fmt::Debug for PgGithubTokenPresence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgGithubTokenPresence").finish_non_exhaustive()
    }
}

impl PgGithubTokenPresence {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl GithubTokenPresence for PgGithubTokenPresence {
    async fn has_token(&self, user: &UserId) -> Result<bool, UserError> {
        let run = async {
            sqlx::query_scalar(PRESENCE_SQL)
                .bind(user.tid().as_str())
                .bind(user.oid().as_str())
                .fetch_one(&self.pool)
                .await
                .map_err(|e| db_err("github token presence", &e))
        };
        bounded("github_token_presence", run)
            .await
            .map_err(|_| UserError::Unavailable)
    }
}

/// True for an active Editor or above who has not stored a token yet: the web app then asks for one before the first write.
/// Pending and disabled users, and Viewers, are never asked.
pub async fn needs_github_token(user: &User, presence: &dyn GithubTokenPresence) -> Result<bool, UserError> {
    let can_write = user.status == UserStatus::Active && user.role.is_some_and(|r| r.at_least(Role::Editor));
    if !can_write {
        return Ok(false);
    }
    Ok(!presence.has_token(&user.id).await?)
}
