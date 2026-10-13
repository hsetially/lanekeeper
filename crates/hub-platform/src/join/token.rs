//! The join-token credential (S5, fallback only): stored hashed, bound to one swimlane, valid for 24 hours, used once.
//!
//! The presented token is hashed with SHA-256 (`token_hash = SHA-256(token as UTF-8)`; whoever issues tokens stores the same)
//! and compared in constant time against every candidate hash of that swimlane, padded to a fixed number so that the work does
//! not depend on how many tokens exist. A token is consumed by one atomic `UPDATE ... WHERE used_at IS NULL`, in the same
//! transaction as the audit event, so two joins with one token cannot both succeed.

use domain::Secret;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use subtle::{Choice, ConditionallySelectable, ConstantTimeEq};
use zeroize::Zeroize;

/// A swimlane never has more than a handful of live tokens; this many are compared on every attempt (empty slots included).
pub const MAX_CANDIDATES: usize = 8;

/// A join token's lifetime, whatever `expires_at` says (S5): a token issued with a longer expiry is still refused after this.
const LIFETIME_SQL: &str = "interval '24 hours'";

/// The SHA-256 of a presented token. Zeroize it when done.
#[must_use]
pub fn hash_token(token: &Secret<String>) -> [u8; 32] {
    Sha256::digest(token.expose().as_bytes()).into()
}

/// Why no token matched. The reasons are not told to the caller (join failures are indistinguishable); this is for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("no live token of this swimlane matches")]
    NoMatch,
    #[error("the token store is unavailable")]
    Unavailable,
}

/// The id of the live token of `swimlane` whose hash equals `hash`.
pub async fn find_live(pool: &PgPool, swimlane: &str, hash: &[u8; 32]) -> Result<i64, TokenError> {
    let rows = sqlx::query(&format!(
        "SELECT id, token_hash::bytea AS token_hash FROM join_tokens \
         WHERE swimlane_id = $1 AND used_at IS NULL AND expires_at > now() AND created_at > now() - {LIFETIME_SQL} \
         ORDER BY id LIMIT {MAX_CANDIDATES}"
    ))
    .bind(swimlane)
    .fetch_all(pool)
    .await
    .map_err(|_| TokenError::Unavailable)?;

    // Every slot is compared, filled or not, and the match is chosen without a branch on it.
    let mut found = Choice::from(0);
    let mut matched = 0i64;
    for slot in 0..MAX_CANDIDATES {
        let (id, mut candidate) = match rows.get(slot) {
            Some(row) => (
                row.try_get::<i64, _>("id").unwrap_or(0),
                row.try_get::<Vec<u8>, _>("token_hash").unwrap_or_default(),
            ),
            None => (0, Vec::new()),
        };
        let equal = hash.as_slice().ct_eq(candidate.as_slice()) & Choice::from(u8::from(slot < rows.len()));
        matched = i64::conditional_select(&matched, &id, equal);
        found |= equal;
        candidate.zeroize();
    }
    if bool::from(found) {
        Ok(matched)
    } else {
        Err(TokenError::NoMatch)
    }
}

/// Mark token `id` used, inside the caller's transaction. False when someone else used it first or it expired meanwhile.
pub async fn consume(tx: &mut sqlx::PgConnection, id: i64) -> Result<bool, TokenError> {
    let used = sqlx::query(&format!(
        "UPDATE join_tokens SET used_at = now() \
         WHERE id = $1 AND used_at IS NULL AND expires_at > now() AND created_at > now() - {LIFETIME_SQL}"
    ))
    .bind(id)
    .execute(tx)
    .await
    .map_err(|_| TokenError::Unavailable)?;
    Ok(used.rows_affected() == 1)
}
