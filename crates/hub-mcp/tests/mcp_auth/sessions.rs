//! Rule 5, S14: how many MCP sessions a user and a replica may hold, and what frees a slot.

use axum::body::Body;
use axum::http::{Method, StatusCode};
use domain::{Role, UserStatus};
use futures::FutureExt;
use serde_json::json;
use tower::ServiceExt;

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

/// An `initialize` request, sent and polled `polls` times (at least once), and then dropped: the client went away before the
/// response head. Returns whether the request was still waiting when it was dropped (it had not finished).
async fn initialize_dropped_after(client: &crate::support::Client, polls: usize) -> bool {
    let request = client
        .request(Method::POST)
        .body(Body::from(serde_json::to_vec(&initialize_body()).unwrap()))
        .unwrap();
    let mut in_flight = Box::pin(client.app.clone().oneshot(request));
    for _ in 0..polls {
        if (&mut in_flight).now_or_never().is_some() {
            return false;
        }
        tokio::task::yield_now().await;
    }
    true
}

/// Let the tasks that a dropped request started run: it closes the session it made, on the runtime. No clock is involved, only
/// turns of the scheduler, so this is as deterministic as the runtime is.
async fn settle() {
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
}

/// Round 2, blocking 1: the gate reserves a slot before it hands the request to rmcp. A client that goes away before the response
/// head drops the gate's future, and the slot must go with it. Before the fix five such requests locked alice out for good (and the
/// same, spread over enough accounts, locked the replica out).
#[tokio::test]
async fn a_dropped_initialize_releases_its_slot() {
    let rig = rig(5, 200);
    let alice = user(&rig, "alice");
    for n in 0..5 {
        assert!(
            initialize_dropped_after(&alice, 1).await,
            "request {n} finished at once, so nothing was in flight when it was dropped"
        );
    }
    settle().await;
    assert_eq!(
        rig.state.counted_sessions(Some(&uid("alice"))),
        0,
        "a dropped request keeps its slot"
    );
    assert_eq!(rig.state.counted_sessions(None), 0);
    // The next real initialize is not refused for the count.
    let mut real = alice.with_session(None);
    real.initialize(&json!({})).await.unwrap();
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 1);
}

/// The slot is not all a dropped request leaves behind: by the first poll rmcp has already made the session (the client never
/// learned its id, so it can neither use nor end it). Releasing only the count would let one user fill rmcp's table with sessions
/// that nobody can reach, for as long as the idle expiry (30 minutes), past the cap of five. The dropped request closes the session.
#[tokio::test]
async fn a_dropped_initialize_leaves_no_session_behind_in_rmcp() {
    // A replica of five: if the five dropped sessions stayed, rmcp's own ceiling would refuse the real ones below.
    let rig = rig(5, 5);
    let alice = user(&rig, "alice");
    for n in 0..5 {
        assert!(
            initialize_dropped_after(&alice, 1).await,
            "request {n} finished at once"
        );
    }
    settle().await;
    assert_eq!(
        rig.state.live_sessions().await,
        0,
        "a session nobody can reach is left in rmcp"
    );
    for _ in 0..5 {
        alice.with_session(None).initialize(&json!({})).await.unwrap();
    }
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 5);
    assert_eq!(rig.state.live_sessions().await, 5);
}

/// Whatever the point at which the client goes away, the count and rmcp's table agree afterwards, and a request that got as far as
/// the response head leaves a session that belongs to its user. There is no point between rmcp's answer and the owner table at
/// which the future could be dropped (the gate does not await there), and this holds the line: if a later change put an await
/// there, some `polls` below would leave a session that is counted for nobody, or a count for no session.
#[tokio::test]
async fn a_request_dropped_at_any_poll_leaves_the_count_equal_to_the_sessions() {
    let (mut unfinished, mut finished) = (0, 0);
    for polls in 1..=12 {
        let rig = rig(5, 200);
        let alice = user(&rig, "alice");
        let request = alice
            .request(Method::POST)
            .body(Body::from(serde_json::to_vec(&initialize_body()).unwrap()))
            .unwrap();
        let mut in_flight = Box::pin(alice.app.clone().oneshot(request));
        let mut head = None;
        for _ in 0..polls {
            if let Some(response) = (&mut in_flight).now_or_never() {
                head = Some(response.unwrap());
                break;
            }
            tokio::task::yield_now().await;
        }
        drop(in_flight);
        settle().await;
        let counted = rig.state.counted_sessions(Some(&uid("alice")));
        let live = rig.state.live_sessions().await;
        assert_eq!(
            counted, live,
            "polls {polls}: the count and rmcp's table disagree"
        );
        match head {
            None => {
                unfinished += 1;
                assert_eq!(
                    counted, 0,
                    "polls {polls}: a request that never answered keeps a session"
                );
            }
            Some(response) => {
                finished += 1;
                assert_eq!(counted, 1, "polls {polls}: an answered request has its session");
                // The session is alice's: she can use it, and ending it frees the slot.
                let id = response.headers()["mcp-session-id"].to_str().unwrap().to_owned();
                drop(response);
                let mine = alice.with_session(Some(id));
                assert!(mine.delete_session().await.is_success(), "polls {polls}");
                settle().await;
                assert_eq!(
                    rig.state.counted_sessions(Some(&uid("alice"))),
                    0,
                    "polls {polls}"
                );
                assert_eq!(rig.state.live_sessions().await, 0, "polls {polls}");
            }
        }
    }
    assert!(
        unfinished > 0,
        "no poll dropped a request that was still in flight"
    );
    assert!(finished > 0, "no poll let a request finish");
}

/// The success path: the slot moves to the owner table, stays while the session lives, and goes back exactly once when the table
/// forgets the entry (the idle expiry and the eviction of the table work through the same listener; `DELETE` is in the test above).
#[tokio::test]
async fn an_opened_session_keeps_its_slot_until_the_owner_table_forgets_it() {
    let rig = rig(5, 200);
    let mut alice = user(&rig, "alice");
    alice.initialize(&json!({})).await.unwrap();
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 1);
    // Requests on the session neither take nor give a slot.
    let listed = alice.call_tool_to_end("list_swimlanes", &json!({})).await;
    assert!(listed.get("result").is_some(), "{listed}");
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 1);
    // A second session, then the table forgets both entries: each frees one slot, none frees two.
    let mut second = user(&rig, "alice");
    second.initialize(&json!({})).await.unwrap();
    assert_eq!(rig.state.counted_sessions(None), 2);
    rig.state.forget_session_owners();
    assert_eq!(rig.state.counted_sessions(Some(&uid("alice"))), 0);
    assert_eq!(rig.state.counted_sessions(None), 0);
    // And the user can open again.
    user(&rig, "alice").initialize(&json!({})).await.unwrap();
    assert_eq!(rig.state.counted_sessions(None), 1);
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

/// Found by the threat model (T5): the table that remembers who opened each session held 4,096 entries while the validator accepted
/// a replica cap up to 100,000. A deployment above 4,096 lost the owner of a live session (the session then answered 404, and the
/// owner's count fell below the real number of sessions). The table is sized from the cap; this opens more sessions than the old
/// table could hold and asks each user's first and last one for its tools.
#[tokio::test]
async fn a_replica_cap_above_the_old_owner_table_still_remembers_every_owner() {
    const USERS: usize = 5;
    const PER_USER: usize = 900;
    let rig = rig(1_000, 4_500);
    let mut sessions = Vec::new();
    for u in 0..USERS {
        let base = user(&rig, &format!("user{u}"));
        let mut mine = Vec::new();
        for _ in 0..PER_USER {
            let mut c = base.with_session(None);
            c.initialize(&json!({})).await.unwrap();
            mine.push(c);
        }
        sessions.push(mine);
    }
    assert_eq!(
        rig.state.counted_sessions(None),
        USERS * PER_USER,
        "a live session lost its owner"
    );
    for (u, mine) in sessions.iter_mut().enumerate() {
        assert_eq!(
            rig.state.counted_sessions(Some(&uid(&format!("user{u}")))),
            PER_USER
        );
        for index in [0, PER_USER - 1] {
            let listed = mine[index].call_tool_to_end("list_swimlanes", &json!({})).await;
            assert!(
                listed.get("result").is_some(),
                "user {u}, session {index}: {listed}"
            );
        }
    }
}

/// The two bounds agree by construction: for every replica cap the validator accepts, the table can hold at least that many owners,
/// with room to spare (its accounting lags a removal, and a table at its limit refuses new entries).
#[test]
fn the_owner_table_is_never_smaller_than_the_replica_cap_the_validator_accepts() {
    use hub_mcp::limits::LimitSettings;
    for cap in [1_u32, 5, 200, 2_048, 4_095, 4_096, 4_097, 10_000, 100_000] {
        let limits = LimitSettings {
            sessions_per_replica: cap,
            ..LimitSettings::default()
        };
        assert!(limits.validate().is_ok(), "{cap}");
        let table = hub_mcp::server::owners_capacity(cap);
        assert!(table >= 2 * u64::from(cap), "cap {cap}: the table holds {table}");
    }
    // The default is unchanged.
    assert_eq!(hub_mcp::server::owners_capacity(200), 4_096);
    // A cap the validator refuses is not this table's business, but it must not overflow.
    let _ = hub_mcp::server::owners_capacity(u32::MAX);
}
