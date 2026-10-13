//! T2: the Secret Manager source (S7, S8, S10).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use domain::Secret;
use ports::{SecretError, SecretSource};
use support::fake_google::{Fault, Harness, Route, SECRETS_HOST};
use support::log_capture::LogCapture;

fn value(secret: &Secret<String>) -> String {
    secret.expose().clone()
}

#[tokio::test(start_paused = true)]
async fn secret_cache_expires_after_5_minutes_and_is_bounded() {
    let h = Harness::new();
    h.fake.set_secret("db-password", "v1");
    // An hour is asked for; the source caps it at five minutes (S7).
    let source = h.secret_source_with(Duration::from_secs(3600), 8);

    assert_eq!(value(&source.get("db-password").await.unwrap()), "v1");
    h.fake.set_secret("db-password", "v2");
    tokio::time::advance(Duration::from_secs(299)).await;
    assert_eq!(
        value(&source.get("db-password").await.unwrap()),
        "v1",
        "still cached at 299 s"
    );
    assert_eq!(h.fake.count(Route::SecretAccess), 1);

    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        value(&source.get("db-password").await.unwrap()),
        "v2",
        "a rotated secret shows up at 5 minutes"
    );
    assert_eq!(h.fake.count(Route::SecretAccess), 2);

    // Bounded: a cache of two names holds two.
    let small = h.secret_source_with(Duration::from_secs(300), 2);
    for name in ["a", "b", "c"] {
        h.fake.set_secret(name, name);
        small.get(name).await.unwrap();
    }
    let before = h.fake.count(Route::SecretAccess);
    small.get("c").await.unwrap();
    assert_eq!(h.fake.count(Route::SecretAccess), before, "the newest is cached");
    small.get("a").await.unwrap();
    assert_eq!(
        h.fake.count(Route::SecretAccess),
        before + 1,
        "the oldest was evicted to keep the bound"
    );
}

#[tokio::test(start_paused = true)]
async fn a_missing_secret_is_not_cached() {
    let h = Harness::new();
    let source = h.secret_source();
    assert_eq!(source.get("later").await.unwrap_err(), SecretError::NotFound);
    h.fake.set_secret("later", "now-here");
    assert_eq!(value(&source.get("later").await.unwrap()), "now-here");
    assert_eq!(h.fake.count(Route::SecretAccess), 2);
}

#[tokio::test(start_paused = true)]
async fn secret_names_are_validated_before_any_request() {
    let h = Harness::new();
    let source = h.secret_source();
    let long = "x".repeat(256);
    for bad in [
        "",
        "has space",
        "../etc/passwd",
        "a/b",
        "a:access",
        "a?x=1",
        "a%2e",
        long.as_str(),
    ] {
        assert_eq!(
            source.get(bad).await.unwrap_err(),
            SecretError::InvalidName,
            "{bad:?}"
        );
    }
    assert!(h.fake.requests().is_empty(), "no request, not even for a token");
}

#[tokio::test(start_paused = true)]
async fn a_secret_whose_checksum_does_not_match_is_refused_and_not_cached() {
    let h = Harness::new();
    h.fake.set_secret("db-password", "good");
    let source = h.secret_source();
    h.fake.script(Route::SecretAccess, Fault::DamagedAnswer, 1);
    assert_eq!(
        source.get("db-password").await.unwrap_err(),
        SecretError::Unavailable
    );
    assert_eq!(
        value(&source.get("db-password").await.unwrap()),
        "good",
        "nothing damaged was kept"
    );
    assert!(
        h.exposition()
            .contains(r#"operation="SecretAccess",outcome="BadResponse""#)
    );
}

#[tokio::test(start_paused = true)]
async fn secret_manager_failures_are_unavailable_and_retried_a_bounded_number_of_times() {
    let h = Harness::new();
    h.fake.set_secret("db-password", "v");
    let source = h.secret_source();

    // 403 is a refusal: not retried, and not told apart from an outage (the caller learns nothing about what exists).
    h.fake.script(Route::SecretAccess, Fault::Status(403), 1);
    assert_eq!(
        source.get("db-password").await.unwrap_err(),
        SecretError::Unavailable
    );
    assert_eq!(h.fake.count(Route::SecretAccess), 1);

    // 503 is retried, three tries in all, then given up.
    h.fake.script(Route::SecretAccess, Fault::Status(503), 3);
    assert_eq!(
        source.get("db-password").await.unwrap_err(),
        SecretError::Unavailable
    );
    assert_eq!(h.fake.count(Route::SecretAccess), 4);

    // One 503 and then the answer.
    h.fake.script(Route::SecretAccess, Fault::Status(503), 1);
    assert_eq!(value(&source.get("db-password").await.unwrap()), "v");
}

#[tokio::test(start_paused = true)]
async fn secret_value_never_appears_in_debug_or_logs() {
    let (logs, _guard) = LogCapture::install();
    let h = Harness::new();
    let planted = "hunter2-planted-secret-value";
    h.fake.set_secret("db-password", planted);
    let source = h.secret_source();

    let got = source.get("db-password").await.unwrap();
    assert_eq!(value(&got), planted);
    // Drive the failure paths too: they log.
    h.fake.script(Route::SecretAccess, Fault::DamagedAnswer, 1);
    h.fake.remove_secret("db-password");
    let _ = h.secret_source().get("gone").await;
    h.fake.set_secret("db-password", planted);
    h.fake.script(Route::SecretAccess, Fault::Status(500), 3);
    let _ = h.secret_source().get("db-password").await;

    let everything = format!(
        "{got:?} {source:?} {:?} {:?} {} {}",
        h.api,
        h.transport,
        logs.text(),
        h.exposition()
    );
    assert!(!everything.contains(planted), "the secret value leaked");
    assert!(!everything.contains("ya29.fake-token"), "an access token leaked");
    assert!(!logs.is_empty(), "the failure paths log something (codes only)");
    assert!(
        !logs.text().contains("db-password") && !logs.text().contains(SECRETS_HOST),
        "names and hosts stay out of logs"
    );
}
