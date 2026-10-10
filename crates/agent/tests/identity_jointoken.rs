//! The join-token fallback Secret (S5, Q26, A2): read with `get`, by name, only when it is needed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use agent::config::KubeName;
use agent::identity::jointoken::{JoinTokenSource, KubeSecretJoinToken};
use agent::identity::{JoinTokenError, KubeError};
use support::fake_kube::FakeKube;

const SECRET: &str = "lanekeeper-join-token";

type Entries<'a> = Vec<(&'a str, &'a [u8])>;

fn source(kube: &FakeKube) -> KubeSecretJoinToken {
    KubeSecretJoinToken::new(kube.client(), &KubeName::subdomain(SECRET).unwrap())
}

async fn read(entries: &[(&str, &[u8])]) -> Result<String, JoinTokenError> {
    let kube = FakeKube::new("sit1").with_secret(SECRET, entries);
    source(&kube).join_token().await.map(|t| t.expose().clone())
}

#[tokio::test]
async fn reads_the_token_entry_without_a_trailing_newline() {
    assert_eq!(
        read(&[("token", b"lk_join_abc123\n")]).await.unwrap(),
        "lk_join_abc123"
    );
    assert_eq!(
        read(&[("token", b"lk_join_abc123")]).await.unwrap(),
        "lk_join_abc123"
    );
}

#[tokio::test]
async fn the_token_is_a_secret() {
    let kube = FakeKube::new("sit1").with_secret(SECRET, &[("token", b"lk_join_abc123")]);
    let token = source(&kube).join_token().await.unwrap();
    assert_eq!(format!("{token:?}"), "[redacted]");
}

#[tokio::test]
async fn a_missing_secret_is_reported() {
    let kube = FakeKube::new("sit1");
    assert_eq!(
        source(&kube).join_token().await.unwrap_err(),
        JoinTokenError::SecretMissing
    );
}

#[tokio::test]
async fn a_secret_without_a_usable_token_entry_is_reported() {
    let long = vec![b'a'; 9000];
    let cases: Vec<(&str, Entries)> = vec![
        ("no entry", vec![]),
        ("other entry only", vec![("not-token", b"abc")]),
        ("empty", vec![("token", b"")]),
        ("blank", vec![("token", b" \n")]),
        ("space inside", vec![("token", b"abc def")]),
        ("newline inside", vec![("token", b"abc\ndef")]),
        ("control character", vec![("token", b"abc\x07def")]),
        ("not utf-8", vec![("token", b"abc\xff\xfedef")]),
        ("too long", vec![("token", long.as_slice())]),
    ];
    for (what, entries) in cases {
        assert_eq!(
            read(&entries).await.unwrap_err(),
            JoinTokenError::Missing,
            "{what}"
        );
    }
}

#[tokio::test]
async fn api_failures_carry_the_status_and_nothing_else() {
    let kube = FakeKube::new("sit1").with_secret(SECRET, &[("token", b"lk_join_abc123")]);
    kube.deny_everything(403);
    let error = source(&kube).join_token().await.unwrap_err();
    assert_eq!(
        error,
        JoinTokenError::Api(KubeError::Status {
            op: "read the join token Secret",
            status: 403
        })
    );
    assert!(!error.to_string().contains("lk_join"));
}

#[tokio::test]
async fn only_a_get_of_the_named_secret_is_ever_sent() {
    let kube = FakeKube::new("sit1").with_secret(SECRET, &[("token", b"lk_join_abc123")]);
    let source = source(&kube);
    source.join_token().await.unwrap();
    source.join_token().await.unwrap();
    let calls = kube.calls();
    assert_eq!(calls.len(), 2, "the Secret is read each time, never cached (A23)");
    for call in calls {
        assert_eq!(call.method, "GET");
        assert_eq!(call.path, kube.secret_path(SECRET));
        assert!(call.query.as_deref().unwrap_or("").is_empty());
    }
}
