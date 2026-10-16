//! The statements of the token vault, one `github_credentials` row per user. The rows hold the wrapped data key, the nonce and the
//! ciphertext, and nothing that opens without KMS. Runtime-checked queries, like the other stores of this crate.

use bytes::Bytes;
use domain::{Timestamp, UserId};
use ports::{KeyRef, WrappedKey};
use sqlx::{PgConnection, Row};

use super::crypto::Sealed;
use crate::db::{ms, ts};
use crate::error::{Internal, Res};

/// A credential row, as stored.
pub(crate) struct Credential {
    pub id: i64,
    pub github_login: String,
    pub sealed: Sealed,
    pub expires_at: Option<Timestamp>,
    pub validated_at: Option<Timestamp>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("id", &self.id)
            .field("github_login", &self.github_login)
            .finish_non_exhaustive()
    }
}

pub(crate) const LOAD: &str = concat!(
    "SELECT id, github_login, kms_key_version, wrapped_dek, nonce, ciphertext, ",
    ms!("expires_at"),
    " AS expires_ms, ",
    ms!("last_validated_at"),
    " AS validated_ms FROM github_credentials WHERE user_tid = $1 AND user_oid = $2"
);

/// The user's row, locked until the transaction ends: concurrent saves for one user take turns.
pub(crate) const LOCK_USER: &str = "SELECT github_login FROM users WHERE tid = $1 AND oid = $2 FOR UPDATE";

/// The login linked to the user, read without a lock (the cheap refusal before GitHub is asked about the repositories).
pub(crate) const LINKED: &str = "SELECT github_login FROM users WHERE tid = $1 AND oid = $2";

/// The credential row of the user, made if there is none. The row of a first save holds nothing usable until [`SEAL`] fills it,
/// in the same transaction, so no other session ever reads the placeholder; the id it returns is what the associated data needs.
pub(crate) const RESERVE: &str = "INSERT INTO github_credentials \
     (user_tid, user_oid, github_login, wrapped_dek, kms_key_version, nonce, ciphertext) \
     VALUES ($1, $2, $3, '\\x'::bytea, 'pending', '\\x000000000000000000000000'::bytea, '\\x'::bytea) \
     ON CONFLICT (user_tid, user_oid) DO UPDATE SET github_login = EXCLUDED.github_login RETURNING id";

pub(crate) const SEAL: &str = concat!(
    "UPDATE github_credentials SET github_login = $2, wrapped_dek = $3, kms_key_version = $4, nonce = $5, ciphertext = $6, \
     expires_at = ",
    ts!("7"),
    ", last_validated_at = ",
    ts!("8"),
    " WHERE id = $1"
);

/// The login is linked on the first save and never changed by one.
pub(crate) const LINK_LOGIN: &str =
    "UPDATE users SET github_login = $3 WHERE tid = $1 AND oid = $2 AND github_login IS NULL";

pub(crate) const UNLINK_LOGIN: &str = "UPDATE users SET github_login = NULL WHERE tid = $1 AND oid = $2";

pub(crate) const DELETE: &str =
    "DELETE FROM github_credentials WHERE user_tid = $1 AND user_oid = $2 RETURNING id, github_login";

/// A page of credentials of users not seen for a while, in id order after `$1`: `$2` is the cut-off (milliseconds since the epoch),
/// `$3` the page size. A user who never signed in is measured from when the credential was made. Read through the primary key; the
/// user is found by its primary key.
pub(crate) const STALE: &str = concat!(
    "SELECT c.id, c.user_tid, c.user_oid FROM github_credentials c \
     JOIN users u ON u.tid = c.user_tid AND u.oid = c.user_oid \
     WHERE c.id > $1 AND coalesce(u.last_seen_at, c.created_at) < ",
    ts!("2"),
    " ORDER BY c.id LIMIT $3"
);

/// Removes one credential, if it is still the same old one: a user who signed in since keeps theirs.
pub(crate) const DELETE_IF_STALE: &str = concat!(
    "DELETE FROM github_credentials c USING users u \
     WHERE c.id = $1 AND u.tid = c.user_tid AND u.oid = c.user_oid \
     AND coalesce(u.last_seen_at, c.created_at) < ",
    ts!("2"),
    " RETURNING c.github_login"
);

pub(crate) const EXPIRING: &str = concat!(
    "SELECT c.id FROM github_credentials c WHERE c.id > $1 AND c.expires_at IS NOT NULL AND c.expires_at < ",
    ts!("2"),
    " ORDER BY c.id LIMIT $3"
);

pub(crate) fn queries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("credential_load", LOAD),
        ("credential_lock_user", LOCK_USER),
        ("credential_linked_login", LINKED),
        ("credential_stale", STALE),
        ("credential_expiring", EXPIRING),
    ]
}

type LoadRow = (
    i64,
    String,
    String,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Option<i64>,
    Option<i64>,
);

fn credential(row: LoadRow) -> Res<Credential> {
    let (id, github_login, key_version, wrapped, nonce, ciphertext, expires_ms, validated_ms) = row;
    let nonce: [u8; 12] = nonce
        .try_into()
        .map_err(|_| Internal::new("credential", "nonce"))?;
    Ok(Credential {
        id,
        github_login,
        sealed: Sealed {
            wrapped: WrappedKey {
                key_version: KeyRef::parse(&key_version)
                    .map_err(|_| Internal::new("credential", "key version"))?,
                ciphertext: Bytes::from(wrapped),
            },
            nonce,
            ciphertext,
        },
        expires_at: expires_ms.map(Timestamp::from_unix_millis),
        validated_at: validated_ms.map(Timestamp::from_unix_millis),
    })
}

/// The user's credential, if they have one.
pub(crate) async fn load(conn: &mut PgConnection, user: &UserId) -> Res<Option<Credential>> {
    let row: Option<LoadRow> = sqlx::query_as(LOAD)
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_load", &e))?;
    row.map(credential).transpose()
}

/// Lock the user's row and read the GitHub login linked to it. `None` when the user does not exist.
pub(crate) async fn lock_user(conn: &mut PgConnection, user: &UserId) -> Res<Option<Option<String>>> {
    let row = sqlx::query(LOCK_USER)
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_lock", &e))?;
    row.map(|r| r.try_get::<Option<String>, _>("github_login"))
        .transpose()
        .map_err(|e| Internal::db("credential_lock", &e))
}

/// The login linked to the user (outer `None`: no such user), read without locking the row.
pub(crate) async fn linked_login(conn: &mut PgConnection, user: &UserId) -> Res<Option<Option<String>>> {
    let row = sqlx::query(LINKED)
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_link", &e))?;
    row.map(|r| r.try_get::<Option<String>, _>("github_login"))
        .transpose()
        .map_err(|e| Internal::db("credential_link", &e))
}

pub(crate) async fn reserve(conn: &mut PgConnection, user: &UserId, login: &str) -> Res<i64> {
    sqlx::query_scalar(RESERVE)
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .bind(login)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_reserve", &e))
}

pub(crate) async fn seal(
    conn: &mut PgConnection,
    id: i64,
    login: &str,
    sealed: &Sealed,
    expires_at: Option<Timestamp>,
    validated_at: Timestamp,
) -> Res<()> {
    let done = sqlx::query(SEAL)
        .bind(id)
        .bind(login)
        .bind(sealed.wrapped.ciphertext.as_ref())
        .bind(sealed.wrapped.key_version.as_str())
        .bind(sealed.nonce.as_slice())
        .bind(sealed.ciphertext.as_slice())
        .bind(expires_at.map(Timestamp::unix_millis))
        .bind(validated_at.unix_millis())
        .execute(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_seal", &e))?;
    if done.rows_affected() == 1 {
        Ok(())
    } else {
        Err(Internal::new("credential_seal", "row missing"))
    }
}

pub(crate) async fn link_login(conn: &mut PgConnection, user: &UserId, login: &str) -> Res<()> {
    sqlx::query(LINK_LOGIN)
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .bind(login)
        .execute(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_link", &e))?;
    Ok(())
}

pub(crate) async fn unlink_login(conn: &mut PgConnection, user: &UserId) -> Res<()> {
    sqlx::query(UNLINK_LOGIN)
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .execute(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_unlink", &e))?;
    Ok(())
}

/// Delete the user's credential; the login it was for, if there was one.
pub(crate) async fn delete(conn: &mut PgConnection, user: &UserId) -> Res<Option<String>> {
    let row: Option<(i64, String)> = sqlx::query_as(DELETE)
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_delete", &e))?;
    Ok(row.map(|(_, login)| login))
}

/// A credential the cleanup job may delete.
pub(crate) struct Stale {
    pub id: i64,
    pub user: UserId,
}

pub(crate) async fn stale(
    conn: &mut PgConnection,
    after: i64,
    cutoff_ms: i64,
    limit: i64,
) -> Res<Vec<Stale>> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(STALE)
        .bind(after)
        .bind(cutoff_ms)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_stale", &e))?;
    rows.into_iter()
        .map(|(id, tid, oid)| {
            let user = format!("{tid}:{oid}")
                .parse::<UserId>()
                .map_err(|_| Internal::new("credential_stale", "user id"))?;
            Ok(Stale { id, user })
        })
        .collect()
}

pub(crate) async fn delete_if_stale(conn: &mut PgConnection, id: i64, cutoff_ms: i64) -> Res<Option<String>> {
    sqlx::query_scalar(DELETE_IF_STALE)
        .bind(id)
        .bind(cutoff_ms)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_delete_stale", &e))
}

/// Ids of credentials that expire before `before_ms`, after `after`, at most `limit`.
pub(crate) async fn expiring(
    conn: &mut PgConnection,
    after: i64,
    before_ms: i64,
    limit: i64,
) -> Res<Vec<i64>> {
    sqlx::query_scalar(EXPIRING)
        .bind(after)
        .bind(before_ms)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| Internal::db("credential_expiring", &e))
}
