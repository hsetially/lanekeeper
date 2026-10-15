//! T5 (S6, S8, S18, rule 5): the GitHub client against a local server. What it sends (the token in one header and nowhere else), what
//! it accepts (TLS 1.3 only, a body up to a cap, the answers the API documents), where it goes (never along a redirect) and how
//! it fails (a fixed word, bounded retries, a timeout).

use std::time::Duration;

use domain::Secret;
use hub_writes::github::{GithubApi, GithubClient, GithubClientConfig, GithubError, RepoRef};
use rustls::RootCertStore;

use crate::support::fake_github::{LocalGithub, Reply, Tls, always, sequence};

const TOKEN: &str = "github_pat_11TESTONLYTOKEN0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHIJKLMNOP";

fn token() -> Secret<String> {
    Secret::new(TOKEN.to_owned())
}

fn repo() -> RepoRef {
    RepoRef::parse("acme/configuration-base-saas").unwrap()
}

/// Fast settings: the same bounds as in service, with waits short enough for a test.
fn fast() -> GithubClientConfig {
    GithubClientConfig {
        attempt_timeout: Duration::from_millis(500),
        total_timeout: Duration::from_secs(5),
        max_attempts: 3,
        backoff_base: Duration::from_millis(2),
        backoff_cap: Duration::from_millis(10),
        max_rate_wait: Duration::from_millis(50),
        ..GithubClientConfig::default()
    }
}

fn client(server: &LocalGithub) -> GithubClient {
    GithubClient::for_tests(&server.base(), fast()).unwrap()
}

const USER: &str = r#"{"login":"octocat","id":1}"#;

#[tokio::test]
async fn user_reads_the_login_and_the_expiry() {
    let server = LocalGithub::start(always(Reply::json(200, USER).with_header(
        "github-authentication-token-expiration",
        "2026-11-20 12:00:00 UTC",
    )))
    .await;
    let me = client(&server).user(&token()).await.unwrap();
    assert_eq!(me.login.as_str(), "octocat");
    assert_eq!(
        me.expires_at.map(domain::Timestamp::unix_millis),
        Some(1_795_176_000_000)
    );

    let seen = server.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("GET", "/user"));
    assert_eq!(seen[0].header("accept"), Some("application/vnd.github+json"));
    assert_eq!(seen[0].header("x-github-api-version"), Some("2022-11-28"));
    assert_eq!(seen[0].header("user-agent"), Some("lanekeeper-hub"));
}

#[tokio::test]
async fn a_token_without_an_expiry_has_none() {
    let server = LocalGithub::start(always(Reply::json(200, USER))).await;
    assert_eq!(client(&server).user(&token()).await.unwrap().expires_at, None);
}

/// A header that is there and cannot be read is not "no expiry": taking it for one would leave a token that does expire without
/// the guard that stops a job from using it.
#[tokio::test]
async fn an_expiry_header_that_cannot_be_read_is_an_unexpected_answer() {
    for bad in ["soon", "2026-11-20 12:00:00 -0500", "2026-02-30 12:00:00 UTC", ""] {
        let server = LocalGithub::start(always(
            Reply::json(200, USER).with_header("github-authentication-token-expiration", bad),
        ))
        .await;
        assert_eq!(
            client(&server).user(&token()).await.unwrap_err(),
            GithubError::Unexpected,
            "{bad:?}"
        );
    }
}

/// S8, S21: the token travels in the `Authorization` header and in nothing else the server can see.
#[tokio::test]
async fn the_token_is_sent_in_the_authorization_header_only() {
    let server = LocalGithub::start(always(Reply::json(200, USER))).await;
    let c = client(&server);
    c.user(&token()).await.unwrap();
    c.can_push(&token(), &repo()).await.unwrap();
    let seen = server.seen();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        let elsewhere = request
            .everything()
            .replace(&format!("authorization: Bearer {TOKEN}"), "");
        assert!(
            !elsewhere.contains(TOKEN),
            "the token is in the request line or another header"
        );
        assert!(request.path.starts_with('/') && !request.path.contains('?'));
    }
    assert_eq!(seen[1].path, "/repos/acme/configuration-base-saas");
}

#[tokio::test]
async fn push_permission_is_read_and_fails_closed() {
    for (body, expect) in [
        (r#"{"permissions":{"push":true}}"#, Ok(true)),
        (r#"{"permissions":{"push":false}}"#, Ok(false)),
        (r#"{"permissions":{}}"#, Ok(false)),
        (r#"{"name":"x"}"#, Ok(false)),
        (r#"{"permissions":{"push":"yes"}}"#, Err(GithubError::Unexpected)),
        ("not json", Err(GithubError::Unexpected)),
    ] {
        let server = LocalGithub::start(always(Reply::json(200, body))).await;
        assert_eq!(
            client(&server).can_push(&token(), &repo()).await,
            expect,
            "{body}"
        );
    }
}

/// A fine-grained token that was not given the repository sees 404 (or 403): the answer is "cannot push", not an error.
#[tokio::test]
async fn a_repository_the_token_cannot_see_is_one_it_cannot_push_to() {
    for status in [404, 403] {
        let server = LocalGithub::start(always(Reply::json(status, r#"{"message":"Not Found"}"#))).await;
        assert_eq!(
            client(&server).can_push(&token(), &repo()).await,
            Ok(false),
            "{status}"
        );
        assert_eq!(server.requests(), 1, "{status} is not retried");
    }
}

#[tokio::test]
async fn statuses_become_fixed_errors() {
    for (status, expect) in [
        (401, GithubError::Unauthorized),
        (403, GithubError::Forbidden),
        (404, GithubError::NotFound),
        (204, GithubError::Unexpected),
        (418, GithubError::Unexpected),
    ] {
        let server = LocalGithub::start(always(Reply::json(status, r#"{"message":"nope"}"#))).await;
        assert_eq!(
            client(&server).user(&token()).await.unwrap_err(),
            expect,
            "{status}"
        );
        assert_eq!(server.requests(), 1, "{status} is not retried");
    }
}

#[tokio::test]
async fn a_server_error_is_retried_a_bounded_number_of_times() {
    let server = LocalGithub::start(sequence(vec![
        Reply::json(502, "{}"),
        Reply::json(503, "{}"),
        Reply::json(200, USER),
    ]))
    .await;
    assert_eq!(
        client(&server).user(&token()).await.unwrap().login.as_str(),
        "octocat"
    );
    assert_eq!(server.requests(), 3);

    let down = LocalGithub::start(always(Reply::json(500, "{}"))).await;
    assert_eq!(
        client(&down).user(&token()).await.unwrap_err(),
        GithubError::Unavailable
    );
    assert_eq!(down.requests(), 3, "exactly max_attempts");
}

#[tokio::test]
async fn a_short_rate_limit_is_waited_for_and_a_long_one_is_an_error() {
    let limited = Reply::json(403, "{}")
        .with_header("x-ratelimit-remaining", "0")
        .with_header("retry-after", "0");
    let server = LocalGithub::start(sequence(vec![limited, Reply::json(200, USER)])).await;
    assert!(client(&server).user(&token()).await.is_ok());
    assert_eq!(server.requests(), 2);

    let long = Reply::json(429, "{}").with_header("retry-after", "120");
    let server = LocalGithub::start(always(long)).await;
    assert_eq!(
        client(&server).user(&token()).await.unwrap_err(),
        GithubError::RateLimited
    );
    assert_eq!(
        server.requests(),
        1,
        "a wait longer than the call allows is not slept"
    );

    // Rate limited and no hint when to come back: not retried either.
    let server = LocalGithub::start(always(Reply::json(429, "{}"))).await;
    assert_eq!(
        client(&server).user(&token()).await.unwrap_err(),
        GithubError::RateLimited
    );
    assert_eq!(server.requests(), 1);
}

#[tokio::test]
async fn a_slow_answer_times_out_and_the_whole_call_is_bounded() {
    let slow = Reply::json(200, USER).after(Duration::from_millis(400));
    let server = LocalGithub::start(always(slow)).await;
    let cfg = GithubClientConfig {
        attempt_timeout: Duration::from_millis(40),
        total_timeout: Duration::from_millis(500),
        ..fast()
    };
    let c = GithubClient::for_tests(&server.base(), cfg).unwrap();
    let began = tokio::time::Instant::now();
    assert_eq!(c.user(&token()).await.unwrap_err(), GithubError::Unavailable);
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "the call outlived its total timeout: {:?}",
        began.elapsed()
    );
}

#[tokio::test]
async fn a_nothing_listening_is_unavailable() {
    // A port that was open and is closed.
    let gone = {
        let server = LocalGithub::start(always(Reply::json(200, USER))).await;
        server.base()
    };
    let c = GithubClient::for_tests(&gone, fast()).unwrap();
    assert_eq!(c.user(&token()).await.unwrap_err(), GithubError::Unavailable);
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused() {
    let big = format!(r#"{{"login":"octocat","pad":"{}"}}"#, "a".repeat(1024 * 1024));
    let server = LocalGithub::start(always(Reply::json(200, &big))).await;
    assert_eq!(
        client(&server).user(&token()).await.unwrap_err(),
        GithubError::Unexpected
    );
}

/// S18, S8: a redirect is not followed, so the token cannot be sent to a host the allowlist never named.
#[tokio::test]
async fn a_redirect_is_not_followed_and_the_token_goes_nowhere_else() {
    let elsewhere = LocalGithub::start(always(Reply::json(200, USER))).await;
    let location = format!("{}/user", elsewhere.base());
    let server = LocalGithub::start(always(Reply::json(302, "").with_header("location", &location))).await;
    assert_eq!(
        client(&server).user(&token()).await.unwrap_err(),
        GithubError::Unexpected
    );
    assert_eq!(server.requests(), 1);
    assert_eq!(elsewhere.requests(), 0, "the client followed a redirect");
}

#[tokio::test]
async fn an_error_never_holds_the_token_or_the_servers_words() {
    let marker = "server-words-marker-5d1f";
    let cases = [
        Reply::json(401, &format!(r#"{{"message":"{marker}"}}"#)),
        Reply::json(500, &format!(r#"{{"message":"{marker}"}}"#)),
        Reply::json(200, &format!("{marker} not json")),
        Reply::json(302, "").with_header("location", &format!("http://127.0.0.1:1/{marker}")),
    ];
    for reply in cases {
        let server = LocalGithub::start(always(reply)).await;
        let e = client(&server).user(&token()).await.unwrap_err();
        let shown = format!("{e} {e:?}");
        assert!(!shown.contains(marker) && !shown.contains("github_pat_") && !shown.contains("127.0.0.1"));
    }
}

// ------------------------------------------------------------------------------------------------------------------ TLS

fn roots_of(server: &LocalGithub) -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.add(server.cert.clone().unwrap()).unwrap();
    roots
}

/// S6: TLS 1.3 works against a server that speaks it, whether or not it also offers 1.2.
#[tokio::test]
async fn tls_13_is_spoken() {
    for tls in [Tls::V13, Tls::Both] {
        let server = LocalGithub::start_with(always(Reply::json(200, USER)), tls).await;
        let c = GithubClient::for_tests_with_roots(&server.base(), roots_of(&server), fast()).unwrap();
        assert_eq!(
            c.user(&token()).await.unwrap().login.as_str(),
            "octocat",
            "{tls:?}"
        );
    }
}

/// S6: a server that offers TLS 1.2 and nothing newer is refused, and no request, so no token, reaches it.
#[tokio::test]
async fn a_tls_12_only_server_is_refused() {
    let server = LocalGithub::start_with(always(Reply::json(200, USER)), Tls::V12).await;
    let c = GithubClient::for_tests_with_roots(&server.base(), roots_of(&server), fast()).unwrap();
    assert_eq!(c.user(&token()).await.unwrap_err(), GithubError::Unavailable);
    assert_eq!(server.requests(), 0, "a request was sent over TLS 1.2");
}

/// A certificate the roots do not vouch for is refused, and no request reaches the server.
#[tokio::test]
async fn an_untrusted_certificate_is_refused() {
    let server = LocalGithub::start_with(always(Reply::json(200, USER)), Tls::V13).await;
    let other = LocalGithub::start_with(always(Reply::json(200, USER)), Tls::V13).await;
    let c = GithubClient::for_tests_with_roots(&server.base(), roots_of(&other), fast()).unwrap();
    assert_eq!(c.user(&token()).await.unwrap_err(), GithubError::Unavailable);
    assert_eq!(server.requests(), 0, "a request was sent to an untrusted server");
}

/// The production constructor takes HTTPS and an allowed host only (S18); the local-server constructors exist only for tests.
#[tokio::test]
async fn the_production_client_refuses_other_addresses() {
    for base in [
        "http://127.0.0.1:1",
        "https://127.0.0.1",
        "https://github.com",
        "https://api.github.com.evil.example",
    ] {
        let cfg = GithubClientConfig {
            api_base: base.to_owned(),
            ..GithubClientConfig::default()
        };
        assert_eq!(GithubClient::new(cfg).unwrap_err(), GithubError::Config, "{base}");
    }
}
