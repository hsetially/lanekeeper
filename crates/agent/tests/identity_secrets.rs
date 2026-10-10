//! What must never leak (S5, S10, S21): the private key leaves memory only for the certificate Secret, and no token,
//! key or certificate Secret value reaches a log line.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent::config::{KubeName, Settings};
use agent::identity::joiner::{IdentityHandle, Joiner};
use agent::identity::jointoken::JoinTokenSource;
use agent::identity::store::{CertStore, KubeCertStore, MemoryCertStore, StoredIdentity};
use agent::identity::{ClientIdentity, IdTokenError, IdentityError, JoinError, KeyMaterial, StoreError};
use k8s_openapi::api::core::v1::Secret;
use support::clock::TestClock;
use support::fake_hub::{FakeHub, ScriptedIdTokens, ScriptedJoinTokens};
use support::fake_kube::FakeKube;
use support::log_capture::LogCapture;
use support::test_ca::{IssueSpec, T0_MS};

const CERT_SECRET: &str = "lanekeeper-agent-cert";
const WI_TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.UEFZTE9BRC1NQVJLRVItV0k.U0lHTkFUVVJFLU1BUktFUi1XSQ";
const JOIN_TOKEN: &str = "lk_join_MARKER_9f8e7d6c5b4a";

fn settings(mode: &str) -> Settings {
    let mut env = support::valid_env(Path::new("/nonexistent"));
    env.insert("LK_JOIN_MODE".into(), mode.into());
    env.insert("LK_JOIN_TOKEN_SECRET".into(), "lanekeeper-join-token".into());
    Settings::from_env(&env).unwrap()
}

/// The 32-byte private scalar inside a PKCS#8 P-256 key.
fn private_scalar(pkcs8: &[u8]) -> Vec<u8> {
    let marker = [0x02_u8, 0x01, 0x01, 0x04, 0x20];
    let at = pkcs8.windows(marker.len()).position(|w| w == marker).unwrap() + marker.len();
    pkcs8[at..at + 32].to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

struct Rig {
    kube: FakeKube,
    hub: Arc<FakeHub>,
    joiner: Joiner,
}

fn rig(mode: &str, wi: Result<&str, IdTokenError>) -> Rig {
    let kube = FakeKube::new("sit1").with_secret(CERT_SECRET, &[("tls.crt", b""), ("tls.key", b"")]);
    let clock = Arc::new(TestClock::starting_at(T0_MS));
    let hub = FakeHub::new(clock.clone());
    let store = KubeCertStore::new(kube.client(), &KubeName::subdomain(CERT_SECRET).unwrap());
    let joiner = Joiner::new(
        &settings(mode),
        ScriptedIdTokens::new(wi),
        Some(ScriptedJoinTokens::new(Ok(JOIN_TOKEN)) as Arc<dyn JoinTokenSource>),
        hub.clone(),
        Arc::new(store),
        clock,
    );
    Rig { kube, hub, joiner }
}

#[tokio::test]
async fn private_key_only_written_to_cert_secret() {
    let Rig { kube, hub, joiner } = rig("auto", Ok(WI_TOKEN));
    let identity = joiner.join().await.unwrap();
    let scalar = private_scalar(identity.key().pkcs8_der());

    // The Kubernetes API saw only the certificate Secret, and the key is in it, as the key of the issued certificate.
    let path = kube.secret_path(CERT_SECRET);
    let mut put_bodies = 0;
    for call in kube.calls() {
        assert_eq!(call.path, path, "{call:?}");
        if call.method == "PUT" {
            put_bodies += 1;
            let sent: Secret = serde_json::from_slice(&call.body).unwrap();
            assert_eq!(sent.metadata.name.as_deref(), Some(CERT_SECRET));
        } else {
            assert!(call.body.is_empty(), "{call:?}");
        }
    }
    assert_eq!(
        put_bodies, 2,
        "the probe writes the Secret back unchanged, and the join writes the key"
    );
    let stored = KeyMaterial::from_pem(&kube.secret_data(CERT_SECRET).unwrap()["tls.key"]).unwrap();
    assert_eq!(stored.pkcs8_der(), identity.key().pkcs8_der());
    assert_eq!(stored.spki_der(), identity.key().spki_der());

    // Nowhere else did the key go: not into the signing request, not into the credential the hub was shown.
    let joins = hub.joins();
    assert_eq!(joins.len(), 1);
    assert!(
        !contains(&joins[0].csr, &scalar),
        "the private scalar is in the CSR"
    );
    assert!(!contains(joins[0].credential.as_bytes(), &scalar));
    assert!(!contains(joins[0].credential.as_bytes(), hex(&scalar).as_bytes()));
    // And the CSR is for the public half of the stored key.
    assert!(contains(
        &joins[0].csr,
        &identity.key().spki_der()[identity.key().spki_der().len() - 65..]
    ));
}

#[tokio::test]
async fn no_other_secret_and_no_listing_is_ever_asked_for() {
    let Rig { kube, joiner, .. } = rig("auto", Ok(WI_TOKEN));
    joiner.join().await.unwrap();
    for call in kube.calls() {
        assert!(["GET", "PUT"].contains(&call.method.as_str()), "{call:?}");
        assert!(call.query.as_deref().unwrap_or("").is_empty(), "{call:?}");
        assert!(
            call.path.ends_with(&format!("/secrets/{CERT_SECRET}")),
            "{call:?}"
        );
    }
}

/// A store that can be probed but not written, as when the API server fails between the two calls.
#[derive(Debug)]
struct WriteFails(MemoryCertStore);

#[async_trait::async_trait]
impl CertStore for WriteFails {
    async fn load(&self) -> Result<Option<StoredIdentity>, StoreError> {
        self.0.load().await
    }
    async fn save(&self, _: &ClientIdentity) -> Result<(), StoreError> {
        Err(StoreError::SecretMissing)
    }
    async fn probe(&self) -> Result<(), StoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_certificate_that_cannot_be_saved_is_still_used_and_the_failure_is_logged() {
    let capture = LogCapture::new();
    let _guard = capture.install();
    let clock = Arc::new(TestClock::starting_at(T0_MS));
    let hub = FakeHub::new(clock.clone());
    let joiner = Joiner::new(
        &settings("auto"),
        ScriptedIdTokens::new(Ok(WI_TOKEN)),
        None,
        hub,
        Arc::new(WriteFails(MemoryCertStore::new())),
        clock,
    );
    let identity = joiner.join().await.unwrap();
    assert_eq!(identity.swimlane().as_str(), "sit1");
    let logs = capture.text();
    assert!(logs.contains("could not store the certificate"), "{logs}");
    assert!(logs.contains("ERROR"), "{logs}");
}

#[tokio::test(start_paused = true)]
async fn tokens_never_in_logs() {
    let capture = LogCapture::new();
    let _guard = capture.install();
    let mut secrets: Vec<String> = vec![
        WI_TOKEN.to_owned(),
        JOIN_TOKEN.to_owned(),
        // Every dot-separated part of the Google token, which is what a partial print would show.
    ];
    secrets.extend(WI_TOKEN.split('.').map(str::to_owned));

    // 1. Workload Identity succeeds and the certificate is stored in the (fake) cluster.
    let one = rig("auto", Ok(WI_TOKEN));
    let identity_one = one.joiner.join().await.unwrap();
    // 2. The metadata server is down, so the join token is used.
    let two = rig("auto", Err(IdTokenError::Unavailable));
    let identity_two = two.joiner.join().await.unwrap();
    // 3. The hub refuses the credential.
    let three = rig("auto", Ok(WI_TOKEN));
    three.hub.reject_next_joins(1);
    assert_eq!(
        three.joiner.join().await.unwrap_err(),
        IdentityError::Join(JoinError::Rejected)
    );
    // 4. The hub issues an identity the agent will not accept.
    let four = rig("auto", Ok(WI_TOKEN));
    four.hub.issue_with(|_, now_ms| {
        IssueSpec::agent("sit1", now_ms).with_uris(&["spiffe://lanekeeper/agent/sit1"])
    });
    assert!(four.joiner.join().await.is_err());
    // 5. The cluster refuses to let the agent read its Secret.
    let five = rig("auto", Ok(WI_TOKEN));
    five.kube.deny_everything(403);
    assert!(five.joiner.join().await.is_err());
    // 6. A renewal and a re-join in the maintenance loop.
    let six = rig("auto", Ok(WI_TOKEN));
    let first = six.joiner.join().await.unwrap();
    let handle = Arc::new(IdentityHandle::new(first));
    let (joiner, hub, handle2) = (Arc::new(six.joiner), six.hub.clone(), handle.clone());
    let task = tokio::spawn(async move {
        let never = joiner.maintain(&handle2, hub.as_ref()).await;
        match never {}
    });
    tokio::time::sleep(Duration::from_secs(13 * 3600)).await;
    six.hub.stream_down();
    six.hub.reject_next_joins(2);
    tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    task.abort();

    // The key of every identity, in every form it could be printed.
    for key in [identity_one.key(), identity_two.key(), handle.current().key()] {
        let pem = key.to_pem();
        secrets.extend(
            pem.expose()
                .lines()
                .filter(|l| !l.starts_with("-----"))
                .map(str::to_owned),
        );
        let scalar = private_scalar(key.pkcs8_der());
        secrets.push(hex(&scalar));
        secrets.push(hex(key.pkcs8_der()));
        secrets.push(format!("{:?}", key.pkcs8_der()));
        secrets.push(format!("{scalar:?}"));
    }

    let logs = capture.text();
    assert!(
        logs.contains("joined the hub"),
        "the capture must have caught the join:\n{logs}"
    );
    assert!(logs.contains("joining with the join token"), "{logs}");
    assert!(logs.contains("renewed the certificate"), "{logs}");
    assert!(logs.contains("joining again failed"), "{logs}");
    assert!(logs.contains("the hub refused the credential"), "{logs}");
    for secret in &secrets {
        assert!(
            !logs.contains(secret.as_str()),
            "a secret reached the logs: {secret}"
        );
    }
}
