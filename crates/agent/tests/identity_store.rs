//! The certificate Secret (S5, S17): the one place the private key is written, by name, with `get` and `update` only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use agent::config::KubeName;
use agent::identity::store::{CertStore, KubeCertStore, MemoryCertStore, StoredIdentity};
use agent::identity::{ClientIdentity, KeyMaterial, KubeError, StoreError};
use bytes::Bytes;
use domain::{SwimlaneId, Timestamp};
use support::fake_kube::FakeKube;
use support::test_ca::{IssueSpec, T0_MS, TestCa};

const SECRET: &str = "lanekeeper-agent-cert";

type Entries<'a> = Vec<(&'a str, Vec<u8>)>;

fn name() -> KubeName {
    KubeName::subdomain(SECRET).unwrap()
}

fn identity() -> ClientIdentity {
    let key = KeyMaterial::generate().unwrap();
    let chain = TestCa::new().issue_for_csr(&key.csr_der().unwrap(), &IssueSpec::agent("sit1", T0_MS));
    ClientIdentity::verify(
        key,
        chain,
        &SwimlaneId::parse("sit1").unwrap(),
        Timestamp::from_unix_millis(T0_MS),
    )
    .unwrap()
}

fn kube_with_placeholder() -> FakeKube {
    FakeKube::new("sit1").with_secret(SECRET, &[("tls.crt", b""), ("tls.key", b"")])
}

fn store(kube: &FakeKube) -> KubeCertStore {
    KubeCertStore::new(kube.client(), &name())
}

#[tokio::test]
async fn save_then_load_round_trips_the_key_and_the_whole_chain() {
    let kube = kube_with_placeholder();
    let identity = identity();
    store(&kube).save(&identity).await.unwrap();

    let loaded = store(&kube).load().await.unwrap().expect("an identity was saved");
    assert_eq!(loaded.key.pkcs8_der(), identity.key().pkcs8_der());
    assert_eq!(loaded.chain_der, identity.chain_der());

    // The Secret holds PEM under the conventional names, so `kubectl` and `openssl` can read it.
    let data = kube.secret_data(SECRET).unwrap();
    let key_pem = String::from_utf8(data["tls.key"].clone()).unwrap();
    let crt_pem = String::from_utf8(data["tls.crt"].clone()).unwrap();
    assert!(key_pem.starts_with("-----BEGIN PRIVATE KEY-----"), "{key_pem}");
    assert_eq!(crt_pem.matches("-----BEGIN CERTIFICATE-----").count(), 2);
}

#[tokio::test]
async fn a_placeholder_secret_holds_no_identity_yet() {
    let kube = kube_with_placeholder();
    assert!(store(&kube).load().await.unwrap().is_none());
    // Missing entries are the same as empty ones.
    let bare = FakeKube::new("sit1").with_secret(SECRET, &[]);
    assert!(store(&bare).load().await.unwrap().is_none());
    // Half an identity is no identity either: the chart's placeholder never has one without the other.
    let key_only = FakeKube::new("sit1").with_secret(
        SECRET,
        &[
            (
                "tls.key",
                KeyMaterial::generate().unwrap().to_pem().expose().as_bytes(),
            ),
            ("tls.crt", b""),
        ],
    );
    assert!(store(&key_only).load().await.unwrap().is_none());
}

#[tokio::test]
async fn a_missing_secret_is_an_error_the_operator_can_act_on() {
    let kube = FakeKube::new("sit1");
    assert_eq!(store(&kube).load().await.unwrap_err(), StoreError::SecretMissing);
    assert_eq!(
        store(&kube).save(&identity()).await.unwrap_err(),
        StoreError::SecretMissing
    );
    assert_eq!(store(&kube).probe().await.unwrap_err(), StoreError::SecretMissing);
    let text = StoreError::SecretMissing.to_string();
    assert!(text.contains("chart must create it"), "{text}");
    // The agent has no `create` permission and must never try (A2).
    assert!(
        kube.calls()
            .iter()
            .all(|c| c.method == "GET" || c.method == "PUT"),
        "{:?}",
        kube.calls()
    );
}

#[tokio::test]
async fn saving_keeps_everything_else_in_the_secret() {
    let kube = FakeKube::new("sit1").with_secret(
        SECRET,
        &[("tls.crt", b""), ("tls.key", b""), ("keep-me", b"other data")],
    );
    kube.decorate(SECRET, ("app", "lanekeeper-agent"), "kubernetes.io/tls");
    store(&kube).save(&identity()).await.unwrap();
    let data = kube.secret_data(SECRET).unwrap();
    assert_eq!(data["keep-me"], b"other data");
    assert_eq!(kube.secret_labels(SECRET)["app"], "lanekeeper-agent");
    assert_eq!(kube.secret_type(SECRET).as_deref(), Some("kubernetes.io/tls"));
}

#[tokio::test]
async fn a_conflict_is_retried_with_a_fresh_read() {
    let kube = kube_with_placeholder();
    kube.fail_next_puts(&[409]);
    store(&kube).save(&identity()).await.unwrap();
    let methods: Vec<String> = kube.calls().iter().map(|c| c.method.clone()).collect();
    assert_eq!(methods, ["GET", "PUT", "GET", "PUT"]);
    assert!(store(&kube).load().await.unwrap().is_some());
}

#[tokio::test]
async fn persistent_conflicts_end_in_an_error_after_three_tries() {
    let kube = kube_with_placeholder();
    kube.fail_next_puts(&[409, 409, 409, 409, 409]);
    let error = store(&kube).save(&identity()).await.unwrap_err();
    assert_eq!(
        error,
        StoreError::Api(KubeError::Status {
            op: "update the certificate Secret",
            status: 409
        })
    );
    let puts = kube.calls().iter().filter(|c| c.method == "PUT").count();
    assert_eq!(puts, 3);
}

#[tokio::test]
async fn a_forbidden_update_is_reported_with_its_status_and_not_retried() {
    let kube = kube_with_placeholder();
    kube.fail_next_puts(&[403]);
    let error = store(&kube).save(&identity()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "the Kubernetes API answered 403 to update the certificate Secret"
    );
    assert_eq!(kube.calls().iter().filter(|c| c.method == "PUT").count(), 1);
}

#[tokio::test]
async fn an_unreadable_secret_is_reported_with_its_status() {
    let kube = kube_with_placeholder();
    kube.deny_everything(403);
    let error = store(&kube).load().await.unwrap_err();
    assert_eq!(
        error,
        StoreError::Api(KubeError::Status {
            op: "read the certificate Secret",
            status: 403
        })
    );
}

#[tokio::test]
async fn damaged_contents_are_malformed_and_not_an_identity() {
    let pem_key = KeyMaterial::generate().unwrap().to_pem();
    let good = identity();
    let chain_pem = {
        let kube = kube_with_placeholder();
        store(&kube).save(&good).await.unwrap();
        kube.secret_data(SECRET).unwrap()["tls.crt"].clone()
    };
    let cases: Vec<(&str, Entries)> = vec![
        (
            "garbage key",
            vec![("tls.key", b"garbage".to_vec()), ("tls.crt", chain_pem.clone())],
        ),
        (
            "garbage chain",
            vec![
                ("tls.key", pem_key.expose().clone().into_bytes()),
                ("tls.crt", b"garbage".to_vec()),
            ],
        ),
        (
            "a certificate block that is not base64",
            vec![
                ("tls.key", pem_key.expose().clone().into_bytes()),
                (
                    "tls.crt",
                    b"-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n".to_vec(),
                ),
            ],
        ),
        (
            "more certificates than the contract allows",
            vec![
                ("tls.key", pem_key.expose().clone().into_bytes()),
                ("tls.crt", chain_pem.repeat(5)),
            ],
        ),
    ];
    for (what, entries) in cases {
        let refs: Vec<(&str, &[u8])> = entries.iter().map(|(k, v)| (*k, v.as_slice())).collect();
        let kube = FakeKube::new("sit1").with_secret(SECRET, &refs);
        assert_eq!(
            store(&kube).load().await.unwrap_err(),
            StoreError::Malformed,
            "{what}"
        );
    }
}

#[tokio::test]
async fn only_get_and_put_on_the_named_secret_ever_reach_the_api() {
    let kube = kube_with_placeholder();
    let store = store(&kube);
    store.probe().await.unwrap();
    store.load().await.unwrap();
    store.save(&identity()).await.unwrap();
    store.load().await.unwrap();
    let path = kube.secret_path(SECRET);
    for call in kube.calls() {
        assert!(call.method == "GET" || call.method == "PUT", "{call:?}");
        assert_eq!(call.path, path, "{call:?}");
        // No list or watch: those are query parameters on the collection, and the path above rules out the collection.
        assert!(call.query.as_deref().unwrap_or("").is_empty(), "{call:?}");
    }
}

#[tokio::test]
async fn probing_does_not_change_the_secret() {
    let kube = FakeKube::new("sit1").with_secret(SECRET, &[("tls.crt", b""), ("tls.key", b""), ("x", b"y")]);
    let before = (kube.secret_data(SECRET).unwrap(), kube.resource_version(SECRET));
    store(&kube).probe().await.unwrap();
    assert_eq!(
        (kube.secret_data(SECRET).unwrap(), kube.resource_version(SECRET)),
        before
    );
    assert_eq!(
        kube.calls().iter().map(|c| c.method.as_str()).collect::<Vec<_>>(),
        ["GET", "PUT"]
    );
}

#[tokio::test]
async fn probing_finds_a_missing_update_permission_before_a_token_is_spent() {
    let kube = kube_with_placeholder();
    kube.fail_next_puts(&[403]);
    let error = store(&kube).probe().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "the Kubernetes API answered 403 to update the certificate Secret"
    );
}

#[tokio::test]
async fn the_memory_store_keeps_the_last_identity_and_counts_saves() {
    let memory = MemoryCertStore::new();
    assert!(memory.load().await.unwrap().is_none());
    memory.probe().await.unwrap();
    let identity = identity();
    memory.save(&identity).await.unwrap();
    memory.save(&identity).await.unwrap();
    assert_eq!(memory.saves(), 2);
    let StoredIdentity { key, chain_der } = memory.load().await.unwrap().unwrap();
    assert_eq!(key.pkcs8_der(), identity.key().pkcs8_der());
    assert_eq!(chain_der, identity.chain_der());
    // A pre-seeded store, for a test that starts with a stored identity.
    let seeded = MemoryCertStore::with(KeyMaterial::generate().unwrap(), vec![Bytes::from_static(b"x")]);
    assert_eq!(seeded.load().await.unwrap().unwrap().chain_der.len(), 1);
}
