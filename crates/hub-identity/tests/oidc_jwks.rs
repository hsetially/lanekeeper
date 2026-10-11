//! T1 (S1): the key-set cache. Time is tokio's paused clock, so nothing sleeps.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use hub_identity::http::{HttpsClient, HttpsError, HttpsRequest, HttpsResponse};
use hub_identity::oidc::jwks::{JwksCache, KeySet};
use hub_identity::{IdentityError, ManualClock, TokenError};
use serde_json::json;
use support::mock_oidc::{MockOidc, NOW_MS, TID, keys, public_jwk};
use tokio::task::JoinSet;
use url::Url;

/// Answers like the mock, but only after 100 ms, so concurrent callers overlap.
struct Slow(Arc<support::fake_https::FakeHttps>);

#[async_trait]
impl HttpsClient for Slow {
    async fn send(&self, request: HttpsRequest) -> Result<HttpsResponse, HttpsError> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.0.send(request).await
    }
}

fn cache(mock: &Arc<MockOidc>, slow: bool) -> (Arc<JwksCache>, Arc<support::fake_https::FakeHttps>) {
    let fake = mock.https();
    let http: Arc<dyn HttpsClient> = if slow {
        Arc::new(Slow(Arc::clone(&fake)))
    } else {
        Arc::clone(&fake) as Arc<_>
    };
    let url = Url::parse(&format!("https://login.test/{TID}/discovery/v2.0/keys")).unwrap();
    let clock = Arc::new(ManualClock::at(domain::Timestamp::from_unix_millis(NOW_MS)));
    (Arc::new(JwksCache::new(http, Arc::new(url), clock)), fake)
}

fn fetches(mock: &MockOidc) -> usize {
    mock.jwks_fetches.load(Ordering::SeqCst)
}

fn unknown_key(e: IdentityError) -> bool {
    matches!(e, IdentityError::Token(TokenError::UnknownKey))
}

#[tokio::test(start_paused = true)]
async fn jwks_refresh_is_rate_limited_and_single_flight() {
    let mock = MockOidc::new();
    let (cache, _) = cache(&mock, true);

    // Ten requests arrive together on a cold cache: one fetch, and all of them are served.
    let mut set = JoinSet::new();
    for _ in 0..10 {
        let cache = Arc::clone(&cache);
        set.spawn(async move { cache.key("key-0").await.map(|_| ()) });
    }
    while let Some(done) = set.join_next().await {
        done.unwrap().unwrap();
    }
    assert_eq!(fetches(&mock), 1, "cold start is one fetch");

    // Ten tokens with an unknown key id, after the interval: exactly one refresh between them.
    tokio::time::advance(Duration::from_secs(31)).await;
    let mut set = JoinSet::new();
    for _ in 0..10 {
        let cache = Arc::clone(&cache);
        set.spawn(async move { cache.key("forged").await.map(|_| ()) });
    }
    while let Some(done) = set.join_next().await {
        assert!(unknown_key(done.unwrap().unwrap_err()));
    }
    assert_eq!(fetches(&mock), 2, "one refresh for ten unknown ids");

    // An attacker looping on unknown ids cannot make the hub fetch more than once per interval.
    for _ in 0..50 {
        assert!(unknown_key(cache.key("forged-again").await.unwrap_err()));
    }
    assert_eq!(fetches(&mock), 2);
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(unknown_key(cache.key("forged-again").await.unwrap_err()));
    assert_eq!(fetches(&mock), 3);
}

#[tokio::test(start_paused = true)]
async fn jwks_cache_honours_clamped_max_age() {
    for (header, lives, expires) in [
        (Some("public, max-age=5"), 59, 61),
        (Some("public, max-age=600"), 599, 601),
        (Some("max-age=999999999"), 6 * 3600 - 1, 6 * 3600 + 1),
        (None, 3599, 3601),
    ] {
        let mock = MockOidc::new();
        mock.set_jwks_cache_control(header);
        let (cache, _) = cache(&mock, false);
        cache.key("key-0").await.unwrap();
        assert_eq!(fetches(&mock), 1);
        tokio::time::advance(Duration::from_secs(lives)).await;
        cache.key("key-0").await.unwrap();
        assert_eq!(fetches(&mock), 1, "{header:?}: still cached after {lives} s");
        tokio::time::advance(Duration::from_secs(expires - lives)).await;
        cache.key("key-0").await.unwrap();
        assert_eq!(fetches(&mock), 2, "{header:?}: refetched after {expires} s");
    }
}

#[tokio::test(start_paused = true)]
async fn expired_keys_are_not_used_when_the_refresh_fails() {
    let mock = MockOidc::new();
    let (cache, _) = cache(&mock, false);
    cache.key("key-0").await.unwrap();
    mock.set_jwks_status(500);
    tokio::time::advance(Duration::from_secs(3601)).await;
    // Past its lifetime and the refresh fails: no token is verified with the old key.
    assert!(cache.key("key-0").await.is_err());
    assert_eq!(fetches(&mock), 2);
    // The next attempts within 5 s do not hit Entra again.
    for _ in 0..5 {
        assert!(cache.key("key-0").await.is_err());
    }
    assert_eq!(fetches(&mock), 2);
    // After the wait, and once Entra is back, it recovers.
    tokio::time::advance(Duration::from_secs(6)).await;
    mock.set_jwks_status(200);
    cache.key("key-0").await.unwrap();
    assert_eq!(fetches(&mock), 3);
}

#[test]
fn key_set_parsing_keeps_only_usable_rsa_signature_keys() {
    let good = public_jwk(&keys()[0]);
    let other = public_jwk(&keys()[1]);
    let doc = |keys: Vec<serde_json::Value>| serde_json::to_vec(&json!({"keys": keys})).unwrap();

    // Skipped: wrong kty, an encryption key, another algorithm, no kid, a short modulus.
    let mut enc = other.clone();
    enc["use"] = json!("enc");
    let mut es = other.clone();
    es["alg"] = json!("ES256");
    let mut no_kid = other.clone();
    no_kid.as_object_mut().unwrap().remove("kid");
    let mut short = other.clone();
    short["n"] = json!("AQAB");
    let oct = json!({"kty": "oct", "kid": "k", "k": "AAAA"});
    let set = KeySet::parse(&doc(vec![good.clone(), enc, es, no_kid, short, oct])).unwrap();
    assert_eq!(set.len(), 1);
    assert!(set.get("key-0").is_some());

    // A repeated key id makes the document ambiguous and is refused as a whole.
    assert!(matches!(
        KeySet::parse(&doc(vec![good.clone(), good.clone()])),
        Err(IdentityError::Jwks)
    ));
    // Too many keys, and a document with nothing usable.
    assert!(matches!(
        KeySet::parse(&doc(vec![good.clone(); 33])),
        Err(IdentityError::Jwks)
    ));
    assert!(matches!(
        KeySet::parse(&doc(vec![json!({"kty": "oct"})])),
        Err(IdentityError::Jwks)
    ));
}
