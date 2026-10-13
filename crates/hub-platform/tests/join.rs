//! T3: `Join` (S5, S9, S10). Both credential paths against a real Postgres, a real CA over a KMS-style signer, and the audit
//! chain in the same transaction as the token use.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

mod support;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{Secret, ShortText, SwimlaneId};
use hub_platform::clock::Clock;
use hub_platform::join::{JoinError, JoinLimits, JoinService, JoinSettings, Refusal};
use ports::GoogleIdentity;
use ports::fakes::FakeTokenVerifier;
use proto::convert::{JoinCredential, JoinParams, JoinSubject};
use sqlx::{PgPool, Row};
use support::ca::{TestCa, fresh_csr};
use support::log_capture::LogCapture;
use support::pg::{PgTestAuditLog, TestDb};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

const AUDIENCE: &str = "https://hub.lanekeeper.test";
const SERVICE_ACCOUNT: &str = "agent-sit1@project.iam.gserviceaccount.com";

struct Rig {
    db: TestDb,
    pool: PgPool,
    ca: TestCa,
    verifier: FakeTokenVerifier,
    audit: Arc<PgTestAuditLog>,
    join: JoinService,
}

impl Rig {
    async fn new() -> Option<Self> {
        Self::with_limits(JoinLimits::default()).await
    }

    async fn with_limits(limits: JoinLimits) -> Option<Self> {
        let db = TestDb::ready().await?;
        let pool = db.app_pool().await;
        let ca = TestCa::new().await;
        let verifier = FakeTokenVerifier::new();
        let audit = Arc::new(PgTestAuditLog::default());
        let join = JoinService::new(
            pool.clone(),
            Arc::new(verifier.clone()),
            audit.clone(),
            Arc::new(clone_issuer(&ca).await),
            ca.clock.clone(),
            JoinSettings {
                hub_audience: ShortText::parse(AUDIENCE).unwrap(),
                limits,
            },
        );
        Some(Self {
            db,
            pool,
            ca,
            verifier,
            audit,
            join,
        })
    }

    fn accept_google(&self, token: &str, email: &str) {
        self.verifier.accept_google(
            token,
            AUDIENCE,
            GoogleIdentity {
                subject: ShortText::parse("1234567890").unwrap(),
                email: ShortText::parse(email).unwrap(),
                audience: ShortText::parse(AUDIENCE).unwrap(),
            },
        );
    }

    async fn agent_rows(&self) -> i64 {
        self.db.count(&self.pool, "SELECT count(*) FROM agents").await
    }

    async fn audit_rows(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1")
            .bind(action)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn token_used(&self, id: i64) -> bool {
        sqlx::query_scalar::<_, bool>("SELECT used_at IS NOT NULL FROM join_tokens WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

/// A second `CaIssuer` over the same CA certificate and the same KMS key as the test CA's.
async fn clone_issuer(ca: &TestCa) -> hub_platform::ca::CaIssuer {
    let signer: Arc<dyn ports::KmsSigner> = ca.soft.clone();
    hub_platform::ca::CaIssuer::new(signer, ca.key.clone(), ca.ca_der.clone())
        .await
        .unwrap()
        .with_clock(ca.clock.clone())
}

macro_rules! rig {
    () => {
        match Rig::new().await {
            Some(rig) => rig,
            None => return,
        }
    };
}

fn swimlane(id: &str) -> SwimlaneId {
    SwimlaneId::parse(id).unwrap()
}

fn google_params(swimlane_id: &str, token: &str, csr: &[u8]) -> JoinParams {
    JoinParams {
        subject: JoinSubject::Agent(swimlane(swimlane_id)),
        csr_der: Bytes::copy_from_slice(csr),
        credential: JoinCredential::GoogleIdToken(Secret::new(token.to_owned())),
    }
}

fn token_params(swimlane_id: &str, token: &str, csr: &[u8]) -> JoinParams {
    JoinParams {
        subject: JoinSubject::Agent(swimlane(swimlane_id)),
        csr_der: Bytes::copy_from_slice(csr),
        credential: JoinCredential::JoinToken(Secret::new(token.to_owned())),
    }
}

fn san_uris(der: &[u8]) -> Vec<String> {
    let (_, cert) = X509Certificate::from_der(der).unwrap();
    cert.subject_alternative_name()
        .unwrap()
        .map(|ext| {
            ext.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    GeneralName::URI(u) => Some((*u).to_owned()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn refused(error: JoinError) -> Option<Refusal> {
    match error {
        JoinError::Refused(reason) => Some(reason),
        _ => None,
    }
}

// --------------------------------------------------------------------------------------------- Google ID token

#[tokio::test]
async fn join_with_google_id_token_issues_24h_cert_with_swimlane_san() {
    let rig = rig!();
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.accept_google("gid-token-1", SERVICE_ACCOUNT);
    let (_key, csr) = fresh_csr();

    let issued = rig
        .join
        .join_agent(None, google_params("sit1", "gid-token-1", &csr))
        .await
        .unwrap();

    assert_eq!(issued.cert_chain_der.len(), 2);
    assert_eq!(
        san_uris(&issued.cert_chain_der[0]),
        ["spiffe://lanekeeper/swimlane/sit1"]
    );
    let (_, leaf) = X509Certificate::from_der(&issued.cert_chain_der[0]).unwrap();
    let life = leaf.validity().not_after.timestamp() - leaf.validity().not_before.timestamp();
    assert_eq!(
        life,
        24 * 3600 + 300,
        "24 hours, and the five minutes of back-dating"
    );
    assert_eq!(
        issued.not_after.unix_millis(),
        rig.ca.clock.now().unix_millis() + 24 * 3600 * 1000
    );

    // The join is recorded: the agent's certificate, and one audit event that exists because the transaction committed.
    let row = sqlx::query(
        "SELECT cert_serial, joined_at IS NOT NULL AS joined FROM agents WHERE swimlane_id = 'sit1'",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        row.get::<String, _>("cert_serial"),
        hex::encode(leaf.raw_serial())
    );
    assert!(row.get::<bool, _>("joined"));
    assert_eq!(rig.audit_rows("agent_joined").await, 1);
}

#[tokio::test]
async fn join_google_email_must_equal_registered_service_account() {
    let rig = rig!();
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.accept_google("gid-other", "someone-else@project.iam.gserviceaccount.com");
    let (_key, csr) = fresh_csr();

    let error = rig
        .join
        .join_agent(None, google_params("sit1", "gid-other", &csr))
        .await
        .unwrap_err();

    assert_eq!(refused(error), Some(Refusal::EmailMismatch));
    assert_eq!(rig.agent_rows().await, 0, "no certificate recorded");
    assert_eq!(
        rig.audit_rows("agent_joined").await,
        0,
        "and nothing audited as a join"
    );
}

#[tokio::test]
async fn join_google_email_comparison_ignores_case_only() {
    let rig = rig!();
    rig.db
        .seed_swimlane(
            &rig.pool,
            "sit1",
            Some("Agent-SIT1@Project.iam.gserviceaccount.com"),
        )
        .await;
    rig.accept_google("gid-case", SERVICE_ACCOUNT);
    rig.accept_google(
        "gid-prefix",
        "agent-sit1@project.iam.gserviceaccount.com.evil.test",
    );
    let (_key, csr) = fresh_csr();
    assert!(
        rig.join
            .join_agent(None, google_params("sit1", "gid-case", &csr))
            .await
            .is_ok()
    );
    let (_key, csr) = fresh_csr();
    let error = rig
        .join
        .join_agent(None, google_params("sit1", "gid-prefix", &csr))
        .await
        .unwrap_err();
    assert_eq!(
        refused(error),
        Some(Refusal::EmailMismatch),
        "a longer address is a different address"
    );
}

#[tokio::test]
async fn join_without_registered_service_account_is_refused() {
    let rig = rig!();
    rig.db.seed_swimlane(&rig.pool, "sit1", None).await;
    // Even an empty registered value and an empty email must not match each other.
    rig.accept_google("gid-any", SERVICE_ACCOUNT);
    let (_key, csr) = fresh_csr();
    let error = rig
        .join
        .join_agent(None, google_params("sit1", "gid-any", &csr))
        .await
        .unwrap_err();
    assert_eq!(refused(error), Some(Refusal::UnknownOrUnregisteredSwimlane));
    // A swimlane that does not exist is refused the same way.
    let error = rig
        .join
        .join_agent(None, google_params("no-such-lane", "gid-any", &csr))
        .await
        .unwrap_err();
    assert_eq!(refused(error), Some(Refusal::UnknownOrUnregisteredSwimlane));
    assert_eq!(rig.agent_rows().await, 0);
}

#[tokio::test]
async fn join_google_token_for_another_audience_or_unknown_is_refused() {
    let rig = rig!();
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.verifier.accept_google(
        "gid-wrong-audience",
        "https://some-other-service.test",
        GoogleIdentity {
            subject: ShortText::parse("1").unwrap(),
            email: ShortText::parse(SERVICE_ACCOUNT).unwrap(),
            audience: ShortText::parse("https://some-other-service.test").unwrap(),
        },
    );
    let (_key, csr) = fresh_csr();
    for token in ["gid-wrong-audience", "gid-never-seen"] {
        let error = rig
            .join
            .join_agent(None, google_params("sit1", token, &csr))
            .await
            .unwrap_err();
        assert_eq!(refused(error), Some(Refusal::BadIdToken), "{token}");
    }
}

// ----------------------------------------------------------------------------------------------- join token

#[tokio::test]
async fn join_token_is_single_use() {
    let rig = rig!();
    rig.db.seed_swimlane(&rig.pool, "sit1", None).await;
    let id = rig
        .db
        .seed_join_token(&rig.pool, "sit1", "jt-secret-1", 60, 3600)
        .await;
    let (_key, csr) = fresh_csr();

    let first = rig
        .join
        .join_agent(None, token_params("sit1", "jt-secret-1", &csr))
        .await
        .unwrap();
    assert_eq!(
        san_uris(&first.cert_chain_der[0]),
        ["spiffe://lanekeeper/swimlane/sit1"]
    );
    assert!(rig.token_used(id).await);

    let (_key, csr) = fresh_csr();
    let error = rig
        .join
        .join_agent(None, token_params("sit1", "jt-secret-1", &csr))
        .await
        .unwrap_err();
    assert_eq!(
        refused(error),
        Some(Refusal::NoLiveToken),
        "the used token is not live"
    );
    assert_eq!(
        rig.audit_rows("agent_joined").await,
        1,
        "one join, one audit event"
    );
}

#[tokio::test]
async fn join_token_used_twice_at_once_succeeds_exactly_once() {
    let rig = rig!();
    rig.db.seed_swimlane(&rig.pool, "sit1", None).await;
    rig.db
        .seed_join_token(&rig.pool, "sit1", "jt-race", 60, 3600)
        .await;
    let (_k1, csr1) = fresh_csr();
    let (_k2, csr2) = fresh_csr();
    let (a, b) = tokio::join!(
        rig.join.join_agent(None, token_params("sit1", "jt-race", &csr1)),
        rig.join.join_agent(None, token_params("sit1", "jt-race", &csr2)),
    );
    let successes = [a.is_ok(), b.is_ok()].into_iter().filter(|ok| *ok).count();
    assert_eq!(successes, 1, "{a:?} {b:?}");
    assert_eq!(rig.audit_rows("agent_joined").await, 1);
}

#[tokio::test]
async fn join_token_expires_after_24h() {
    let rig = rig!();
    rig.db.seed_swimlane(&rig.pool, "sit1", None).await;
    let expired = rig
        .db
        .seed_join_token(&rig.pool, "sit1", "jt-expired", 25 * 3600, -3600)
        .await;
    // An issuer that wrote a long expiry does not extend the life: created 25 hours ago, `expires_at` in 30 days.
    let long = rig
        .db
        .seed_join_token(&rig.pool, "sit1", "jt-long", 25 * 3600, 30 * 24 * 3600)
        .await;
    let fresh = rig
        .db
        .seed_join_token(&rig.pool, "sit1", "jt-fresh", 23 * 3600, 3600)
        .await;
    for token in ["jt-expired", "jt-long"] {
        let (_key, csr) = fresh_csr();
        let error = rig
            .join
            .join_agent(None, token_params("sit1", token, &csr))
            .await
            .unwrap_err();
        assert_eq!(refused(error), Some(Refusal::NoLiveToken), "{token}");
    }
    assert!(!rig.token_used(expired).await && !rig.token_used(long).await);
    let (_key, csr) = fresh_csr();
    assert!(
        rig.join
            .join_agent(None, token_params("sit1", "jt-fresh", &csr))
            .await
            .is_ok()
    );
    assert!(rig.token_used(fresh).await);
}

#[tokio::test]
async fn join_token_is_bound_to_one_swimlane() {
    let rig = rig!();
    rig.db.seed_swimlane(&rig.pool, "sit1", None).await;
    rig.db.seed_swimlane(&rig.pool, "sit2", None).await;
    let id = rig
        .db
        .seed_join_token(&rig.pool, "sit1", "jt-for-sit1", 60, 3600)
        .await;
    let (_key, csr) = fresh_csr();
    let error = rig
        .join
        .join_agent(None, token_params("sit2", "jt-for-sit1", &csr))
        .await
        .unwrap_err();
    assert_eq!(refused(error), Some(Refusal::NoLiveToken));
    assert!(
        !rig.token_used(id).await,
        "a wrong-swimlane attempt does not burn the token"
    );
    let (_key, csr) = fresh_csr();
    assert!(
        rig.join
            .join_agent(None, token_params("sit1", "jt-for-sit1", &csr))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn join_token_is_stored_and_compared_hashed() {
    let rig = rig!();
    rig.db.seed_swimlane(&rig.pool, "sit1", None).await;
    let id = rig
        .db
        .seed_join_token(&rig.pool, "sit1", "jt-plaintext-never-stored", 60, 3600)
        .await;
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash::bytea FROM join_tokens WHERE id = $1")
        .bind(id)
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 32);
    assert_ne!(stored.as_slice(), b"jt-plaintext-never-stored".as_slice());
    let expected =
        hub_platform::join::token::hash_token(&Secret::new("jt-plaintext-never-stored".to_owned()));
    assert_eq!(stored, expected.to_vec(), "SHA-256 of the token text");

    // A token whose hash is one bit off is a different token.
    let near = hub_platform::join::token::hash_token(&Secret::new("jt-plaintext-never-stored ".to_owned()));
    assert_ne!(near, expected);
    // The comparison is the `subtle` one: the source has no `==` between hashes.
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/join/token.rs")).unwrap();
    assert!(source.contains("ct_eq"), "hashes are compared with subtle");
    assert!(!source.contains("== candidate") && !source.contains("candidate =="));
}

#[tokio::test]
async fn join_failures_are_indistinguishable() {
    let rig = rig!();
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.db.seed_swimlane(&rig.pool, "sit2", None).await;
    rig.accept_google("gid-ok-for-sit1", SERVICE_ACCOUNT);
    rig.accept_google("gid-other", "other@project.iam.gserviceaccount.com");
    rig.db
        .seed_join_token(&rig.pool, "sit2", "jt-used", 60, 3600)
        .await;
    let (_key, csr) = fresh_csr();
    rig.join
        .join_agent(None, token_params("sit2", "jt-used", &csr))
        .await
        .unwrap();
    rig.db
        .seed_join_token(&rig.pool, "sit2", "jt-expired", 60, -60)
        .await;

    let mut codes = Vec::new();
    for params in [
        google_params("sit1", "gid-other", &csr),
        google_params("sit1", "no-such-token", &csr),
        google_params("sit2", "gid-ok-for-sit1", &csr),
        google_params("nowhere", "gid-ok-for-sit1", &csr),
        token_params("sit2", "jt-used", &csr),
        token_params("sit2", "jt-expired", &csr),
        token_params("sit2", "jt-never-existed", &csr),
        token_params("sit1", "jt-used", &csr),
        google_params("sit1", "gid-ok-for-sit1", b"not a csr"),
    ] {
        let error = rig.join.join_agent(None, params).await.unwrap_err();
        assert!(matches!(error, JoinError::Refused(_)), "{error:?}");
        codes.push(error.status_code());
        // The caller-visible text carries no reason either.
        assert_eq!(error.to_string(), "join refused");
    }
    assert!(
        codes.iter().all(|c| *c == tonic::Code::Unauthenticated),
        "{codes:?}"
    );
}

#[tokio::test]
async fn join_audit_event_is_in_the_same_transaction() {
    let rig = rig!();
    rig.db.seed_swimlane(&rig.pool, "sit1", None).await;
    let id = rig
        .db
        .seed_join_token(&rig.pool, "sit1", "jt-audit", 60, 3600)
        .await;
    let (_key, csr) = fresh_csr();

    // The audit write fails: the token use, the agent's certificate and the event all roll back, and no certificate is sent.
    rig.audit.set_failing(true);
    let error = rig
        .join
        .join_agent(None, token_params("sit1", "jt-audit", &csr))
        .await
        .unwrap_err();
    assert_eq!(error, JoinError::Unavailable);
    assert!(
        !rig.token_used(id).await,
        "a failing audit write rolls the token use back"
    );
    assert_eq!(rig.agent_rows().await, 0);
    assert_eq!(rig.audit_rows("agent_joined").await, 0);

    // The same token still works once the audit log does, and then the three exist together.
    rig.audit.set_failing(false);
    assert!(
        rig.join
            .join_agent(None, token_params("sit1", "jt-audit", &csr))
            .await
            .is_ok()
    );
    assert!(rig.token_used(id).await);
    assert_eq!(rig.agent_rows().await, 1);
    assert_eq!(rig.audit_rows("agent_joined").await, 1);
}

#[tokio::test]
async fn join_audit_event_for_the_google_path_is_in_the_same_transaction() {
    let rig = rig!();
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.accept_google("gid-audit", SERVICE_ACCOUNT);
    let (_key, csr) = fresh_csr();
    rig.audit.set_failing(true);
    assert_eq!(
        rig.join
            .join_agent(None, google_params("sit1", "gid-audit", &csr))
            .await
            .unwrap_err(),
        JoinError::Unavailable
    );
    assert_eq!(
        rig.agent_rows().await,
        0,
        "the agent's certificate is recorded only with its audit event"
    );
}

#[tokio::test]
async fn join_attempts_are_limited_per_swimlane_and_per_address() {
    let limits = JoinLimits {
        per_swimlane: 3,
        per_address: 5,
        window: Duration::from_secs(3600),
        max_tracked: 100,
    };
    let Some(rig) = Rig::with_limits(limits).await else {
        return;
    };
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.db
        .seed_swimlane(&rig.pool, "sit2", Some(SERVICE_ACCOUNT))
        .await;
    rig.accept_google("gid-wrong", "other@project.iam.gserviceaccount.com");
    rig.accept_google("gid-right", SERVICE_ACCOUNT);
    let (_key, csr) = fresh_csr();
    let ip: IpAddr = "10.1.2.3".parse().unwrap();

    for _ in 0..3 {
        let error = rig
            .join
            .join_agent(Some(ip), google_params("sit1", "gid-wrong", &csr))
            .await
            .unwrap_err();
        assert!(matches!(error, JoinError::Refused(_)));
    }
    // The fourth attempt for sit1 is refused before anything is checked, even with the right credential.
    let error = rig
        .join
        .join_agent(Some(ip), google_params("sit1", "gid-right", &csr))
        .await
        .unwrap_err();
    assert_eq!(error, JoinError::RateLimited);
    assert_eq!(error.status_code(), tonic::Code::ResourceExhausted);
    assert_eq!(rig.agent_rows().await, 0);

    // sit2 has its own budget, and the address has used 4 of its 5.
    assert!(
        rig.join
            .join_agent(Some(ip), google_params("sit2", "gid-right", &csr))
            .await
            .is_ok()
    );
    let error = rig
        .join
        .join_agent(Some(ip), google_params("sit2", "gid-right", &csr))
        .await
        .unwrap_err();
    assert_eq!(error, JoinError::RateLimited, "the address budget is used up");
}

#[tokio::test]
async fn join_with_a_sentinel_subject_is_refused_on_the_agent_service() {
    let rig = rig!();
    rig.accept_google("gid-x", SERVICE_ACCOUNT);
    let (_key, csr) = fresh_csr();
    let error = rig
        .join
        .join_agent(
            None,
            JoinParams {
                subject: JoinSubject::Sentinel(ShortText::parse("nfs-vm-1").unwrap()),
                csr_der: Bytes::from(csr),
                credential: JoinCredential::GoogleIdToken(Secret::new("gid-x".to_owned())),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(refused(error), Some(Refusal::WrongKind));
}

// ------------------------------------------------------------------------------------------------ renewal

#[tokio::test]
async fn renewal_at_half_lifetime_issues_a_new_certificate_for_the_same_swimlane_and_audits_it() {
    let rig = rig!();
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.accept_google("gid-renew", SERVICE_ACCOUNT);
    let (_key, csr) = fresh_csr();
    let first = rig
        .join
        .join_agent(None, google_params("sit1", "gid-renew", &csr))
        .await
        .unwrap();
    let first_serial: String =
        sqlx::query_scalar("SELECT cert_serial FROM agents WHERE swimlane_id = 'sit1'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();

    // Twelve hours on, the agent renews over its authenticated stream with a new key.
    rig.ca.clock.advance_secs(12 * 3600);
    let (new_key, new_csr) = fresh_csr();
    let renewed = rig
        .join
        .renew_agent(&swimlane("sit1"), &Bytes::from(new_csr))
        .await
        .unwrap();

    assert_eq!(
        san_uris(&renewed.cert_chain_der[0]),
        ["spiffe://lanekeeper/swimlane/sit1"]
    );
    assert_eq!(
        renewed.not_after.unix_millis() - first.not_after.unix_millis(),
        12 * 3600 * 1000,
        "a fresh 24 hours from the renewal"
    );
    let (_, leaf) = X509Certificate::from_der(&renewed.cert_chain_der[0]).unwrap();
    assert_eq!(
        leaf.public_key().subject_public_key.data.as_ref(),
        rcgen::PublicKeyData::der_bytes(&new_key),
        "for the new key"
    );
    let row = sqlx::query("SELECT cert_serial FROM agents WHERE swimlane_id = 'sit1'")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_ne!(row.get::<String, _>("cert_serial"), first_serial);
    assert_eq!(rig.audit_rows("agent_certificate_renewed").await, 1);
    assert_eq!(rig.audit_rows("agent_joined").await, 1, "a renewal is not a join");
}

#[tokio::test]
async fn a_failing_audit_write_means_no_renewed_certificate() {
    let rig = rig!();
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    let (_key, csr) = fresh_csr();
    rig.audit.set_failing(true);
    let error = rig
        .join
        .renew_agent(&swimlane("sit1"), &Bytes::from(csr))
        .await
        .unwrap_err();
    assert_eq!(error, JoinError::Unavailable);
    assert_eq!(rig.agent_rows().await, 0);
}

// ---------------------------------------------------------------------------------------------- S10

#[tokio::test]
async fn join_logs_and_errors_never_contain_the_credential() {
    let rig = rig!();
    rig.db
        .seed_swimlane(&rig.pool, "sit1", Some(SERVICE_ACCOUNT))
        .await;
    rig.accept_google("gid-SECRET-PLANTED-1", SERVICE_ACCOUNT);
    rig.db
        .seed_join_token(&rig.pool, "sit1", "jt-SECRET-PLANTED-2", 60, 3600)
        .await;
    let (log, guard) = LogCapture::install();
    let (_key, csr) = fresh_csr();
    let mut texts = Vec::new();
    // Failures and successes of both paths.
    for params in [
        google_params("sit1", "gid-SECRET-PLANTED-WRONG", &csr),
        token_params("sit1", "jt-SECRET-PLANTED-WRONG", &csr),
        token_params("sit1", "jt-SECRET-PLANTED-2", &csr),
        google_params("sit1", "gid-SECRET-PLANTED-1", &csr),
    ] {
        match rig.join.join_agent(None, params).await {
            Ok(issued) => texts.push(format!("{issued:?}")),
            Err(error) => texts.push(format!("{error} {error:?}")),
        }
    }
    drop(guard);
    let all = format!("{}\n{}", log.text(), texts.join("\n"));
    assert!(
        !all.contains("SECRET-PLANTED"),
        "a credential reached a log or an error:\n{all}"
    );
    let hash = hex::encode(hub_platform::join::token::hash_token(&Secret::new(
        "jt-SECRET-PLANTED-2".to_owned(),
    )));
    assert!(!all.contains(&hash), "not even its hash");
    assert!(
        !log.is_empty(),
        "the join said something (refusal codes), or this test proves nothing"
    );
}
