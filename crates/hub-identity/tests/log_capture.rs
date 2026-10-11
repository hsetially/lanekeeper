//! T1 (S10): nothing sensitive reaches the logs while a user signs in, fails to sign in, or the identity provider misbehaves.
//! Later tasks add their flows (sessions, admin operations, the audit chain) to the same assertion.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use serde_json::json;
use support::log_capture;
use support::login_rig::{begin, query, rig, sign_in_with};
use support::mock_oidc::{CLIENT_SECRET, TokenSpec};

#[tokio::test]
async fn log_capture_has_no_token_cookie_secret_or_diff() {
    let (capture, _guard) = log_capture::install();
    let rig = rig().await;

    // A successful sign-in, keeping the pieces that must not be logged.
    let begun = begin(&rig, Some("/swimlanes")).await;
    let id_token = TokenSpec::valid(begun.param("nonce")).mint();
    rig.mock.set_id_token(id_token.clone());
    rig.login
        .complete(
            &query("AUTH-CODE-12345", begun.param("state")),
            Some(&begun.cookie),
        )
        .await
        .unwrap();
    let verifier = rig.mock.token_forms()[0]["code_verifier"].clone();

    // Failures: a bad token, a refused code, an unreachable provider, a bad key set.
    let _ = sign_in_with(&rig, |n| {
        TokenSpec::valid(n).claim("tid", json!("99999999-9999-4999-8999-999999999999"))
    })
    .await;
    rig.mock.set_token_status(400);
    let _ = sign_in_with(&rig, TokenSpec::valid).await;
    rig.mock.set_token_status(200);
    *rig.mock.token_transport_error.lock().unwrap() = true;
    let _ = sign_in_with(&rig, TokenSpec::valid).await;
    *rig.mock.token_transport_error.lock().unwrap() = false;
    rig.mock.set_jwks_status(500);

    let log = capture.text();
    assert!(
        log.contains("token endpoint refused"),
        "the refusal is logged by status, so the capture works: {log:?}"
    );
    for secret in [
        id_token.as_str(),
        "AUTH-CODE-12345",
        CLIENT_SECRET,
        begun.cookie.as_str(),
        verifier.as_str(),
        "ACCESS-TOKEN-DO-NOT-KEEP",
        begun.param("nonce"),
        begun.param("state"),
        "AADSTS",
    ] {
        assert!(!log.contains(secret), "{secret} reached the log");
    }
}

/// T5: the audit path logs positions, hashes and reasons, never the diff the events carry, and never a token (S9, S10).
#[tokio::test]
async fn log_capture_has_no_diff_or_token_through_the_audit_jobs() {
    use hub_identity::audit::PgAuditLog;
    use hub_identity::audit::shipper::AuditLogShipper;
    use ports::{AuditLog, Tx};
    use support::audit_rig::{AuditRig, event};

    let (capture, _guard) = log_capture::install();
    let rig = AuditRig::new(db!()).await;
    let secret = "password: SECRET-DIFF-IN-EVERY-EVENT";
    for n in 0..4 {
        let mut e = event(n);
        e.diff = Some(format!("+{secret}\n"));
        let mut tx = rig.pool.begin().await.unwrap();
        PgAuditLog::new()
            .record(&mut Tx::from_pg(&mut tx), e)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    rig.checkpoint_job().run_once().await.unwrap();
    AuditLogShipper::new(rig.pool.clone(), rig.leases.clone(), rig.metrics.clone())
        .tick()
        .await
        .unwrap();
    // A clean night, then a tampered one.
    rig.nightly().run_once().await.unwrap();
    rig.tamper_open(true).await;
    // (The statement text itself is logged by sqlx at debug level in this capture, so it must not hold the marker.)
    rig.admin_sql("UPDATE audit_events SET event_json = event_json || ' ' WHERE seq = 2")
        .await;
    let _ = rig.chain_verifier().verify().await.unwrap();
    // A job that cannot sign.
    let failing = rig.checkpoint_job_with(
        std::sync::Arc::new(ports::fakes::FakeKmsSigner::new(&[])),
        rig.bucket.clone(),
    );
    rig.record(1).await;
    let _ = failing.run_once().await;

    let log = capture.text();
    assert!(
        log.contains("audit event"),
        "the shipper wrote its lines, so the capture works: {log}"
    );
    assert!(!log.contains("SECRET-DIFF"), "event content reached the log");
}
