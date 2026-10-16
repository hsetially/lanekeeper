//! S11, S14: the limits and checks of the server that are not about who is calling: the size of a request, the `Host` it names.

use axum::body::Body;
use axum::http::{Method, StatusCode, header};
use domain::{Role, UserStatus};
use serde_json::{Value, json};

use crate::rig::Rig;

fn initialize_body() -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }
    })
}

fn rig() -> Rig {
    let rig = Rig::oauth();
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    rig.verifier.accept("tok-alice", "alice");
    rig
}

#[tokio::test]
async fn a_request_for_another_host_is_refused() {
    let rig = rig();
    for host in [
        "evil.example",
        "hub.example.test.evil.example",
        "127.0.0.1:8799",
        "",
    ] {
        let alice = rig.client(Some("tok-alice")).at_host(host);
        let response = alice.post(&initialize_body()).await;
        assert!(
            matches!(response.status(), StatusCode::FORBIDDEN | StatusCode::BAD_REQUEST),
            "{host:?}: {}",
            response.status()
        );
    }
    // The public host with its default port, and in capitals, is the same host.
    for host in ["hub.example.test:443", "HUB.EXAMPLE.TEST"] {
        let alice = rig.client(Some("tok-alice")).at_host(host);
        assert_eq!(
            alice.post(&initialize_body()).await.status(),
            StatusCode::OK,
            "{host}"
        );
    }
}

#[tokio::test]
async fn oversized_body_is_413() {
    let rig = rig();
    let alice = rig.client(Some("tok-alice"));
    // Declared: refused before the token is looked at.
    let request = alice
        .request(Method::POST)
        .header(header::CONTENT_LENGTH, (4 * 1024 * 1024 + 1).to_string())
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(alice.send(request).await.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(rig.verifier.calls(), 0);
    // Not declared: rmcp counts the bytes as they arrive.
    let request = alice
        .request(Method::POST)
        .body(Body::from(vec![b' '; 4 * 1024 * 1024 + 1]))
        .unwrap();
    assert_eq!(alice.send(request).await.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
