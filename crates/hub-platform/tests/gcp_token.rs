//! T2: the workload's access token from the metadata server (S7).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use hub_platform::gcp::token::{AccessTokens, TokenError};
use support::fake_google::{Fault, Harness, Route};
use support::log_capture::LogCapture;

#[tokio::test(start_paused = true)]
async fn workload_identity_token_is_cached_and_never_logged() {
    let (logs, _guard) = LogCapture::install();
    let h = Harness::new();

    let first = h.tokens.token().await.unwrap();
    let second = h.tokens.token().await.unwrap();
    assert!(bool::from(first.ct_eq(&second)));
    assert_eq!(h.fake.count(Route::Token), 1, "one fetch serves both");
    let request = &h.fake.requests()[0];
    assert_eq!(
        request.metadata_flavor.as_deref(),
        Some("Google"),
        "the metadata server requires this header"
    );
    assert_eq!(
        request.path,
        "/computeMetadata/v1/instance/service-accounts/default/token"
    );

    // A token lives for 3,599 s and is renewed 60 s early.
    tokio::time::advance(Duration::from_secs(3538)).await;
    h.tokens.token().await.unwrap();
    assert_eq!(
        h.fake.count(Route::Token),
        1,
        "still cached one second before the refresh point"
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    let renewed = h.tokens.token().await.unwrap();
    assert_eq!(h.fake.count(Route::Token), 2, "renewed 60 s before it expires");
    assert!(!bool::from(first.ct_eq(&renewed)));

    // Failure paths log too.
    h.fake.script(Route::Token, Fault::Status(500), 3);
    tokio::time::advance(Duration::from_secs(4000)).await;
    assert_eq!(h.tokens.token().await.unwrap_err(), TokenError::Unavailable);

    let everything = format!(
        "{first:?} {renewed:?} {:?} {} {}",
        h.tokens,
        logs.text(),
        h.exposition()
    );
    for token in [first.expose(), renewed.expose()] {
        assert!(!everything.contains(token.as_str()), "an access token leaked");
    }
    assert!(!logs.is_empty());
}

#[tokio::test(start_paused = true)]
async fn concurrent_callers_share_one_fetch() {
    let h = Harness::new();
    let tokens = futures::future::join_all((0..16).map(|_| h.tokens.token())).await;
    assert!(tokens.iter().all(Result::is_ok));
    assert_eq!(h.fake.count(Route::Token), 1);
}

#[tokio::test(start_paused = true)]
async fn a_refused_token_is_forgotten_unless_a_newer_one_replaced_it() {
    let h = Harness::new();
    let old = h.tokens.token().await.unwrap();
    h.tokens.refused(&old).await;
    let fresh = h.tokens.token().await.unwrap();
    assert_eq!(
        h.fake.count(Route::Token),
        2,
        "refusing the token forces a new fetch"
    );
    assert!(!bool::from(old.ct_eq(&fresh)));

    // A late `401` for the old token must not throw the new one away.
    h.tokens.refused(&old).await;
    let again = h.tokens.token().await.unwrap();
    assert_eq!(h.fake.count(Route::Token), 2);
    assert!(bool::from(fresh.ct_eq(&again)));
}

#[tokio::test(start_paused = true)]
async fn answers_that_are_not_access_tokens_are_refused() {
    let h = Harness::new();
    h.fake.script(Route::Token, Fault::Garbage, 1);
    assert_eq!(h.tokens.token().await.unwrap_err(), TokenError::Malformed);

    for lifetime in [0, 86_401, u64::MAX / 2] {
        h.fake.set_token_lifetime(lifetime);
        assert_eq!(
            h.tokens.token().await.unwrap_err(),
            TokenError::Malformed,
            "expires_in {lifetime}"
        );
    }
    h.fake.set_token_lifetime(3599);
    assert!(
        h.tokens.token().await.is_ok(),
        "and the next good answer is accepted"
    );
}

#[tokio::test(start_paused = true)]
async fn the_metadata_server_is_retried_and_then_given_up_on() {
    let h = Harness::new();
    h.fake.script(Route::Token, Fault::Status(503), 2);
    assert!(h.tokens.token().await.is_ok(), "two 503s and then a token");
    assert_eq!(h.fake.count(Route::Token), 3);

    let h = Harness::new();
    h.fake.script(Route::Token, Fault::Status(404), 1);
    assert_eq!(
        h.tokens.token().await.unwrap_err(),
        TokenError::Unavailable,
        "a 404 is not retried"
    );
    assert_eq!(h.fake.count(Route::Token), 1);
}
