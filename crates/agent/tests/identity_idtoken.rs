//! The Google ID token from the metadata server (Workload Identity, S5).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use agent::config::BaseUrl;
use agent::http::HttpClient;
use agent::identity::IdTokenError;
use agent::identity::idtoken::{IdTokenSource, MetadataIdTokens};
use domain::ShortText;
use support::fake_metadata::FakeMetadata;
use support::raw_server::{RawServer, Reply};

const TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.eyJhdWQiOiJodHRwczovL2h1YiJ9.c2lnbmF0dXJlLWJ5dGVz";

fn audience() -> ShortText {
    ShortText::parse("https://hub.example.com").unwrap()
}

fn source(url: &str) -> MetadataIdTokens {
    MetadataIdTokens::with_client(
        HttpClient::new(Duration::from_secs(5), 16 * 1024),
        BaseUrl::parse(url, "http").unwrap(),
    )
}

async fn fetch(reply: Reply) -> Result<String, IdTokenError> {
    let server = RawServer::start(move |_| reply.clone()).await;
    source(&server.url())
        .id_token(&audience())
        .await
        .map(|t| t.expose().clone())
}

#[tokio::test]
async fn asks_the_metadata_server_for_the_audience_with_the_flavor_header() {
    // The fake answers `403` to a request without the flavor header, so a success proves the header was sent.
    let server = FakeMetadata::serving(TOKEN).await;
    source(server.url()).id_token(&audience()).await.unwrap();
    let seen = server.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(
        seen[0].target,
        "/computeMetadata/v1/instance/service-accounts/default/identity?audience=https%3A%2F%2Fhub.example.com"
    );
    assert_eq!(seen[0].header("metadata-flavor"), Some("Google"));
}

#[tokio::test]
async fn the_audience_cannot_inject_query_parameters() {
    let server = RawServer::start(|_| Reply::ok(TOKEN)).await;
    let hostile = ShortText::parse("a&format=full#x y?z=1%").unwrap();
    source(&server.url()).id_token(&hostile).await.unwrap();
    assert_eq!(
        server.requests()[0].target,
        "/computeMetadata/v1/instance/service-accounts/default/identity?audience=a%26format%3Dfull%23x%20y%3Fz%3D1%25"
    );
}

#[tokio::test]
async fn returns_the_token_without_the_trailing_newline() {
    assert_eq!(fetch(Reply::ok(format!("{TOKEN}\n"))).await.unwrap(), TOKEN);
}

#[tokio::test]
async fn nothing_listening_means_workload_identity_is_unavailable() {
    let server = FakeMetadata::absent().await;
    assert_eq!(
        source(server.url()).id_token(&audience()).await.unwrap_err(),
        IdTokenError::Unavailable
    );
}

#[tokio::test]
async fn no_answer_in_time_means_workload_identity_is_unavailable() {
    let server = FakeMetadata::hanging().await;
    let impatient = MetadataIdTokens::with_client(
        HttpClient::new(Duration::from_millis(200), 16 * 1024),
        BaseUrl::parse(server.url(), "http").unwrap(),
    );
    assert_eq!(
        impatient.id_token(&audience()).await.unwrap_err(),
        IdTokenError::Unavailable
    );
}

#[tokio::test]
async fn an_error_status_is_a_refusal_and_not_unavailability() {
    for status in [400, 403, 404, 500, 503] {
        assert_eq!(
            fetch(Reply::status(status, "no")).await.unwrap_err(),
            IdTokenError::Refused { status },
            "{status}"
        );
    }
}

#[tokio::test]
async fn anything_that_is_not_a_token_is_malformed() {
    let long = format!("{TOKEN}{}", "a".repeat(9000));
    let bodies: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("html", "<html>login</html>".to_owned()),
        ("two segments", "abc.def".to_owned()),
        ("four segments", "a.b.c.d".to_owned()),
        ("empty segment", "a..c".to_owned()),
        ("space inside", "abc.def ghi.jkl".to_owned()),
        ("non-ascii", "abc.dé.jkl".to_owned()),
        ("too long", long),
        ("json", "{\"token\":\"a.b.c\"}".to_owned()),
    ];
    for (what, body) in bodies {
        assert_eq!(
            fetch(Reply::ok(body)).await.unwrap_err(),
            IdTokenError::Malformed,
            "{what}"
        );
    }
}

#[tokio::test]
async fn a_body_over_the_cap_is_malformed_not_buffered() {
    let server = RawServer::start(|_| Reply::ok(vec![b'a'; 64 * 1024])).await;
    assert_eq!(
        source(&server.url()).id_token(&audience()).await.unwrap_err(),
        IdTokenError::Malformed
    );
}

#[tokio::test]
async fn the_token_is_a_secret() {
    let server = RawServer::start(|_| Reply::ok(TOKEN)).await;
    let token = source(&server.url()).id_token(&audience()).await.unwrap();
    assert_eq!(format!("{token:?}"), "[redacted]");
    assert_eq!(token.expose(), TOKEN);
}
