//! T5 (S9): the checkpoint bucket is create-only, and its credentials stay secret.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use domain::Secret;
use hub_identity::audit::bucket::{MAX_OBJECT_BYTES, object_name};
use hub_identity::audit::{
    AccessTokenSource, BucketError, CheckpointBucket, GcsCheckpointBucket, MemoryBucket, MetadataTokenSource,
    PutOutcome,
};
use hub_identity::http::HttpsError;
use support::fake_https::{FakeHttps, json_response};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The URL and `Authorization` header of each request the fake saw.
type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

const TOKEN: &str = "ya29.TOP-SECRET-ACCESS-TOKEN";

struct FixedToken;

#[async_trait]
impl AccessTokenSource for FixedToken {
    async fn token(&self) -> Result<Secret<String>, BucketError> {
        Ok(Secret::new(TOKEN.to_owned()))
    }
}

#[tokio::test]
async fn bucket_put_is_create_only() {
    let bucket = MemoryBucket::new();
    let name = object_name(7);
    assert_eq!(
        bucket.put(&name, Bytes::from_static(b"first")).await.unwrap(),
        PutOutcome::Created
    );
    assert_eq!(
        bucket.put(&name, Bytes::from_static(b"second")).await.unwrap(),
        PutOutcome::AlreadyExists
    );
    assert_eq!(
        bucket.get(&name).await.unwrap().unwrap(),
        Bytes::from_static(b"first")
    );
    assert_eq!(bucket.get(&object_name(8)).await.unwrap(), None);
    assert_eq!(bucket.len(), 1);
}

#[tokio::test]
async fn buckets_refuse_names_and_sizes_they_do_not_write() {
    let bucket = MemoryBucket::new();
    for bad in [
        "",
        "x",
        "audit/checkpoints/../../x.json",
        "audit/checkpoints/01.json",
        "audit/other/1.json",
    ] {
        assert_eq!(
            bucket.put(bad, Bytes::new()).await,
            Err(BucketError::InvalidName),
            "{bad}"
        );
        assert_eq!(bucket.get(bad).await, Err(BucketError::InvalidName), "{bad}");
    }
    let big = Bytes::from(vec![0u8; MAX_OBJECT_BYTES + 1]);
    assert_eq!(bucket.put(&object_name(1), big).await, Err(BucketError::TooLarge));
}

fn gcs(fake: Arc<FakeHttps>) -> GcsCheckpointBucket {
    GcsCheckpointBucket::new(fake, Arc::new(FixedToken), "lk-audit-checkpoints").unwrap()
}

#[tokio::test]
async fn gcs_upload_is_create_only_and_authorised() {
    let seen: Seen = Arc::default();
    let status = Arc::new(AtomicUsize::new(200));
    let fake = Arc::new({
        let (seen, status) = (seen.clone(), status.clone());
        FakeHttps::new(move |req| {
            seen.lock().unwrap().push((
                req.url().to_string(),
                req.header("authorization")
                    .map(|v| v.to_str().unwrap().to_owned()),
            ));
            Ok(json_response(
                u16::try_from(status.load(Ordering::SeqCst)).unwrap(),
                "{}",
                None,
            ))
        })
    });
    let bucket = gcs(fake.clone());
    let name = object_name(42);

    assert_eq!(
        bucket.put(&name, Bytes::from_static(b"{}")).await.unwrap(),
        PutOutcome::Created
    );
    let (url, auth) = seen.lock().unwrap()[0].clone();
    assert!(
        url.starts_with("https://storage.googleapis.com/upload/storage/v1/b/lk-audit-checkpoints/o?"),
        "{url}"
    );
    assert!(
        url.contains("ifGenerationMatch=0"),
        "the precondition that makes it create-only: {url}"
    );
    assert!(url.contains("uploadType=media"), "{url}");
    assert!(url.contains("name=audit%2Fcheckpoints%2F42.json"), "{url}");
    assert_eq!(auth.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    assert_eq!(fake.seen()[0].method, "POST");

    status.store(412, Ordering::SeqCst);
    assert_eq!(
        bucket.put(&name, Bytes::from_static(b"{}")).await.unwrap(),
        PutOutcome::AlreadyExists
    );
    for (code, want) in [
        (401, BucketError::Rejected),
        (403, BucketError::Rejected),
        (400, BucketError::Rejected),
        (500, BucketError::Unavailable),
        (429, BucketError::Unavailable),
    ] {
        status.store(code, Ordering::SeqCst);
        assert_eq!(
            bucket.put(&name, Bytes::from_static(b"{}")).await,
            Err(want),
            "{code}"
        );
    }
}

#[tokio::test]
async fn gcs_download_encodes_the_object_name() {
    let fake = Arc::new(FakeHttps::new(|req| {
        let url = req.url().to_string();
        if url.contains("5.json") {
            Ok(json_response(200, "{\"x\":1}", None))
        } else if url.contains("6.json") {
            Ok(json_response(404, "", None))
        } else if url.contains("7.json") {
            Ok(json_response(200, &"x".repeat(MAX_OBJECT_BYTES + 1), None))
        } else {
            Err(HttpsError::Timeout)
        }
    }));
    let bucket = gcs(fake.clone());
    assert_eq!(
        bucket.get(&object_name(5)).await.unwrap().unwrap(),
        Bytes::from_static(b"{\"x\":1}")
    );
    let url = fake.seen()[0].url.clone();
    assert_eq!(
        url,
        "https://storage.googleapis.com/storage/v1/b/lk-audit-checkpoints/o/audit%2Fcheckpoints%2F5.json?alt=media"
    );
    assert_eq!(bucket.get(&object_name(6)).await.unwrap(), None);
    assert_eq!(bucket.get(&object_name(7)).await, Err(BucketError::TooLarge));
    assert_eq!(bucket.get(&object_name(8)).await, Err(BucketError::Unavailable));
}

#[tokio::test]
async fn gcs_refuses_bad_names_before_any_call() {
    let fake = Arc::new(FakeHttps::new(|_| Ok(json_response(200, "", None))));
    let bucket = gcs(fake.clone());
    assert_eq!(
        bucket.put("../x", Bytes::new()).await,
        Err(BucketError::InvalidName)
    );
    assert_eq!(bucket.get("../x").await, Err(BucketError::InvalidName));
    assert!(fake.seen().is_empty());
    for bad in ["", "ab", "UPPER", "has space", "../x", "a/b"] {
        assert!(
            GcsCheckpointBucket::new(fake.clone(), Arc::new(FixedToken), bad).is_err(),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn the_access_token_never_appears_in_errors_or_debug() {
    let fake = Arc::new(FakeHttps::new(|_| Ok(json_response(403, TOKEN, None))));
    let bucket = gcs(fake);
    let err = bucket.put(&object_name(1), Bytes::new()).await.unwrap_err();
    let get_err = bucket.get(&object_name(1)).await.unwrap_err();
    let dump = format!(
        "{err} {err:?} {get_err} {get_err:?} {bucket:?} {:?}",
        Secret::new(TOKEN.to_owned())
    );
    assert!(!dump.contains("TOP-SECRET"), "{dump}");
}

// ---------------------------------------------------------------------------------------------------------------
// The metadata server

/// A server that answers every request with `reply(connection number)`, and counts connections.
async fn metadata_server(
    reply: impl Fn(usize) -> String + Send + Sync + 'static,
) -> (SocketAddr, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let (c, r) = (count.clone(), requests.clone());
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let n = c.fetch_add(1, Ordering::SeqCst) + 1;
            let mut buf = vec![0u8; 4096];
            let read = socket.read(&mut buf).await.unwrap();
            r.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..read]).into_owned());
            socket.write_all(reply(n).as_bytes()).await.unwrap();
            socket.shutdown().await.ok();
        }
    });
    (addr, count, requests)
}

fn http_json(status: &str, flavor: bool, body: &str) -> String {
    let flavor = if flavor { "Metadata-Flavor: Google\r\n" } else { "" };
    format!(
        "HTTP/1.1 {status}\r\n{flavor}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn metadata_token_is_fetched_with_the_flavor_header_and_cached() {
    let (addr, count, requests) = metadata_server(|n| {
        http_json(
            "200 OK",
            true,
            &format!("{{\"access_token\":\"{TOKEN}-{n}\",\"expires_in\":3599,\"token_type\":\"Bearer\"}}"),
        )
    })
    .await;
    let source = MetadataTokenSource::at(addr);
    let first = source.token().await.unwrap();
    assert_eq!(first.expose(), &format!("{TOKEN}-1"));
    let second = source.token().await.unwrap();
    assert_eq!(second.expose(), &format!("{TOKEN}-1"), "served from the cache");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let request = requests.lock().unwrap()[0].to_ascii_lowercase();
    assert!(
        request.starts_with("get /computemetadata/v1/instance/service-accounts/default/token http/1.1"),
        "{request}"
    );
    assert!(request.contains("metadata-flavor: google"), "{request}");
    assert!(request.contains("host: metadata.google.internal"), "{request}");
    assert!(!format!("{source:?}").contains("TOP-SECRET"));
}

#[tokio::test]
async fn metadata_token_near_expiry_is_fetched_again() {
    // A lifetime of 30 seconds is inside the one-minute margin: every call asks again.
    let (addr, count, _) = metadata_server(|n| {
        http_json(
            "200 OK",
            true,
            &format!("{{\"access_token\":\"t{n}\",\"expires_in\":30}}"),
        )
    })
    .await;
    let source = MetadataTokenSource::at(addr);
    assert_eq!(source.token().await.unwrap().expose(), "t1");
    assert_eq!(source.token().await.unwrap().expose(), "t2");
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn metadata_answers_that_are_not_trusted_are_refused() {
    let cases = [
        // Not from a metadata server: no flavor header.
        http_json("200 OK", false, "{\"access_token\":\"t\",\"expires_in\":3600}"),
        http_json("404 Not Found", true, "{}"),
        http_json("200 OK", true, "not json"),
        http_json("200 OK", true, "{\"access_token\":\"\",\"expires_in\":3600}"),
        http_json(
            "200 OK",
            true,
            &format!(
                "{{\"access_token\":\"{}\",\"expires_in\":3600}}",
                "a".repeat(20_000)
            ),
        ),
    ];
    for reply in cases {
        let r = reply.clone();
        let (addr, _, _) = metadata_server(move |_| r.clone()).await;
        let err = MetadataTokenSource::at(addr).token().await.unwrap_err();
        assert!(
            matches!(err, BucketError::Rejected | BucketError::Unavailable),
            "{reply:.60}: {err:?}"
        );
    }
}

#[tokio::test]
async fn metadata_server_that_is_down_is_unavailable() {
    // A port nobody listens on.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    assert_eq!(
        MetadataTokenSource::at(addr).token().await.unwrap_err(),
        BucketError::Unavailable
    );
}
