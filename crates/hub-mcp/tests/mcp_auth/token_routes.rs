//! S3, S7, S9, S11: the REST routes for personal tokens. They ride the hub's session guard (a signed-in user, Viewer), answer 404
//! unless personal-token mode is on, and never return a token value except in the response that creates it.

use axum::body::Body;
use axum::http::{Method, Response, StatusCode, header};
use domain::{Role, UserStatus};
use serde_json::{Value, json};

use crate::rig::{ORIGIN, Rig};
use crate::support::json_body;

const COLLECTION: &str = "/api/v1/me/mcp-tokens";

fn rig() -> Rig {
    let rig = Rig::personal();
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    rig.add_user("bob", Some(Role::Viewer), UserStatus::Active);
    rig
}

async fn create(rig: &Rig, who: &str, label: &str) -> Response<Body> {
    rig.rest(
        Method::POST,
        COLLECTION,
        Some(who),
        Some(ORIGIN),
        Some(&json!({ "label": label }).to_string()),
    )
    .await
}

async fn code(response: Response<Body>) -> String {
    json_body(response).await["code"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn the_routes_are_not_found_unless_personal_token_mode_is_on() {
    let rig = Rig::oauth();
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    for (method, path, body) in [
        (Method::GET, COLLECTION, None),
        (Method::POST, COLLECTION, Some(r#"{"label":"x"}"#)),
        (Method::DELETE, "/api/v1/me/mcp-tokens/1", None),
    ] {
        let response = rig
            .rest(method.clone(), path, Some("alice"), Some(ORIGIN), body)
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(code(response).await, "not_found");
    }
}

#[tokio::test]
async fn the_routes_need_a_signed_in_user_who_may_use_the_hub() {
    let rig = rig();
    rig.add_user("pat", Some(Role::Viewer), UserStatus::Pending);
    rig.add_user("dave", Some(Role::Admin), UserStatus::Disabled);
    assert_eq!(
        rig.rest(Method::GET, COLLECTION, None, None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for who in ["pat", "dave"] {
        assert_eq!(
            rig.rest(Method::GET, COLLECTION, Some(who), None, None)
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{who}"
        );
        assert_eq!(
            create(&rig, who, "x").await.status(),
            StatusCode::FORBIDDEN,
            "{who}"
        );
    }
    assert!(rig.store.as_ref().unwrap().rows().is_empty());
    // A personal token is not a session: it cannot make or revoke personal tokens.
    let token = create(&rig, "alice", "laptop").await;
    let token = json_body(token).await["token"].as_str().unwrap().to_owned();
    let request = axum::http::Request::builder()
        .method(Method::GET)
        .uri(COLLECTION)
        .header(header::HOST, crate::rig::HOST)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = tower::ServiceExt::oneshot(rig.router.clone(), request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn creating_a_token_shows_it_once() {
    let rig = rig();
    let response = create(&rig, "alice", "laptop").await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = json_body(response).await;
    let token = body["token"].as_str().unwrap();
    assert!(token.starts_with("lkpat_") && token.len() == 49);
    assert_eq!(body["label"], "laptop");
    assert_eq!(body["revoked_at"], Value::Null);
    assert_eq!(body["last_used_at"], Value::Null);
    let lifetime = body["expires_at"].as_i64().unwrap() - body["created_at"].as_i64().unwrap();
    assert_eq!(lifetime, 7 * 24 * 60 * 60 * 1000);
    // The same fields as the OpenAPI schema, and no more.
    let mut keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "created_at",
            "expires_at",
            "id",
            "label",
            "last_used_at",
            "revoked_at",
            "token"
        ]
    );
    // It works as a bearer token on /mcp straight away.
    let mut client = rig.client(Some(token));
    client.initialize(&json!({})).await.unwrap();
    // And it is not in the list.
    let listed = json_body(rig.rest(Method::GET, COLLECTION, Some("alice"), None, None).await).await;
    assert!(!listed.to_string().contains(&token["lkpat_".len()..]));
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn the_list_is_the_callers_own_newest_first_with_keyset_paging() {
    let rig = rig();
    for n in 0..5 {
        assert_eq!(
            create(&rig, "alice", &format!("a{n}")).await.status(),
            StatusCode::CREATED
        );
    }
    assert_eq!(create(&rig, "bob", "b0").await.status(), StatusCode::CREATED);
    let page = |query: &str| {
        let rig = &rig;
        let path = format!("{COLLECTION}{query}");
        async move { json_body(rig.rest(Method::GET, &path, Some("alice"), None, None).await).await }
    };
    let first = page("?limit=2").await;
    let labels = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(labels(&first), ["a4", "a3"]);
    let cursor = first["next"].as_str().unwrap().to_owned();
    let second = page(&format!("?limit=2&cursor={cursor}")).await;
    assert_eq!(labels(&second), ["a2", "a1"]);
    let third = page(&format!("?limit=2&cursor={}", second["next"].as_str().unwrap())).await;
    assert_eq!(labels(&third), ["a0"]);
    assert_eq!(third["next"], Value::Null);
    // The whole list without a limit, and another user's tokens are not in it.
    assert_eq!(labels(&page("").await), ["a4", "a3", "a2", "a1", "a0"]);
}

#[tokio::test]
async fn list_queries_are_strict() {
    let rig = rig();
    for query in [
        "?limit=0",
        "?limit=501",
        "?limit=-1",
        "?limit=1.5",
        "?limit=",
        "?limit=0010",
        "?limit=1&limit=2",
        "?cursor=!!",
        "?cursor=",
        "?cursor=MA",
        "?cursor=LTE",
        "?cursor=MDA",
        "?other=1",
        "?limit",
        "?%6Cimit=1",
        &format!("?cursor={}", "A".repeat(513)),
    ] {
        let response = rig
            .rest(
                Method::GET,
                &format!("{COLLECTION}{query}"),
                Some("alice"),
                None,
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(code(response).await, "bad_request");
    }
    for query in ["?limit=1", "?limit=500", "", "?cursor=MQ"] {
        let response = rig
            .rest(
                Method::GET,
                &format!("{COLLECTION}{query}"),
                Some("alice"),
                None,
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{query}");
    }
}

#[tokio::test]
async fn bodies_are_checked_before_anything_is_written() {
    let rig = rig();
    let cases = [
        (r#"{"label":""}"#, StatusCode::BAD_REQUEST),
        (r#"{"label":"   "}"#, StatusCode::BAD_REQUEST),
        (r#"{"label":"tab\there"}"#, StatusCode::BAD_REQUEST),
        (r"{}", StatusCode::BAD_REQUEST),
        (r#"{"label":"x","extra":1}"#, StatusCode::BAD_REQUEST),
        (r#"{"label":7}"#, StatusCode::BAD_REQUEST),
        (r"[]", StatusCode::BAD_REQUEST),
        ("not json", StatusCode::BAD_REQUEST),
        ("", StatusCode::BAD_REQUEST),
    ];
    for (body, expected) in cases {
        let response = rig
            .rest(Method::POST, COLLECTION, Some("alice"), Some(ORIGIN), Some(body))
            .await;
        assert_eq!(response.status(), expected, "{body}");
    }
    let too_long = json!({ "label": "x".repeat(65) }).to_string();
    let response = rig
        .rest(
            Method::POST,
            COLLECTION,
            Some("alice"),
            Some(ORIGIN),
            Some(&too_long),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let huge = json!({ "label": "x".repeat(5000) }).to_string();
    let response = rig
        .rest(Method::POST, COLLECTION, Some("alice"), Some(ORIGIN), Some(&huge))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        rig.store.as_ref().unwrap().rows().is_empty(),
        "nothing was written"
    );
    assert!(rig.audit.entries().is_empty(), "and nothing was audited");
}

#[tokio::test]
async fn the_body_must_be_json() {
    let rig = rig();
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(COLLECTION)
        .header(header::HOST, crate::rig::HOST)
        .header(hub_platform::TEST_USER_HEADER, "alice")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("label=laptop"))
        .unwrap();
    let response = tower::ServiceExt::oneshot(rig.router.clone(), request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(rig.store.as_ref().unwrap().rows().is_empty());
}

#[tokio::test]
async fn state_changing_requests_need_the_public_origin() {
    let rig = rig();
    let body = json!({ "label": "x" }).to_string();
    for origin in [
        None,
        Some("https://evil.example"),
        Some("null"),
        Some("http://hub.example.test"),
        Some("https://hub.example.test:8443"),
    ] {
        let response = rig
            .rest(Method::POST, COLLECTION, Some("alice"), origin, Some(&body))
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin:?}");
        let response = rig
            .rest(
                Method::DELETE,
                "/api/v1/me/mcp-tokens/1",
                Some("alice"),
                origin,
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin:?}");
    }
    assert!(rig.store.as_ref().unwrap().rows().is_empty());
    assert!(rig.audit.entries().is_empty());
    // A read needs no origin.
    assert_eq!(
        rig.rest(Method::GET, COLLECTION, Some("alice"), None, None)
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_sixth_live_token_is_a_conflict() {
    let rig = rig();
    for n in 0..5 {
        assert_eq!(
            create(&rig, "alice", &format!("t{n}")).await.status(),
            StatusCode::CREATED
        );
    }
    let response = create(&rig, "alice", "sixth").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(code(response).await, "conflict");
    assert_eq!(rig.audit.entries().len(), 5);
}

#[tokio::test]
async fn revoking_is_idempotent_scoped_to_the_owner_and_audited_once() {
    let rig = rig();
    let created = json_body(create(&rig, "alice", "laptop").await).await;
    let token = created["token"].as_str().unwrap().to_owned();
    let id = created["id"].as_i64().unwrap();
    let path = format!("/api/v1/me/mcp-tokens/{id}");

    // Bob cannot revoke it, and cannot tell that it exists.
    let response = rig
        .rest(Method::DELETE, &path, Some("bob"), Some(ORIGIN), None)
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let mut alice = rig.client(Some(&token));
    alice.initialize(&json!({})).await.unwrap();

    let response = rig
        .rest(Method::DELETE, &path, Some("alice"), Some(ORIGIN), None)
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let again = rig
        .rest(Method::DELETE, &path, Some("alice"), Some(ORIGIN), None)
        .await;
    assert_eq!(again.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        rig.audit.entries().len(),
        2,
        "one event for the creation and one for the revocation"
    );

    // The token stops working at once, and the list still shows it, revoked.
    assert_eq!(
        rig.client(Some(&token))
            .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let listed = json_body(rig.rest(Method::GET, COLLECTION, Some("alice"), None, None).await).await;
    assert!(listed["items"][0]["revoked_at"].as_i64().is_some());
}

#[tokio::test]
async fn ids_that_are_not_ids_are_a_bad_request_and_missing_ones_are_not_found() {
    let rig = rig();
    for id in ["0", "-1", "01", "1e3", "abc", "99999999999999999999", "1%20"] {
        let response = rig
            .rest(
                Method::DELETE,
                &format!("/api/v1/me/mcp-tokens/{id}"),
                Some("alice"),
                Some(ORIGIN),
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{id}");
    }
    let response = rig
        .rest(
            Method::DELETE,
            "/api/v1/me/mcp-tokens/424242",
            Some("alice"),
            Some(ORIGIN),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_idempotency_key_is_accepted_and_does_not_change_the_answer() {
    // B17: the header is accepted here and the hub-level layer (D68) wraps these routes later.
    let rig = rig();
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(COLLECTION)
        .header(header::HOST, crate::rig::HOST)
        .header(hub_platform::TEST_USER_HEADER, "alice")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", "abc-123")
        .body(Body::from(r#"{"label":"laptop"}"#))
        .unwrap();
    let response = tower::ServiceExt::oneshot(rig.router.clone(), request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}
