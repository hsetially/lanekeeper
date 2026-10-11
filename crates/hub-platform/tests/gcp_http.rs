//! T2: the Google HTTP client (S6, S18).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;

use hub_platform::gcp::http::{
    GoogleHttp, GoogleRequest, HostPolicy, HttpError, HttpLimits, Origin, tls_config,
};
use support::fake_google::{FakeGoogle, Fault, Route};
use support::tls::TestPki;

fn plain_client(fake: &FakeGoogle, policy: HostPolicy) -> GoogleHttp {
    GoogleHttp::new(
        policy,
        tls_config(rustls::RootCertStore::empty()).unwrap(),
        fake.dialer(),
        HttpLimits::default(),
    )
}

#[tokio::test(start_paused = true)]
async fn google_http_refuses_hosts_outside_the_allowlist() {
    let fake = FakeGoogle::new();
    let client = plain_client(&fake, HostPolicy::google());
    for url in [
        "https://evil.example",
        "https://googleapis.com.evil.example",
        "https://googleapis.com",
        "http://secretmanager.googleapis.com",
        "https://secretmanager.googleapis.com:8443",
        "http://127.0.0.1",
        "http://localhost:8080",
        "http://metadata.google.internal:8080",
        "https://metadata.google.internal",
        "https://169.254.169.254",
    ] {
        let origin = Origin::parse(url).unwrap();
        let error = client
            .exchange(&origin, GoogleRequest::get("/"))
            .await
            .unwrap_err();
        assert_eq!(error, HttpError::HostNotAllowed, "{url}");
    }
    assert!(
        fake.dials().is_empty(),
        "a refused destination never opens a connection, whatever the dialer"
    );
}

#[tokio::test(start_paused = true)]
async fn google_http_reaches_the_metadata_server_over_plain_http_only_there() {
    let fake = FakeGoogle::new();
    let client = plain_client(&fake, HostPolicy::google());
    let origin = Origin::parse("http://metadata.google.internal").unwrap();
    let request = GoogleRequest::get("/computeMetadata/v1/instance/service-accounts/default/token")
        .with_header("metadata-flavor", "Google")
        .unwrap();
    let response = client.exchange(&origin, request).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(fake.dials(), [("metadata.google.internal".to_owned(), 80)]);
    assert_eq!(
        fake.requests()[0].host.as_deref(),
        Some("metadata.google.internal")
    );
}

fn tls_client(
    fake: &FakeGoogle,
    server_pki: &TestPki,
    client_pki: &TestPki,
    host: &str,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> (GoogleHttp, Origin) {
    let server = server_pki.server_config(host, versions);
    let client = GoogleHttp::new(
        HostPolicy::google(),
        tls_config(client_pki.roots()).unwrap(),
        fake.tls_dialer(server),
        HttpLimits::default(),
    );
    (client, Origin::parse(&format!("https://{host}")).unwrap())
}

#[tokio::test(start_paused = true)]
async fn google_http_speaks_tls13_to_a_server_with_a_verified_certificate() {
    let fake = FakeGoogle::new();
    let pki = TestPki::new("test root");
    let (client, origin) = tls_client(
        &fake,
        &pki,
        &pki,
        "cloudkms.googleapis.com",
        &[&rustls::version::TLS13],
    );
    let request = GoogleRequest::get("/v1/anything")
        .with_header("x-test", "1")
        .unwrap();
    let response = client.exchange(&origin, request).await.unwrap();
    assert_eq!(
        response.status, 404,
        "the fake's answer for an unknown route, through TLS"
    );
    assert_eq!(fake.dials(), [("cloudkms.googleapis.com".to_owned(), 443)]);
    assert_eq!(
        fake.requests()[0].host.as_deref(),
        Some("cloudkms.googleapis.com"),
        "default port is left out of Host"
    );
}

#[tokio::test(start_paused = true)]
async fn google_http_refuses_tls12_wrong_roots_and_a_certificate_for_another_host() {
    let host = "cloudkms.googleapis.com";
    let pki = TestPki::new("test root");

    // A server that offers TLS 1.2 only.
    let fake = FakeGoogle::new();
    let (client, origin) = tls_client(&fake, &pki, &pki, host, &[&rustls::version::TLS12]);
    assert_eq!(
        client
            .exchange(&origin, GoogleRequest::get("/"))
            .await
            .unwrap_err(),
        HttpError::Tls,
        "TLS 1.2 is refused (S6)"
    );
    assert!(fake.requests().is_empty());

    // A certificate from a CA the client does not trust.
    let fake = FakeGoogle::new();
    let other = TestPki::new("someone else");
    let (client, origin) = tls_client(&fake, &other, &pki, host, &[&rustls::version::TLS13]);
    assert_eq!(
        client
            .exchange(&origin, GoogleRequest::get("/"))
            .await
            .unwrap_err(),
        HttpError::Tls
    );
    assert!(
        fake.requests().is_empty(),
        "no request is sent over a connection that failed verification"
    );

    // A certificate from the trusted CA, but for another name.
    let fake = FakeGoogle::new();
    let server = pki.server_config("secretmanager.googleapis.com", &[&rustls::version::TLS13]);
    let client = GoogleHttp::new(
        HostPolicy::google(),
        tls_config(pki.roots()).unwrap(),
        fake.tls_dialer(server),
        HttpLimits::default(),
    );
    let origin = Origin::parse(&format!("https://{host}")).unwrap();
    assert_eq!(
        client
            .exchange(&origin, GoogleRequest::get("/"))
            .await
            .unwrap_err(),
        HttpError::Tls
    );
}

#[tokio::test(start_paused = true)]
async fn google_http_caps_the_body_and_does_not_follow_redirects() {
    let fake = FakeGoogle::new();
    let limits = HttpLimits {
        max_body: 1024,
        ..HttpLimits::default()
    };
    let policy = HostPolicy::only_for_tests(&["api.test"]);
    let client = GoogleHttp::new(
        policy,
        tls_config(rustls::RootCertStore::empty()).unwrap(),
        fake.dialer(),
        limits,
    );
    let origin = Origin::parse("http://api.test").unwrap();
    let token_path = "/computeMetadata/v1/instance/service-accounts/default/token";
    let get = || {
        GoogleRequest::get(token_path)
            .with_header("metadata-flavor", "Google")
            .unwrap()
    };

    fake.script(Route::Token, Fault::Oversize(1025), 1);
    assert_eq!(
        client.exchange(&origin, get()).await.unwrap_err(),
        HttpError::TooLarge
    );
    fake.script(Route::Token, Fault::Oversize(1024), 1);
    assert_eq!(
        client.exchange(&origin, get()).await.unwrap().body.len(),
        1024,
        "exactly the cap is fine"
    );

    fake.script(Route::Token, Fault::Status(302), 1);
    let response = client.exchange(&origin, get()).await.unwrap();
    assert_eq!(response.status, 302);
    assert_eq!(fake.count(Route::Token), 3, "the redirect was not followed");
}

#[tokio::test(start_paused = true)]
async fn google_http_gives_up_on_a_server_that_never_answers() {
    let fake = FakeGoogle::new();
    let client = plain_client(&fake, HostPolicy::only_for_tests(&["api.test"]));
    let origin = Origin::parse("http://api.test").unwrap();
    fake.script(Route::Token, Fault::Hang, 1);
    let request = GoogleRequest::get("/computeMetadata/v1/instance/service-accounts/default/token");
    let started = tokio::time::Instant::now();
    assert_eq!(
        client.exchange(&origin, request).await.unwrap_err(),
        HttpError::Timeout
    );
    assert_eq!(started.elapsed(), HttpLimits::default().attempt_timeout);
    let _ = Arc::new(());
}
