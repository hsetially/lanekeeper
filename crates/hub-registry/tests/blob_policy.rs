//! The blob endpoint's policy (T6, S11, D79): the hash of a denied file's content is never served, through hub-platform's own endpoint,
//! and the policy fails closed. Over a real Postgres.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use domain::ContentHash;
use http::{Method, Request};
use http_body_util::BodyExt;
use hub_platform::blobs::endpoint::{BlobPolicy, BlobState, mount};
use hub_platform::{GuardedRouter, TEST_USER_HEADER};
use hub_registry::RegistryBlobPolicy;
use ports::{BlobStore, content_hash};
use support::reads::Reads;
use tower::ServiceExt;

const KEY: &str = "-----BEGIN PRIVATE KEY-----\nPLANTED-KEY-BYTES\n";

async fn world() -> Option<Reads> {
    let mut r = Reads::new(&["sit1"]).await?;
    r.world
        .inventory(&[
            ("core.yml", "port: 1\n"),
            ("keys/server.pem", KEY),
            // The same bytes as the key, under a name nothing denies.
            ("copy-of-key.txt", KEY),
        ])
        .await;
    Some(r)
}

fn policy(r: &Reads) -> RegistryBlobPolicy {
    RegistryBlobPolicy::new(r.world.rig.pool.clone(), &r.config, r.world.rig.metrics.clone())
}

#[tokio::test]
async fn the_blob_policy_refuses_their_hashes() {
    let Some(r) = world().await else { return };
    let policy = policy(&r);
    let key = content_hash(KEY.as_bytes());
    assert!(!policy.allowed(&key).await, "the content of a denied file");
    assert!(
        policy.allowed(&content_hash(b"port: 1\n")).await,
        "an ordinary file's content"
    );
    assert!(
        policy.allowed(&content_hash(b"nobody has ever seen this")).await,
        "a hash nothing refers to is allowed: the endpoint says 404 for it itself"
    );
    assert_eq!(r.world.rig.metrics.blob_withheld_count(), 1);
    // The bytes are the same under a name nothing denies; that file is an ordinary file (denial is by the path rules), but its hash is
    // the denied file's, and while the denied file has it, it is refused: the blob endpoint cannot be a way round the deny rules.
    let copy_denied: bool =
        sqlx::query_scalar("SELECT denied FROM file_state WHERE path = 'copy-of-key.txt'")
            .fetch_one(&r.world.rig.pool)
            .await
            .unwrap();
    assert!(!copy_denied);
    // The denied file is deleted on NFS. Its row stays (the trace of a removed file) and its history still names the hash, and a denied file
    // is refused for every version it has had (T9, D104): the bytes are still the key's.
    let mut r = r;
    r.world.nfs_delete(&["keys/server.pem"]).await;
    assert!(
        !policy.allowed(&key).await,
        "the file that was denied has had these bytes: it is still refused"
    );
    // Once no row of a denied file has had the hash, nothing holds it back (the copy is an ordinary file by the admin's own rules).
    sqlx::query("DELETE FROM file_state WHERE path = 'keys/server.pem'")
        .execute(&r.world.rig.pool)
        .await
        .unwrap();
    assert!(policy.allowed(&key).await);
}

fn blob_request(hash: &ContentHash) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/blobs/{hash}"))
        .header(TEST_USER_HEADER, "viewer")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn the_platforms_blob_endpoint_serves_an_ordinary_blob_and_answers_a_denied_hash_as_it_answers_an_unknown_one()
 {
    let Some(r) = world().await else { return };
    let store: Arc<dyn BlobStore> = Arc::clone(&r.world.rig.blobs) as _;
    let key_hash = content_hash(KEY.as_bytes());
    // The key's bytes are in the store (the copy under an allowed name put them there).
    assert!(store.get(&key_hash).await.unwrap().is_some());
    let state = BlobState::new(Arc::clone(&store), Arc::new(policy(&r)));
    let (router, _) = mount(GuardedRouter::new(support::http::auth(), None), state)
        .into_parts()
        .unwrap();

    let get = |hash: ContentHash| {
        let router = router.clone();
        async move {
            let response = router.oneshot(blob_request(&hash)).await.unwrap();
            let (parts, body) = response.into_parts();
            (
                parts.status,
                parts.headers,
                body.collect().await.unwrap().to_bytes(),
            )
        }
    };
    let (status, _, body) = get(content_hash(b"port: 1\n")).await;
    assert_eq!(status, 200);
    assert_eq!(body, Bytes::from_static(b"port: 1\n"));

    let (denied_status, _, denied_body) = get(key_hash).await;
    let (unknown_status, _, unknown_body) = get(content_hash(b"nobody has ever seen this")).await;
    assert_eq!(denied_status, 404);
    assert_eq!(
        (denied_status, &denied_body),
        (unknown_status, &unknown_body),
        "a refused hash is indistinguishable from an unknown one"
    );
    assert!(!String::from_utf8_lossy(&denied_body).contains("PLANTED-KEY-BYTES"));
}

#[tokio::test]
async fn a_policy_that_cannot_ask_says_no() {
    let Some(r) = world().await else { return };
    let policy = policy(&r);
    assert!(policy.allowed(&content_hash(b"port: 1\n")).await);
    r.world.rig.pool.close().await;
    for bytes in [&b"port: 1\n"[..], KEY.as_bytes(), b"never seen"] {
        assert!(
            !policy.allowed(&content_hash(bytes)).await,
            "fail closed: a database that is down is a withheld blob"
        );
    }
}
