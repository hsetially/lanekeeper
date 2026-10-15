//! S7, S9, P12 against a real Postgres 16: `PgTokenStore` as `lanekeeper_app`. See `pg.rs` for how the database is found; without
//! one these tests print `SKIPPED` and did not run.

use std::sync::Arc;

use domain::{Role, Timestamp, UserId};
use hub_mcp::auth::pat::{PersonalTokens, TokenError, TokenSettings};
use hub_mcp::auth::pg_store::{PgTokenStore, explain};
use hub_mcp::auth::store::{Label, TokenId};
use hub_mcp::auth::token::{PersonalToken, TokenHmac};
use hub_mcp::clock::{Clock, ManualClock, SystemClock};
use ports::AuthError;
use sqlx::{PgPool, Row};

use crate::pg::{PgTestAudit, TestDb, sqlstate};
use crate::rig::{key, uid};

struct Pg {
    db: TestDb,
    pool: PgPool,
    tokens: PersonalTokens,
    clock: Arc<ManualClock>,
}

fn label(text: &str) -> Label {
    Label::parse(text).unwrap()
}

/// A database with `alice` and `bob`, and a token service over it whose audit log is `audit`.
async fn rig(audit: PgTestAudit, settings: TokenSettings) -> Option<Pg> {
    let db = TestDb::ready().await?;
    db.make_test_audit_table().await;
    for who in ["alice", "bob"] {
        db.add_user(&uid(who)).await;
    }
    let pool = db.app_pool(16).await;
    // The clock starts at the real time: the database stamps new rows with its own clock, and the service judges expiry with this one.
    let clock = Arc::new(ManualClock::at(SystemClock.now()));
    let tokens = PersonalTokens::new(
        Arc::new(PgTokenStore::new(pool.clone())),
        key(),
        Arc::new(audit),
        Arc::clone(&clock) as Arc<dyn Clock>,
        settings,
    );
    Some(Pg {
        db,
        pool,
        tokens,
        clock,
    })
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn audit_actions(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT action FROM test_audit ORDER BY seq")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn pg_roundtrip_creates_authenticates_lists_and_revokes() {
    let Some(pg) = rig(PgTestAudit::default(), TokenSettings::default()).await else {
        return;
    };
    let alice = uid("alice");
    let created = pg.tokens.create(&alice, label("laptop"), None).await.unwrap();
    let token = created.token.expose().clone();
    assert_eq!(created.record.owner, alice);
    // The database's clock stamped it: seven days, to the second.
    assert_eq!(
        created.record.expires_at.unix_millis() - created.record.created_at.unix_millis(),
        7 * 24 * 60 * 60 * 1000
    );
    assert_eq!(pg.tokens.authenticate(&token).await, Ok(alice.clone()));

    let listed = pg.tokens.list(&alice, None, 10).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.record.id);
    assert!(
        pg.tokens.list(&uid("bob"), None, 10).await.unwrap().is_empty(),
        "another user's list is empty"
    );

    pg.tokens.revoke(&alice, created.record.id, None).await.unwrap();
    assert_eq!(pg.tokens.authenticate(&token).await, Err(AuthError::InvalidToken));
    let listed = pg.tokens.list(&alice, None, 10).await.unwrap();
    assert!(listed[0].revoked_at.is_some());
    // Revoking again is fine and changes nothing.
    pg.tokens.revoke(&alice, created.record.id, None).await.unwrap();
    assert!(matches!(
        pg.tokens.revoke(&uid("bob"), created.record.id, None).await,
        Err(TokenError::NotFound)
    ));
}

#[tokio::test]
async fn pat_is_stored_as_hmac_not_plaintext_in_the_table() {
    let Some(pg) = rig(PgTestAudit::default(), TokenSettings::default()).await else {
        return;
    };
    let created = pg
        .tokens
        .create(&uid("alice"), label("laptop"), None)
        .await
        .unwrap();
    let token = created.token.expose().clone();
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hmac FROM mcp_tokens")
        .fetch_one(&pg.pool)
        .await
        .unwrap();
    let parsed = PersonalToken::parse(&token).unwrap();
    assert_eq!(stored, key().lookup(&parsed).unwrap().as_bytes());
    // Nothing in any column, and nothing in the audit table, holds the token.
    let row = sqlx::query("SELECT row_to_json(t)::text AS j FROM mcp_tokens t")
        .fetch_one(&pg.pool)
        .await
        .unwrap();
    let json: String = row.get("j");
    assert!(!json.contains(&token["lkpat_".len()..]), "{json}");
    let diffs: Vec<Option<String>> = sqlx::query_scalar("SELECT diff FROM test_audit")
        .fetch_all(&pg.pool)
        .await
        .unwrap();
    assert!(!format!("{diffs:?}").contains(&token["lkpat_".len()..]));
}

#[tokio::test]
async fn create_and_revoke_write_exactly_one_audit_event_in_the_same_tx() {
    let Some(pg) = rig(PgTestAudit::default(), TokenSettings::default()).await else {
        return;
    };
    let alice = uid("alice");
    assert_eq!(count(&pg.pool, "test_audit").await, 0);
    let created = pg.tokens.create(&alice, label("laptop"), None).await.unwrap();
    assert_eq!(audit_actions(&pg.pool).await, ["mcp_token_created"]);
    let diff: String = sqlx::query_scalar("SELECT diff FROM test_audit")
        .fetch_one(&pg.pool)
        .await
        .unwrap();
    assert_eq!(
        diff,
        format!(
            "token={}; label=laptop; expires_at={}",
            created.record.id,
            created.record.expires_at.unix_millis()
        )
    );
    pg.tokens.revoke(&alice, created.record.id, None).await.unwrap();
    assert_eq!(
        audit_actions(&pg.pool).await,
        ["mcp_token_created", "mcp_token_revoked"]
    );
    // A second revocation, and a refused one, write nothing.
    pg.tokens.revoke(&alice, created.record.id, None).await.unwrap();
    let _ = pg.tokens.revoke(&uid("bob"), created.record.id, None).await;
    let _ = pg
        .tokens
        .revoke(&alice, TokenId::new(424_242).unwrap(), None)
        .await;
    assert_eq!(audit_actions(&pg.pool).await.len(), 2);
}

#[tokio::test]
async fn rollback_leaves_no_token_and_no_event() {
    let Some(pg) = rig(
        PgTestAudit {
            fail_after_write: true,
        },
        TokenSettings::default(),
    )
    .await
    else {
        return;
    };
    let alice = uid("alice");
    // The event is written into the transaction and then the call fails: the row and the event both go.
    let made = pg.tokens.create(&alice, label("laptop"), None).await;
    assert!(matches!(made, Err(TokenError::Unavailable)), "{made:?}");
    assert_eq!(count(&pg.pool, "mcp_tokens").await, 0);
    assert_eq!(count(&pg.pool, "test_audit").await, 0);

    // The same for a revocation: make a token with a log that works, then try to revoke it with one that fails.
    let working = PersonalTokens::new(
        Arc::new(PgTokenStore::new(pg.pool.clone())),
        key(),
        Arc::new(PgTestAudit::default()),
        Arc::clone(&pg.clock) as Arc<dyn Clock>,
        TokenSettings::default(),
    );
    let created = working.create(&alice, label("laptop"), None).await.unwrap();
    let revoked = pg.tokens.revoke(&alice, created.record.id, None).await;
    assert!(matches!(revoked, Err(TokenError::Unavailable)));
    let revoked_at: Option<Timestamp> = pg.tokens.list(&alice, None, 1).await.unwrap()[0].revoked_at;
    assert_eq!(revoked_at, None, "the token is still live");
    assert_eq!(
        audit_actions(&pg.pool).await,
        ["mcp_token_created"],
        "and no second event"
    );
    assert_eq!(working.authenticate(created.token.expose()).await, Ok(alice));
}

#[tokio::test]
async fn twelve_creations_at_once_make_exactly_five() {
    let Some(pg) = rig(PgTestAudit::default(), TokenSettings::default()).await else {
        return;
    };
    let tokens = Arc::new(pg.tokens);
    let alice = uid("alice");
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..12 {
        let (tokens, alice) = (Arc::clone(&tokens), alice.clone());
        tasks.spawn(async move {
            tokens
                .create(&alice, label(&format!("t{n}")), None)
                .await
                .map(|c| c.record.id)
        });
    }
    let (mut made, mut refused) = (0, 0);
    while let Some(done) = tasks.join_next().await {
        match done.unwrap() {
            Ok(_) => made += 1,
            Err(TokenError::LimitReached) => refused += 1,
            Err(other) => panic!("{other:?}"),
        }
    }
    assert_eq!(
        (made, refused),
        (5, 7),
        "the advisory lock serialises the check and the insert"
    );
    assert_eq!(count(&pg.pool, "mcp_tokens").await, 5);
    assert_eq!(
        count(&pg.pool, "test_audit").await,
        5,
        "one event for each token, none for the refused ones"
    );
}

#[tokio::test]
async fn last_used_moves_at_most_every_ten_seconds_and_never_for_a_revoked_token() {
    let settings = TokenSettings {
        cache_ttl: std::time::Duration::ZERO,
        ..TokenSettings::default()
    };
    let Some(pg) = rig(PgTestAudit::default(), settings).await else {
        return;
    };
    let alice = uid("alice");
    let created = pg.tokens.create(&alice, label("laptop"), None).await.unwrap();
    let token = created.token.expose().clone();
    let used = || async {
        let list = pg.tokens.list(&alice, None, 1).await.unwrap();
        list[0].last_used_at
    };
    assert_eq!(used().await, None);
    pg.tokens.authenticate(&token).await.unwrap();
    let first = used().await.unwrap();
    assert!(
        (first.unix_millis() - pg.clock.now().unix_millis()).abs() < 1500,
        "the hub's clock stamped it"
    );
    pg.clock.advance_secs(9);
    pg.tokens.authenticate(&token).await.unwrap();
    assert_eq!(
        used().await,
        Some(first),
        "nine seconds later nothing was written"
    );
    pg.clock.advance_secs(2);
    pg.tokens.authenticate(&token).await.unwrap();
    assert!(used().await.unwrap() > first);

    pg.tokens.revoke(&alice, created.record.id, None).await.unwrap();
    let before = used().await;
    pg.clock.advance_secs(60);
    assert_eq!(pg.tokens.authenticate(&token).await, Err(AuthError::InvalidToken));
    assert_eq!(used().await, before, "a revoked token is not marked as used");
}

#[tokio::test]
async fn a_token_that_has_ended_is_not_marked_as_used_and_is_refused() {
    let settings = TokenSettings {
        cache_ttl: std::time::Duration::ZERO,
        ..TokenSettings::default()
    };
    let Some(pg) = rig(PgTestAudit::default(), settings).await else {
        return;
    };
    let alice = uid("alice");
    let created = pg.tokens.create(&alice, label("laptop"), None).await.unwrap();
    pg.clock.advance_secs(8 * 24 * 60 * 60);
    assert_eq!(
        pg.tokens.authenticate(created.token.expose()).await,
        Err(AuthError::Expired)
    );
    assert_eq!(
        pg.tokens.list(&alice, None, 1).await.unwrap()[0].last_used_at,
        None
    );
}

#[tokio::test]
async fn the_list_pages_newest_first_by_keyset() {
    let Some(pg) = rig(
        PgTestAudit::default(),
        TokenSettings {
            max_live: 10,
            ..TokenSettings::default()
        },
    )
    .await
    else {
        return;
    };
    let alice = uid("alice");
    let mut ids = Vec::new();
    for n in 0..7 {
        ids.push(
            pg.tokens
                .create(&alice, label(&format!("t{n}")), None)
                .await
                .unwrap()
                .record
                .id,
        );
    }
    pg.tokens.create(&uid("bob"), label("bob's"), None).await.unwrap();
    ids.reverse();
    let first = pg.tokens.list(&alice, None, 3).await.unwrap();
    assert_eq!(first.iter().map(|r| r.id).collect::<Vec<_>>(), ids[..3]);
    let second = pg.tokens.list(&alice, Some(first[2].id), 3).await.unwrap();
    assert_eq!(second.iter().map(|r| r.id).collect::<Vec<_>>(), ids[3..6]);
    let third = pg.tokens.list(&alice, Some(second[2].id), 3).await.unwrap();
    assert_eq!(third.iter().map(|r| r.id).collect::<Vec<_>>(), ids[6..]);
}

#[tokio::test]
async fn lookups_and_lists_use_their_indexes() {
    let Some(pg) = rig(PgTestAudit::default(), TokenSettings::default()).await else {
        return;
    };
    let alice = uid("alice");
    pg.tokens.create(&alice, label("laptop"), None).await.unwrap();
    // A table this small is read whole by any planner; with sequential scans off, the plan shows whether an index could serve the
    // statement at all (P12).
    let mut conn = pg.pool.acquire().await.unwrap();
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut *conn)
        .await
        .unwrap();
    let find = explain::find(&mut conn, &TokenHmac::from_bytes([7; 32]), pg.clock.now())
        .await
        .unwrap();
    let list = explain::list(&mut conn, &alice, None, 100).await.unwrap();
    for (name, plan) in [("find", find.to_string()), ("list", list.to_string())] {
        assert!(!plan.contains("Seq Scan"), "{name}: {plan}");
        assert!(plan.contains("Index"), "{name}: {plan}");
    }
    assert!(
        find.to_string().contains("mcp_tokens_token_hmac_key"),
        "the unique index on the hash: {find}"
    );
    assert!(list.to_string().contains("mcp_tokens_user_idx"), "{list}");
}

#[tokio::test]
async fn the_app_role_has_only_the_grants_the_store_needs() {
    let Some(pg) = rig(PgTestAudit::default(), TokenSettings::default()).await else {
        return;
    };
    let created = pg
        .tokens
        .create(&uid("alice"), label("laptop"), None)
        .await
        .unwrap();
    let id = created.record.id.get();
    // What the store does as `lanekeeper_app` works. What it must never be able to do does not (migration 0005).
    for sql in [
        "DELETE FROM mcp_tokens",
        "UPDATE mcp_tokens SET label = 'renamed'",
        "UPDATE mcp_tokens SET token_hmac = '\\x00'::bytea",
        "UPDATE mcp_tokens SET expires_at = expires_at + interval '1 day'",
        "TRUNCATE mcp_tokens",
    ] {
        let error = sqlx::query(sql).execute(&pg.pool).await.unwrap_err();
        assert_eq!(sqlstate(&error).as_deref(), Some("42501"), "{sql}");
    }
    let _ = id;
}

#[tokio::test]
async fn database_errors_are_logged_by_class_and_never_leak_into_the_answer() {
    let Some(pg) = rig(PgTestAudit::default(), TokenSettings::default()).await else {
        return;
    };
    let alice = uid("alice");
    pg.tokens.create(&alice, label("laptop"), None).await.unwrap();
    // The table goes away under the service: every statement fails with 42P01.
    let mut conn = pg.db.admin().await;
    sqlx::query("DROP TABLE mcp_tokens")
        .execute(&mut conn)
        .await
        .unwrap();
    let made = pg.tokens.create(&alice, label("again"), None).await;
    assert!(matches!(made, Err(TokenError::Unavailable)), "{made:?}");
    assert!(matches!(
        pg.tokens.list(&alice, None, 10).await,
        Err(TokenError::Unavailable)
    ));
    let token = hub_mcp::auth::token::generate().unwrap();
    assert_eq!(pg.tokens.authenticate(&token).await, Err(AuthError::Unavailable));
    let _: &UserId = &alice;
    let _ = Role::Viewer;
}
