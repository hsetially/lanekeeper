//! T5 (S8): the GitHub token vault over a real Postgres. A token is checked with GitHub before it is stored, stored under a fresh
//! data key whose associated data is the owner and the row, opened only inside its owner's request, and removed when its owner
//! goes unseen; every change to the vault leaves exactly one chained audit event.
#![allow(clippy::too_many_lines)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use domain::{Role, Secret, Timestamp, User, UserStatus, WriteCtx};
use hub_writes::github::{GithubError, RepoRef, TokenShapeError};
use hub_writes::vault::{PolicyRepos, TokenDesk, TokenError, TokenFuture, TokenVault, VaultDeps};
use hub_writes::{WriteConfig, WriteHub};
use ports::fakes::FakeKmsEnvelope;
use ports::{AuditAction, AuditActor, AuditVia, KmsEnvelope, KmsError, WrappedKey, verify_chain};

use crate::support::fake_github::FakeGithub;
use crate::support::pg::TestDb;
use crate::support::rig::{Rig, START, ctx};

const BASE: &str = "acme/configuration-base-saas";
const TENANT: &str = "acme/csp-tenant-data";
const DAY_MS: i64 = 86_400_000;

/// A well-formed fine-grained token, distinct for each `n`. Not a real token.
fn tok(n: u32) -> String {
    format!("github_pat_11T{n:03}{}", "Ab1_".repeat(15))
}

fn secret(t: &str) -> Secret<String> {
    Secret::new(t.to_owned())
}

/// KMS with counters and a switch to make it fail.
struct CountingKms {
    inner: FakeKmsEnvelope,
    wraps: AtomicUsize,
    unwraps: AtomicUsize,
    down: AtomicBool,
}

impl CountingKms {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: FakeKmsEnvelope::new(),
            wraps: AtomicUsize::new(0),
            unwraps: AtomicUsize::new(0),
            down: AtomicBool::new(false),
        })
    }

    fn unwraps(&self) -> usize {
        self.unwraps.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl KmsEnvelope for CountingKms {
    async fn wrap(&self, dek: &Secret<[u8; 32]>, aad: &[u8]) -> Result<WrappedKey, KmsError> {
        self.wraps.fetch_add(1, Ordering::SeqCst);
        if self.down.load(Ordering::SeqCst) {
            return Err(KmsError::Unavailable);
        }
        self.inner.wrap(dek, aad).await
    }

    async fn unwrap(&self, w: &WrappedKey, aad: &[u8]) -> Result<Secret<[u8; 32]>, KmsError> {
        self.unwraps.fetch_add(1, Ordering::SeqCst);
        if self.down.load(Ordering::SeqCst) {
            return Err(KmsError::Unavailable);
        }
        self.inner.unwrap(w, aad).await
    }
}

struct Vaulted {
    rig: Rig,
    gh: FakeGithub,
    kms: Arc<CountingKms>,
    vault: TokenVault,
    editor: User,
}

fn vault_of(hub: &WriteHub, gh: &FakeGithub, kms: &Arc<CountingKms>) -> TokenVault {
    hub.token_vault(VaultDeps {
        kms: kms.clone(),
        github: Arc::new(gh.clone()),
        repos: PolicyRepos {
            base: RepoRef::parse(BASE).unwrap(),
            tenant: RepoRef::parse(TENANT).unwrap(),
        },
    })
}

async fn fixture_with(config: WriteConfig) -> Option<Vaulted> {
    let rig = Rig::with_config(TestDb::ready().await?, config).await;
    let gh = FakeGithub::default();
    let kms = CountingKms::new();
    let vault = vault_of(&rig.hub, &gh, &kms);
    let editor = rig.user(1, Role::Editor).await;
    // The editor's token: for `octocat`, who can push to both repositories.
    gh.token(&tok(1), "octocat", &[BASE, TENANT]);
    Some(Vaulted {
        rig,
        gh,
        kms,
        vault,
        editor,
    })
}

async fn fixture() -> Option<Vaulted> {
    fixture_with(WriteConfig::default()).await
}

impl Vaulted {
    fn ctx(&self, key: &str) -> WriteCtx {
        ctx(&self.editor, key)
    }

    async fn put(&self, key: &str, token: &str) -> Result<hub_writes::vault::StoredToken, TokenError> {
        self.vault.put(&self.ctx(key), secret(token)).await
    }

    /// The token as the closure of `with_token` sees it.
    async fn open_as(&self, who: &User, key: &str) -> Result<String, TokenError> {
        open(&self.vault, &ctx(who, key)).await
    }

    async fn rows(&self) -> i64 {
        self.rig.count("github_credentials").await
    }

    async fn events(&self, action: AuditAction) -> usize {
        self.rig
            .chain()
            .await
            .iter()
            .filter(|e| e.event.action == action)
            .count()
    }

    async fn linked(&self, who: &User) -> Option<String> {
        sqlx::query_scalar("SELECT github_login FROM users WHERE tid = $1 AND oid = $2")
            .bind(who.id.tid().as_str())
            .bind(who.id.oid().as_str())
            .fetch_one(&self.rig.pool)
            .await
            .unwrap()
    }
}

async fn open(vault: &TokenVault, c: &WriteCtx) -> Result<String, TokenError> {
    vault
        .with_token(c, |t| -> TokenFuture<'_, String> {
            Box::pin(async move { t.expose().clone() })
        })
        .await
}

// ------------------------------------------------------------------------------------------------------------- saving

#[tokio::test]
async fn a_token_is_checked_stored_encrypted_and_its_login_linked() {
    let Some(f) = fixture().await else { return };
    let stored = f.put("k-1", &tok(1)).await.unwrap();
    assert_eq!(stored.github_login, "octocat");
    assert_eq!(stored.scopes, [format!("push:{BASE}"), format!("push:{TENANT}")]);
    assert_eq!(stored.expires_at, None);
    assert!(!stored.expires_soon);
    assert_eq!(stored.verified_at, Timestamp::from_unix_millis(START));

    // GitHub was asked who the token is and about both repositories, once each.
    assert_eq!((f.gh.user_calls(), f.gh.push_calls()), (1, 2));
    assert_eq!(f.rows().await, 1);
    assert_eq!(f.linked(&f.editor).await.as_deref(), Some("octocat"));

    // The row holds ciphertext, a wrapped key and a nonce, and nothing that is, or contains, the token.
    let row: (Vec<u8>, Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT wrapped_dek, nonce, ciphertext FROM github_credentials")
            .fetch_one(&f.rig.pool)
            .await
            .unwrap();
    let t = tok(1);
    for (what, bytes) in [("wrapped key", &row.0), ("nonce", &row.1), ("ciphertext", &row.2)] {
        assert!(
            !bytes.windows(8).any(|w| t.as_bytes().windows(8).any(|p| p == w)),
            "the {what} holds a run of the token"
        );
    }
    assert_eq!(row.1.len(), 12);
    assert_eq!(row.2.len(), t.len() + 16);

    // And the owner gets the same token back, inside their request.
    assert_eq!(f.open_as(&f.editor, "o-1").await.unwrap(), t);
}

#[tokio::test]
async fn saving_a_token_writes_exactly_one_chained_audit_event_without_the_token() {
    let Some(f) = fixture().await else { return };
    f.put("k-1", &tok(1)).await.unwrap();
    let chain = f.rig.chain().await;
    verify_chain(&chain).unwrap();
    assert_eq!(chain.len(), 1);
    let e = &chain[0].event;
    assert_eq!(e.action, AuditAction::GithubTokenStored);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: f.editor.id.clone()
        }
    );
    assert_eq!(e.via, AuditVia::Ui);
    assert_eq!(
        e.github_login.as_ref().map(domain::ShortText::as_str),
        Some("octocat")
    );
    assert_eq!(
        e.request_id.as_ref().map(domain::RequestId::as_str),
        Some("req-k-1")
    );
    let json = serde_json::to_string(e).unwrap();
    assert!(!json.contains("github_pat_") && !json.contains(&tok(1)[11..]));
}

#[tokio::test]
async fn classic_token_is_rejected_before_github_is_asked() {
    let Some(f) = fixture().await else { return };
    for (classic, expect) in [
        (
            "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
            TokenShapeError::Classic,
        ),
        (
            "gho_0123456789abcdefghijklmnopqrstuvwxyz",
            TokenShapeError::Classic,
        ),
        (
            "0123456789abcdef0123456789abcdef01234567",
            TokenShapeError::Classic,
        ),
        ("not-a-token", TokenShapeError::NotFineGrained),
        ("", TokenShapeError::Empty),
    ] {
        let e = f.put("k-1", classic).await.unwrap_err();
        assert_eq!(e, TokenError::Shape(expect), "{classic}");
    }
    assert_eq!(
        (f.gh.user_calls(), f.gh.push_calls()),
        (0, 0),
        "GitHub was shown a token that cannot be accepted"
    );
    assert_eq!(
        (f.rows().await, f.events(AuditAction::GithubTokenStored).await),
        (0, 0)
    );
    // The key was not spent on a refusal.
    assert_eq!(f.rig.count("idempotency_keys").await, 0);
}

#[tokio::test]
async fn token_without_push_to_both_repos_is_rejected() {
    let Some(f) = fixture().await else { return };
    f.gh.token(&tok(2), "octocat", &[BASE]);
    f.gh.token(&tok(3), "octocat", &[TENANT]);
    f.gh.token(&tok(4), "octocat", &[]);
    for (n, base, tenant) in [(2, true, false), (3, false, true), (4, false, false)] {
        let e = f.put(&format!("k-{n}"), &tok(n)).await.unwrap_err();
        assert_eq!(e, TokenError::NoPushAccess { base, tenant }, "token {n}");
    }
    assert_eq!(f.rows().await, 0);
    assert_eq!(f.events(AuditAction::GithubTokenStored).await, 0);
    assert_eq!(f.linked(&f.editor).await, None, "a refused token linked a login");
}

#[tokio::test]
async fn a_token_github_does_not_accept_is_rejected() {
    let Some(f) = fixture().await else { return };
    // `tok(9)` is well formed and unknown to GitHub.
    assert_eq!(f.put("k-1", &tok(9)).await.unwrap_err(), TokenError::Rejected);
    f.gh.fail_next_user(GithubError::RateLimited);
    assert_eq!(
        f.put("k-2", &tok(1)).await.unwrap_err(),
        TokenError::GithubUnavailable
    );
    f.gh.fail_next_user(GithubError::Unavailable);
    assert_eq!(
        f.put("k-3", &tok(1)).await.unwrap_err(),
        TokenError::GithubUnavailable
    );
    f.gh.fail_next_push_check(GithubError::Unavailable);
    assert_eq!(
        f.put("k-4", &tok(1)).await.unwrap_err(),
        TokenError::GithubUnavailable
    );
    f.gh.fail_next_user(GithubError::Unexpected);
    assert_eq!(f.put("k-5", &tok(1)).await.unwrap_err(), TokenError::Unavailable);
    assert_eq!(f.rows().await, 0);
    assert_eq!(f.events(AuditAction::GithubTokenStored).await, 0);
    // The failed attempts were not stored as answers: the same keys work now.
    assert!(f.put("k-2", &tok(1)).await.is_ok());
}

#[tokio::test]
async fn mismatched_login_is_refused() {
    let Some(f) = fixture().await else { return };
    f.gh.token(&tok(2), "someone-else", &[BASE, TENANT]);
    f.gh.token(&tok(3), "OctoCat", &[BASE, TENANT]);
    f.put("k-1", &tok(1)).await.unwrap();

    // Another account's token: refused, and the first token is still the one that is stored.
    assert_eq!(
        f.put("k-2", &tok(2)).await.unwrap_err(),
        TokenError::LoginMismatch
    );
    assert_eq!(f.open_as(&f.editor, "o-1").await.unwrap(), tok(1));
    assert_eq!(f.linked(&f.editor).await.as_deref(), Some("octocat"));
    assert_eq!(f.events(AuditAction::GithubTokenStored).await, 1);

    // The same account, written with other capitals, is the same account (GitHub ignores the case of a login).
    f.put("k-3", &tok(3)).await.unwrap();
    assert_eq!(f.open_as(&f.editor, "o-2").await.unwrap(), tok(3));
    assert_eq!(f.rows().await, 1, "a replacement is the same row");
}

/// The link outlives the token: deleting it does not free the user to link another account (only an admin does that).
#[tokio::test]
async fn the_link_survives_deleting_the_token_and_an_admin_can_reset_it() {
    let Some(f) = fixture().await else { return };
    let admin = f.rig.user(2, Role::Admin).await;
    f.gh.token(&tok(2), "someone-else", &[BASE, TENANT]);
    f.put("k-1", &tok(1)).await.unwrap();
    f.vault.delete(&f.ctx("d-1")).await.unwrap();
    assert_eq!(f.linked(&f.editor).await.as_deref(), Some("octocat"));
    assert_eq!(
        f.put("k-2", &tok(2)).await.unwrap_err(),
        TokenError::LoginMismatch
    );

    // Not an editor's to do; an admin may.
    assert_eq!(
        f.vault
            .reset_github_link(&f.ctx("r-0"), &f.editor.id)
            .await
            .unwrap_err(),
        TokenError::Forbidden
    );
    f.vault
        .reset_github_link(&ctx(&admin, "r-1"), &f.editor.id)
        .await
        .unwrap();
    assert_eq!(f.linked(&f.editor).await, None);
    f.put("k-3", &tok(2)).await.unwrap();
    assert_eq!(f.linked(&f.editor).await.as_deref(), Some("someone-else"));

    // One event for the reset, naming the admin, and the user in its text.
    let chain = f.rig.chain().await;
    verify_chain(&chain).unwrap();
    let resets: Vec<_> = chain
        .iter()
        .filter(|e| matches!(&e.event.actor, AuditActor::User { user } if *user == admin.id))
        .collect();
    assert_eq!(resets.len(), 1);
    assert_eq!(resets[0].event.action, AuditAction::GithubTokenDeleted);
    assert!(
        resets[0]
            .event
            .diff
            .as_deref()
            .is_some_and(|d| d.contains(&f.editor.id.to_string()))
    );
}

#[tokio::test]
async fn resetting_a_link_that_does_not_exist_changes_nothing() {
    let Some(f) = fixture().await else { return };
    let admin = f.rig.user(2, Role::Admin).await;
    f.vault
        .reset_github_link(&ctx(&admin, "r-1"), &f.editor.id)
        .await
        .unwrap();
    assert!(f.rig.chain().await.is_empty(), "a no-op wrote an audit event");
}

/// Two saves for different accounts race: the user's row is locked, so one wins and the other is refused. No interleaving ends
/// with both stored or with a link that disagrees with the token.
#[tokio::test]
async fn racing_first_saves_for_two_accounts_link_exactly_one() {
    let Some(f) = fixture().await else { return };
    f.gh.token(&tok(2), "someone-else", &[BASE, TENANT]);
    let (octocat, other) = (tok(1), tok(2));
    let (first, second) = tokio::join!(f.put("k-a", &octocat), f.put("k-b", &other));
    let (winner, loser) = match (first, second) {
        (Ok(won), Err(lost)) | (Err(lost), Ok(won)) => (won, lost),
        other => panic!("expected exactly one save to win, got {other:?}"),
    };
    assert_eq!(loser, TokenError::LoginMismatch);
    assert_eq!(
        f.linked(&f.editor).await.as_deref(),
        Some(winner.github_login.as_str())
    );
    assert_eq!(f.rows().await, 1);
    assert_eq!(f.events(AuditAction::GithubTokenStored).await, 1);
    let held = f.open_as(&f.editor, "o-1").await.unwrap();
    assert_eq!(
        held,
        if winner.github_login == "octocat" {
            tok(1)
        } else {
            tok(2)
        }
    );
}

// ------------------------------------------------------------------------------------------------------ replacing, keys

#[tokio::test]
async fn fresh_data_key_per_token_in_the_stored_rows() {
    let Some(f) = fixture().await else { return };
    f.put("k-1", &tok(1)).await.unwrap();
    let first: (i64, Vec<u8>, Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT id, wrapped_dek, nonce, ciphertext FROM github_credentials")
            .fetch_one(&f.rig.pool)
            .await
            .unwrap();
    // The same token again, under another key: nothing of the first row is reused.
    f.put("k-2", &tok(1)).await.unwrap();
    let second: (i64, Vec<u8>, Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT id, wrapped_dek, nonce, ciphertext FROM github_credentials")
            .fetch_one(&f.rig.pool)
            .await
            .unwrap();
    assert_eq!(first.0, second.0, "a replacement keeps its row");
    assert_ne!(first.1, second.1, "the wrapped key was reused");
    assert_ne!(first.2, second.2, "the nonce was reused");
    assert_ne!(first.3, second.3, "the ciphertext was reused");
    assert_eq!(
        f.kms.wraps.load(Ordering::SeqCst),
        2,
        "one wrapping for each token"
    );
    assert_eq!(f.events(AuditAction::GithubTokenStored).await, 2);
}

/// S8: a stored credential opens only for the user it was made for. The row is moved to another user, with everything in it.
#[tokio::test]
async fn ciphertext_moved_to_another_user_fails_decryption() {
    let Some(f) = fixture().await else { return };
    let other = f.rig.user(2, Role::Editor).await;
    f.put("k-1", &tok(1)).await.unwrap();
    assert_eq!(f.open_as(&f.editor, "o-1").await.unwrap(), tok(1));

    sqlx::query("UPDATE github_credentials SET user_tid = $1, user_oid = $2")
        .bind(other.id.tid().as_str())
        .bind(other.id.oid().as_str())
        .execute(&f.rig.pool)
        .await
        .unwrap();
    assert_eq!(
        f.open_as(&other, "o-2").await.unwrap_err(),
        TokenError::Undecryptable
    );
    assert_eq!(
        f.open_as(&f.editor, "o-3").await.unwrap_err(),
        TokenError::NoToken
    );
    // Moved back, it opens again: it was the owner that mattered.
    sqlx::query("UPDATE github_credentials SET user_tid = $1, user_oid = $2")
        .bind(f.editor.id.tid().as_str())
        .bind(f.editor.id.oid().as_str())
        .execute(&f.rig.pool)
        .await
        .unwrap();
    assert_eq!(f.open_as(&f.editor, "o-4").await.unwrap(), tok(1));
}

#[tokio::test]
async fn swapped_sealed_columns_between_users_fail_decryption_and_restoring_them_opens() {
    let Some(f) = fixture().await else { return };
    let other = f.rig.user(2, Role::Editor).await;
    f.gh.token(&tok(2), "hubot", &[BASE, TENANT]);
    f.put("k-1", &tok(1)).await.unwrap();
    f.vault.put(&ctx(&other, "k-2"), secret(&tok(2))).await.unwrap();

    let swap = || async {
        // The sealed columns of the two rows trade places; the owners and the ids stay.
        sqlx::query(
            "UPDATE github_credentials a SET wrapped_dek = b.wrapped_dek, kms_key_version = b.kms_key_version, \
             nonce = b.nonce, ciphertext = b.ciphertext \
             FROM github_credentials b WHERE a.id <> b.id",
        )
        .execute(&f.rig.pool)
        .await
        .unwrap();
    };
    swap().await;
    assert_eq!(
        f.open_as(&f.editor, "o-1").await.unwrap_err(),
        TokenError::Undecryptable
    );
    assert_eq!(
        f.open_as(&other, "o-2").await.unwrap_err(),
        TokenError::Undecryptable
    );
    // Swapping back restores both: the failure was the binding, not damage.
    swap().await;
    assert_eq!(f.open_as(&f.editor, "o-3").await.unwrap(), tok(1));
    assert_eq!(f.open_as(&other, "o-4").await.unwrap(), tok(2));
}

/// The credential id is part of the associated data: the same bytes under a new row id do not open.
#[tokio::test]
async fn the_same_bytes_under_another_credential_id_fail_decryption() {
    let Some(f) = fixture().await else { return };
    f.put("k-1", &tok(1)).await.unwrap();
    let mut tx = f.rig.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO github_credentials (user_tid, user_oid, github_login, wrapped_dek, kms_key_version, nonce, ciphertext) \
         SELECT user_tid, user_oid, github_login, wrapped_dek, kms_key_version, nonce, ciphertext FROM github_credentials",
    )
    .execute(&mut *tx)
    .await
    .expect_err("a second row for one user must not be possible");
    tx.rollback().await.unwrap();
    // Delete and re-insert: the same bytes, a new id.
    let mut tx = f.rig.pool.begin().await.unwrap();
    sqlx::query(
        "CREATE TEMP TABLE parked AS SELECT user_tid, user_oid, github_login, wrapped_dek, kms_key_version, nonce, ciphertext \
         FROM github_credentials",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("DELETE FROM github_credentials")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO github_credentials (user_tid, user_oid, github_login, wrapped_dek, kms_key_version, nonce, ciphertext) \
         SELECT user_tid, user_oid, github_login, wrapped_dek, kms_key_version, nonce, ciphertext FROM parked",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        f.open_as(&f.editor, "o-1").await.unwrap_err(),
        TokenError::Undecryptable
    );
}

#[tokio::test]
async fn a_changed_stored_byte_fails_decryption() {
    let Some(f) = fixture().await else { return };
    f.put("k-1", &tok(1)).await.unwrap();
    sqlx::query(
        "UPDATE github_credentials SET ciphertext = set_byte(ciphertext, 3, get_byte(ciphertext, 3) # 1)",
    )
    .execute(&f.rig.pool)
    .await
    .unwrap();
    assert_eq!(
        f.open_as(&f.editor, "o-1").await.unwrap_err(),
        TokenError::Undecryptable
    );
}

// ----------------------------------------------------------------------------------------------------- idempotent saves

#[tokio::test]
async fn the_same_key_and_token_replay_without_a_second_write() {
    let Some(f) = fixture().await else { return };
    let first = f.put("k-1", &tok(1)).await.unwrap();
    let again = f.put("k-1", &tok(1)).await.unwrap();
    assert_eq!(first.github_login, again.github_login);
    assert_eq!(
        (f.gh.user_calls(), f.gh.push_calls()),
        (1, 2),
        "the replay asked GitHub again"
    );
    assert_eq!(f.kms.wraps.load(Ordering::SeqCst), 1);
    assert_eq!(f.events(AuditAction::GithubTokenStored).await, 1);
}

#[tokio::test]
async fn the_same_key_with_another_token_is_a_conflict_and_changes_nothing() {
    let Some(f) = fixture().await else { return };
    f.gh.token(&tok(2), "octocat", &[BASE, TENANT]);
    f.put("k-1", &tok(1)).await.unwrap();
    assert_eq!(
        f.put("k-1", &tok(2)).await.unwrap_err(),
        TokenError::IdempotencyConflict
    );
    assert_eq!(f.open_as(&f.editor, "o-1").await.unwrap(), tok(1));
    assert_eq!(f.events(AuditAction::GithubTokenStored).await, 1);
    // The row of the key holds a digest of the request and the stored answer, not the token.
    let rows: Vec<(Vec<u8>, Option<Vec<u8>>)> =
        sqlx::query_as("SELECT request_hash, response_body FROM idempotency_keys")
            .fetch_all(&f.rig.pool)
            .await
            .unwrap();
    assert!(!rows.is_empty());
    for (hash, body) in rows {
        let t = tok(1);
        assert_eq!(hash.len(), 32);
        assert!(!hash.windows(8).any(|w| t.as_bytes().windows(8).any(|p| p == w)));
        let text = body
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        assert!(!text.contains("github_pat_") && !text.contains(&t[11..]));
    }
}

// ----------------------------------------------------------------------------------------------- what stops a save, whole

#[tokio::test]
async fn a_kms_failure_stores_nothing_not_even_a_placeholder() {
    let Some(f) = fixture().await else { return };
    f.kms.down.store(true, Ordering::SeqCst);
    assert_eq!(f.put("k-1", &tok(1)).await.unwrap_err(), TokenError::Unavailable);
    assert_eq!(f.rows().await, 0, "a half-made credential was left behind");
    assert_eq!(f.linked(&f.editor).await, None);
    assert!(f.rig.chain().await.is_empty());
    // KMS back: the same key is free and the save goes through.
    f.kms.down.store(false, Ordering::SeqCst);
    assert!(f.put("k-1", &tok(1)).await.is_ok());
}

#[tokio::test]
async fn an_audit_failure_rolls_the_whole_save_back() {
    let Some(f) = fixture().await else { return };
    f.rig.audit.fail_next();
    assert_eq!(f.put("k-1", &tok(1)).await.unwrap_err(), TokenError::Unavailable);
    assert_eq!(
        f.rows().await,
        0,
        "a credential was stored without its audit event"
    );
    assert_eq!(
        f.linked(&f.editor).await,
        None,
        "a login was linked without its audit event"
    );
    assert!(f.put("k-1", &tok(1)).await.is_ok());
    assert_eq!(f.events(AuditAction::GithubTokenStored).await, 1);
}

#[tokio::test]
async fn a_failed_replacement_leaves_the_old_token_working() {
    let Some(f) = fixture().await else { return };
    f.gh.token(&tok(2), "octocat", &[BASE, TENANT]);
    f.put("k-1", &tok(1)).await.unwrap();
    f.kms.down.store(true, Ordering::SeqCst);
    assert!(f.put("k-2", &tok(2)).await.is_err());
    f.kms.down.store(false, Ordering::SeqCst);
    assert_eq!(f.open_as(&f.editor, "o-1").await.unwrap(), tok(1));
}

// ------------------------------------------------------------------------------------------------------------- using

#[tokio::test]
async fn decrypt_happens_only_inside_the_owners_request() {
    let Some(f) = fixture().await else { return };
    let other = f.rig.user(2, Role::Editor).await;
    f.put("k-1", &tok(1)).await.unwrap();
    assert_eq!(f.kms.unwraps(), 0, "saving opened a token");

    // Somebody else asks: they have no token, and nothing was decrypted for them (or for anyone).
    assert_eq!(f.open_as(&other, "o-1").await.unwrap_err(), TokenError::NoToken);
    assert_eq!(f.kms.unwraps(), 0);
    // The status is not a use.
    assert!(f.vault.status(&f.ctx("s-1")).await.unwrap().is_some());
    assert_eq!(f.kms.unwraps(), 0);
    // The owner's request decrypts once.
    assert_eq!(f.open_as(&f.editor, "o-2").await.unwrap(), tok(1));
    assert_eq!(f.kms.unwraps(), 1);
}

/// `ctx.user` is a snapshot from sign-in; the directory decides who may act now (S4).
#[tokio::test]
async fn a_stale_session_snapshot_does_not_open_a_token() {
    let Some(f) = fixture().await else { return };
    let viewer = f.rig.user(2, Role::Viewer).await;
    f.put("k-1", &tok(1)).await.unwrap();
    let before = f.kms.unwraps();
    // The directory says Viewer; the session still says Editor.
    let mut snapshot = viewer.clone();
    snapshot.role = Some(Role::Editor);
    assert_eq!(
        f.open_as(&snapshot, "o-1").await.unwrap_err(),
        TokenError::Forbidden
    );
    assert_eq!(f.kms.unwraps(), before, "a refused request decrypted something");
}

#[tokio::test]
async fn an_expired_token_is_not_decrypted() {
    let Some(f) = fixture().await else { return };
    f.gh.expires(&tok(1), Timestamp::from_unix_millis(START + 2 * DAY_MS));
    f.put("k-1", &tok(1)).await.unwrap();
    assert!(f.open_as(&f.editor, "o-1").await.is_ok());
    let unwraps = f.kms.unwraps();
    f.rig.advance_secs(2 * 86_400);
    assert_eq!(
        f.open_as(&f.editor, "o-2").await.unwrap_err(),
        TokenError::Expired
    );
    assert_eq!(f.kms.unwraps(), unwraps, "an expired token was decrypted");
    // Replacing it brings it back.
    f.gh.token(&tok(2), "octocat", &[BASE, TENANT]);
    f.put("k-2", &tok(2)).await.unwrap();
    assert_eq!(f.open_as(&f.editor, "o-3").await.unwrap(), tok(2));
}

#[tokio::test]
async fn the_vault_answers_no_token_after_a_delete() {
    let Some(f) = fixture().await else { return };
    assert_eq!(f.vault.status(&f.ctx("s-0")).await.unwrap(), None);
    assert_eq!(
        f.open_as(&f.editor, "o-0").await.unwrap_err(),
        TokenError::NoToken
    );
    f.put("k-1", &tok(1)).await.unwrap();
    f.vault.delete(&f.ctx("d-1")).await.unwrap();
    assert_eq!(
        f.open_as(&f.editor, "o-1").await.unwrap_err(),
        TokenError::NoToken
    );
    assert_eq!(f.vault.status(&f.ctx("s-1")).await.unwrap(), None);
}

// ---------------------------------------------------------------------------------------------------------- the expiry

#[tokio::test]
async fn an_expiry_within_a_week_is_flagged() {
    let Some(f) = fixture().await else { return };
    for (token, days, soon) in [(2, 3, true), (3, 6, true), (4, 8, false), (5, 90, false)] {
        f.gh.token(&tok(token), "octocat", &[BASE, TENANT]);
        f.gh.expires(&tok(token), Timestamp::from_unix_millis(START + days * DAY_MS));
        let stored = f.put(&format!("k-{token}"), &tok(token)).await.unwrap();
        assert_eq!(stored.expires_soon, soon, "{days} days");
        assert_eq!(
            stored.expires_at,
            Some(Timestamp::from_unix_millis(START + days * DAY_MS))
        );
        // What the status says later agrees with what the save said.
        let status = f
            .vault
            .status(&f.ctx(&format!("s-{token}")))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (status.expires_soon, status.expires_at),
            (soon, stored.expires_at)
        );
    }
    // Time passes: a token that was far off is soon.
    f.rig.advance_secs(85 * 86_400);
    assert!(
        f.vault
            .status(&f.ctx("s-late"))
            .await
            .unwrap()
            .unwrap()
            .expires_soon
    );
}

// --------------------------------------------------------------------------------------------------------------- delete

#[tokio::test]
async fn deleting_removes_the_credential_with_one_audit_event_and_is_silent_when_there_is_none() {
    let Some(f) = fixture().await else { return };
    // Nothing to delete: done, no event, and the key is not spent.
    f.vault.delete(&f.ctx("d-0")).await.unwrap();
    assert!(f.rig.chain().await.is_empty());

    f.put("k-1", &tok(1)).await.unwrap();
    f.vault.delete(&f.ctx("d-1")).await.unwrap();
    assert_eq!(f.rows().await, 0);
    let chain = f.rig.chain().await;
    verify_chain(&chain).unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[1].event.action, AuditAction::GithubTokenDeleted);
    assert_eq!(
        chain[1]
            .event
            .github_login
            .as_ref()
            .map(domain::ShortText::as_str),
        Some("octocat")
    );

    // The same key again replays: no second event.
    f.vault.delete(&f.ctx("d-1")).await.unwrap();
    assert_eq!(f.rig.chain().await.len(), 2);
    // Another key with nothing left to delete: still no event.
    f.vault.delete(&f.ctx("d-2")).await.unwrap();
    assert_eq!(f.rig.chain().await.len(), 2);
}

#[tokio::test]
async fn a_delete_key_cannot_be_reused_for_a_save() {
    let Some(f) = fixture().await else { return };
    f.put("k-1", &tok(1)).await.unwrap();
    f.vault.delete(&f.ctx("d-1")).await.unwrap();
    assert_eq!(
        f.put("d-1", &tok(1)).await.unwrap_err(),
        TokenError::IdempotencyConflict
    );
    assert_eq!(f.rows().await, 0);
}

// ----------------------------------------------------------------------------------------------------------- who may

#[tokio::test]
async fn only_an_active_editor_or_better_holds_a_token() {
    let Some(f) = fixture().await else { return };
    let denied = [
        f.rig.user(10, Role::Viewer).await,
        f.rig.user_with_status(11, None, UserStatus::Active).await,
        f.rig
            .user_with_status(12, Some(Role::Editor), UserStatus::Pending)
            .await,
        f.rig
            .user_with_status(13, Some(Role::Admin), UserStatus::Disabled)
            .await,
    ];
    for who in &denied {
        let c = ctx(who, "k-x");
        assert_eq!(
            f.vault.put(&c, secret(&tok(1))).await.unwrap_err(),
            TokenError::Forbidden,
            "{who:?}"
        );
        assert_eq!(f.vault.delete(&c).await.unwrap_err(), TokenError::Forbidden);
        assert_eq!(f.vault.status(&c).await.unwrap_err(), TokenError::Forbidden);
        assert_eq!(open(&f.vault, &c).await.unwrap_err(), TokenError::Forbidden);
    }
    // A user the directory does not know is refused too.
    let stranger = ports::conformance::sample::active(99, Role::Admin);
    assert_eq!(
        f.vault
            .put(&ctx(&stranger, "k-y"), secret(&tok(1)))
            .await
            .unwrap_err(),
        TokenError::Forbidden
    );
    assert_eq!(
        (f.gh.user_calls(), f.gh.push_calls()),
        (0, 0),
        "GitHub was asked on behalf of a refused user"
    );
    assert_eq!(f.rows().await, 0);
    assert!(f.rig.chain().await.is_empty());
    assert_eq!(
        f.rig.count("idempotency_keys").await,
        0,
        "a refused user spent a key"
    );

    for (n, role) in [(20, Role::Editor), (21, Role::Operator), (22, Role::Admin)] {
        let who = f.rig.user(n, role).await;
        f.gh.token(&tok(n), &format!("user{n}"), &[BASE, TENANT]);
        f.vault.put(&ctx(&who, "k-ok"), secret(&tok(n))).await.unwrap();
    }
    assert_eq!(f.rows().await, 3);
}

#[tokio::test]
async fn the_desk_trait_is_the_vault() {
    let Some(f) = fixture().await else { return };
    let desk: &dyn TokenDesk = &f.vault;
    let stored = desk.put_token(&f.ctx("k-1"), secret(&tok(1))).await.unwrap();
    assert_eq!(stored.github_login, "octocat");
    desk.delete_token(&f.ctx("d-1")).await.unwrap();
    assert_eq!(f.rows().await, 0);
}

// ------------------------------------------------------------------------------------------------------------- cleanup

/// Set when a user was last seen, in days before the test clock.
async fn seen(f: &Vaulted, who: &User, days_ago: i64) {
    sqlx::query(
        "UPDATE users SET last_seen_at = to_timestamp($3::bigint / 1000.0) WHERE tid = $1 AND oid = $2",
    )
    .bind(who.id.tid().as_str())
    .bind(who.id.oid().as_str())
    .bind(START - days_ago * DAY_MS)
    .execute(&f.rig.pool)
    .await
    .unwrap();
}

async fn with_token_for(f: &Vaulted, n: u32) -> User {
    let who = f.rig.user(n, Role::Editor).await;
    f.gh.token(&tok(n), &format!("user{n}"), &[BASE, TENANT]);
    f.vault
        .put(&ctx(&who, &format!("k-{n}")), secret(&tok(n)))
        .await
        .unwrap();
    who
}

#[tokio::test]
async fn credentials_of_users_not_seen_for_30_days_are_deleted_with_an_event_each() {
    let Some(f) = fixture().await else { return };
    let gone = with_token_for(&f, 10).await;
    let edge = with_token_for(&f, 11).await;
    let recent = with_token_for(&f, 12).await;
    let never = with_token_for(&f, 13).await;
    let no_token = f.rig.user(14, Role::Editor).await;
    seen(&f, &gone, 31).await;
    seen(&f, &edge, 29).await;
    seen(&f, &recent, 1).await;
    seen(&f, &no_token, 400).await;
    // `never` signed in without ever being seen; its credential is as old as it is.
    sqlx::query(
        "UPDATE github_credentials SET created_at = to_timestamp($1::bigint / 1000.0) WHERE user_oid = $2",
    )
    .bind(START - 40 * DAY_MS)
    .bind(never.id.oid().as_str())
    .execute(&f.rig.pool)
    .await
    .unwrap();
    let before = f.rig.chain().await.len();

    let report = f.rig.hub.credential_cleanup().run_once(&|| true).await;
    assert_eq!((report.deleted, report.failed), (2, 0));

    let left: Vec<String> = sqlx::query_scalar("SELECT user_oid FROM github_credentials ORDER BY user_oid")
        .fetch_all(&f.rig.pool)
        .await
        .unwrap();
    assert_eq!(
        left,
        [edge.id.oid().as_str(), recent.id.oid().as_str()].map(str::to_owned)
    );

    // One system event for each deleted credential, in the chain, with no token in it.
    let chain = f.rig.chain().await;
    verify_chain(&chain).unwrap();
    let events = &chain[before..];
    assert_eq!(events.len(), 2);
    for e in events {
        assert_eq!(e.event.action, AuditAction::GithubTokenDeleted);
        assert_eq!(e.event.via, AuditVia::System);
        assert!(matches!(e.event.actor, AuditActor::System { .. }));
        assert!(!serde_json::to_string(&e.event).unwrap().contains("github_pat_"));
    }
    // The login stays linked, as after a delete by the owner.
    assert_eq!(f.linked(&gone).await.as_deref(), Some("user10"));
    // A second run finds nothing to do.
    assert_eq!(f.rig.hub.credential_cleanup().run_once(&|| true).await.deleted, 0);
    assert_eq!(f.rig.chain().await.len(), chain.len());
}

/// The scan lists the credential as stale; the user signs in before the delete, which asks again. The job asks whether it still
/// holds its lease between the scan and each delete, so that is where the sign-in is placed (a multi-thread runtime lets the
/// synchronous hook wait for the update).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_who_signs_in_between_the_scan_and_the_delete_keeps_the_token() {
    let Some(f) = fixture().await else { return };
    let who = with_token_for(&f, 10).await;
    seen(&f, &who, 31).await;
    let signed_in = AtomicBool::new(false);
    let report = f
        .rig
        .hub
        .credential_cleanup()
        .run_once(&|| {
            if !signed_in.swap(true, Ordering::SeqCst) {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        sqlx::query("UPDATE users SET last_seen_at = to_timestamp($2::bigint / 1000.0) WHERE oid = $1")
                            .bind(who.id.oid().as_str())
                            .bind(START)
                            .execute(&f.rig.pool)
                            .await
                            .unwrap();
                    });
                });
            }
            true
        })
        .await;
    assert!(
        signed_in.load(Ordering::SeqCst),
        "the hook never ran, so the race was not staged"
    );
    assert_eq!(
        (report.deleted, report.failed),
        (0, 0),
        "a user who had just signed in lost their token"
    );
    assert_eq!(f.rows().await, 1);
    assert_eq!(f.events(AuditAction::GithubTokenDeleted).await, 0);
}

#[tokio::test]
async fn the_cleanup_stops_when_the_lease_is_lost_and_works_in_bounded_pages() {
    let config = WriteConfig {
        expire_batch: 2,
        ..WriteConfig::default()
    };
    let Some(f) = fixture_with(config).await else {
        return;
    };
    for n in 10..15 {
        let who = with_token_for(&f, n).await;
        seen(&f, &who, 60).await;
    }
    // Lease lost at once: nothing is deleted.
    let report = f.rig.hub.credential_cleanup().run_once(&|| false).await;
    assert_eq!(report.deleted, 0);
    assert_eq!(f.rows().await, 5);
    // Lease lost after two deletions: the run stops there.
    let budget = AtomicUsize::new(2);
    let report = f
        .rig
        .hub
        .credential_cleanup()
        .run_once(&|| {
            budget
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        })
        .await;
    assert_eq!(report.deleted, 2);
    assert_eq!(f.rows().await, 3);
    // Held throughout, with a page of two: all five are gone over several pages.
    let report = f.rig.hub.credential_cleanup().run_once(&|| true).await;
    assert_eq!(report.deleted, 3);
    assert_eq!(f.rows().await, 0);
}

#[tokio::test]
async fn the_cleanup_counts_tokens_that_expire_within_a_week() {
    let Some(f) = fixture().await else { return };
    for (n, days) in [(10, 2), (11, 6), (12, 20)] {
        f.gh.token(&tok(n), &format!("user{n}"), &[BASE, TENANT]);
        f.gh.expires(&tok(n), Timestamp::from_unix_millis(START + days * DAY_MS));
        let who = f.rig.user(n, Role::Editor).await;
        f.vault.put(&ctx(&who, "k-1"), secret(&tok(n))).await.unwrap();
    }
    let report = f.rig.hub.credential_cleanup().run_once(&|| true).await;
    assert_eq!((report.deleted, report.expiring_soon), (0, 2));
}
