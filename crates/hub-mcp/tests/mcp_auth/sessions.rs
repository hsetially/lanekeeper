//! Rule 5, S14: how many MCP sessions a user and a replica may hold, and what frees a slot.

use axum::http::StatusCode;
use domain::{Role, UserStatus};
use serde_json::json;

use crate::rig::{Rig, uid};

fn rig(per_user: u32, per_replica: u32) -> Rig {
    Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.limits.sessions_per_user = per_user;
            s.limits.sessions_per_replica = per_replica;
            s
        },
    )
}

fn user(rig: &Rig, key: &str) -> crate::support::Client {
    rig.add_user(key, Some(Role::Viewer), UserStatus::Active);
    rig.verifier.accept(&format!("tok-{key}"), key);
    rig.client(Some(&format!("tok-{key}")))
}

fn initialize_body() -> serde_json::Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}})
}

#[tokio::test]
async fn a_user_may_hold_five_sessions_and_not_a_sixth() {
    let rig = rig(5, 200);
    let alice = user(&rig, "alice");
    let mut clients = Vec::new();
    for _ in 0..5 {
        let mut c = alice.with_session(None);
        c.initialize(&json!({})).await.unwrap();
        clients.push(c);
    }
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 5);
    let sixth = alice.post(&initialize_body()).await;
    assert_eq!(sixth.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(sixth.headers().contains_key("retry-after"));
    assert_eq!(
        rig.state.counted_sessions(Some(&uid("alice"))),
        5,
        "a refused request holds nothing"
    );
    // Another user is not affected.
    let mut bob = user(&rig, "bob");
    bob.initialize(&json!({})).await.unwrap();
    // Ending one of Alice's makes room for one.
    assert!(clients[0].delete_session().await.is_success());
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 4);
    let mut again = alice.with_session(None);
    again.initialize(&json!({})).await.unwrap();
    assert_eq!(
        alice.post(&initialize_body()).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // A request on a session that exists is not a new session and is never refused for the count.
    let ok = clients[1].call_tool_to_end("list_swimlanes", &json!({})).await;
    assert!(ok.get("result").is_some(), "{ok}");
}

#[tokio::test]
async fn the_replica_cap_refuses_with_unavailable() {
    let rig = rig(5, 2);
    let mut a = user(&rig, "alice");
    let mut b = user(&rig, "bob");
    let carol = user(&rig, "carol");
    a.initialize(&json!({})).await.unwrap();
    b.initialize(&json!({})).await.unwrap();
    assert_eq!(rig.state.counted_sessions(None), 2);
    let refused = carol.post(&initialize_body()).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(refused.headers().contains_key("retry-after"));
    assert_eq!(rig.state.counted_sessions(None), 2);
    a.delete_session().await;
    let mut carol = carol;
    carol.initialize(&json!({})).await.unwrap();
}

#[tokio::test]
async fn a_request_that_opens_no_session_holds_no_slot() {
    let rig = rig(1, 10);
    let alice = user(&rig, "alice");
    // Not JSON-RPC at all, and an initialize with a protocol error: neither makes a session, and neither keeps the slot.
    for body in [
        json!({"nonsense": true}),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    ] {
        let response = alice.post(&body).await;
        assert!(!response.status().is_success(), "{}", response.status());
        assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 0);
    }
    let mut c = alice.with_session(None);
    c.initialize(&json!({})).await.unwrap();
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 1);
}

#[tokio::test(start_paused = true)]
async fn an_idle_session_frees_its_slot() {
    let rig = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.limits.sessions_per_user = 1;
            s.limits.session_idle = std::time::Duration::from_secs(60);
            s
        },
    );
    let mut alice = user(&rig, "alice");
    alice.initialize(&json!({})).await.unwrap();
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 1);
    assert_eq!(
        user(&rig, "alice").post(&initialize_body()).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // rmcp closes a session that has been idle for its keep-alive, and the count follows.
    tokio::time::sleep(std::time::Duration::from_secs(61)).await;
    for _ in 0..50 {
        if rig.state.counted_sessions(Some(&uid("alice"))) == 0 {
            break;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 0);
    let mut again = user(&rig, "alice");
    again.initialize(&json!({})).await.unwrap();
}

/// Found by the fuzz target `mcp_http_body`: `initialize` is the first request of a session and is sent once. rmcp's own handler
/// overwrites what the session negotiated with whatever the second one declares, and a version at or past 2026-07-28 puts the
/// session on the inline lifecycle, where `ping` is "method not found" and every request needs `_meta`. Only the caller's own
/// session was ever hurt, but a protocol violation must not change a session's state.
#[tokio::test]
async fn a_second_initialize_is_refused_and_the_session_keeps_its_terms() {
    let rig = rig(5, 200);
    let mut alice = user(&rig, "alice");
    alice.initialize(&json!({})).await.unwrap();
    for version in ["2025-06-18", "4050-06-18", "2026-07-28"] {
        let mut body = initialize_body();
        body["id"] = json!(7);
        body["params"]["protocolVersion"] = json!(version);
        let response = alice.post(&body).await;
        assert_eq!(response.status(), StatusCode::OK, "{version}");
        let messages = crate::support::Sse::new(response.into_body()).all().await;
        let answer = messages.iter().find(|m| m["id"] == json!(7)).expect("an answer");
        assert!(
            answer.get("result").is_none(),
            "{version}: a second initialize was accepted: {answer}"
        );
        assert_eq!(answer["error"]["code"], json!(-32600), "{version}");
    }
    // The session is as it was: a ping is answered, and so is a tool call.
    let response = alice
        .post(&json!({"jsonrpc": "2.0", "id": 8, "method": "ping"}))
        .await;
    let messages = crate::support::Sse::new(response.into_body()).all().await;
    assert!(
        messages
            .iter()
            .any(|m| m["id"] == json!(8) && m.get("result").is_some()),
        "{messages:?}"
    );
    let listed = alice.call_tool_to_end("list_swimlanes", &json!({})).await;
    assert!(listed.get("result").is_some(), "{listed}");
    // It is still one session of alice's.
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 1);
}
