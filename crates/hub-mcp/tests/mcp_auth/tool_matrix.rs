//! S4, S14: every tool by every kind of user. The role is checked on every call, before the arguments are read; a user who is
//! pending or disabled never reaches a tool; the list shows what the caller may call.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use domain::{Role, UserStatus};
use hub_mcp::tools::ToolName;
use serde_json::{Value, json};

use crate::args::minimal_examples;
use crate::rig::Rig;
use crate::tools_rig::{call, contract, signed_in};

fn rig() -> Rig {
    Rig::oauth()
}

const ROLES: [Role; 4] = [Role::Viewer, Role::Editor, Role::Operator, Role::Admin];

#[tokio::test]
async fn every_tool_by_every_user_state() {
    let rig = rig();
    let spec = contract();
    for role in ROLES {
        let key = format!("user-{}", role.as_str());
        let mut client = signed_in(&rig, &key, role).await;
        for (tool, args) in minimal_examples() {
            let required = spec.tool(tool.as_str()).unwrap().role();
            let called = call(&mut client, tool.as_str(), &args).await;
            if tool == ToolName::NotifyServices {
                // Off by default: forbidden to everybody, whatever the role (the contract's text).
                assert_eq!(called.err(), "forbidden", "{tool} for {role:?}");
            } else if role.at_least(required) {
                // Allowed by role: it may fail for any other reason (the fakes know nothing of the swimlane), never as forbidden.
                if called.is_error() {
                    assert_ne!(called.err(), "forbidden", "{tool} for {role:?}");
                } else {
                    called.ok();
                }
            } else {
                assert_eq!(called.err(), "forbidden", "{tool} for {role:?}");
            }
        }
    }
    // Pending and disabled users, and a user with no role, never reach a tool: the gate answers 403 on every request.
    for (key, role, status) in [
        ("pending", Some(Role::Admin), UserStatus::Pending),
        ("disabled", Some(Role::Admin), UserStatus::Disabled),
        ("norole", None, UserStatus::Active),
    ] {
        rig.add_user(key, role, status);
        rig.verifier.accept(&format!("tok-{key}"), key);
        let client = rig.client(Some(&format!("tok-{key}")));
        let response = client
            .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}}))
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{key}");
    }
}

#[tokio::test]
async fn role_is_checked_before_arguments_are_parsed() {
    let rig = rig();
    let mut viewer = signed_in(&rig, "alice", Role::Viewer).await;
    // A viewer calling an Operator tool with arguments that are nonsense is told it may not, not that the arguments are wrong:
    // the arguments of a tool the caller may not use are never looked at.
    for (tool, role) in [
        ("restart_service", "operator"),
        ("apply_change", "editor"),
        ("upload_file", "editor"),
    ] {
        let called = call(&mut viewer, tool, &json!({"nonsense": [1, 2, 3], "x": null})).await;
        assert_eq!(called.err(), "forbidden", "{tool} ({role})");
    }
    // The same nonsense to a tool the viewer may use is an argument error.
    let called = call(&mut viewer, "get_swimlane", &json!({"nonsense": 1})).await;
    assert_eq!(called.err(), "invalid_argument");
}

#[tokio::test]
async fn tools_list_shows_only_callable_tools() {
    let rig = rig();
    let spec = contract();
    for role in ROLES {
        let mut client = signed_in(&rig, &format!("lister-{}", role.as_str()), role).await;
        let id = client.id();
        let response = client
            .post(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}))
            .await;
        let mut sse = crate::support::Sse::new(response.into_body());
        let (_, message) = sse.until_response(id).await.unwrap();
        let listed: BTreeSet<String> = message["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        let expected: BTreeSet<String> = spec
            .tools()
            .iter()
            .filter(|t| role.at_least(t.role()) && t.name() != "notify_services")
            .map(|t| t.name().to_owned())
            .collect();
        assert_eq!(listed, expected, "{role:?}");
        // Every tool in the list has a name, a description, an input schema and annotations.
        for tool in message["result"]["tools"].as_array().unwrap() {
            assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["annotations"].is_object());
        }
    }
}

#[tokio::test]
async fn unknown_tool_is_a_protocol_error() {
    let rig = rig();
    let mut client = signed_in(&rig, "alice", Role::Admin).await;
    let response = client.call_tool_to_end("rm_rf", &json!({})).await;
    assert!(response.get("result").is_none(), "{response}");
    assert_eq!(response["error"]["code"], -32602, "{response}");
    // A tool name that is the right one in another spelling is not that tool.
    for name in ["LIST_SWIMLANES", "list_swimlanes ", "list-swimlanes", ""] {
        let response = client.call_tool_to_end(name, &json!({})).await;
        assert!(response.get("error").is_some(), "{name:?}: {response}");
    }
}

#[tokio::test]
async fn a_role_change_takes_effect_on_the_next_call() {
    let rig = rig();
    let mut client = signed_in(&rig, "alice", Role::Operator).await;
    let args = json!({"swimlane": "sit1", "service": {"namespace": "apps", "name": "core"}});
    let before = call(&mut client, "restart_service", &args).await;
    before.ok();
    // The directory now says Viewer. The session and the token are unchanged.
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Active);
    let after = call(&mut client, "restart_service", &args).await;
    assert_eq!(after.err(), "forbidden");
    // A disabled user is turned away by the gate, whatever session they hold.
    rig.add_user("alice", Some(Role::Viewer), UserStatus::Disabled);
    let (_, outcome) = client
        .call_tool("get_swimlane", &json!({"swimlane": "sit1"}))
        .await;
    assert_eq!(outcome.err(), Some(StatusCode::FORBIDDEN));
}

#[tokio::test]
async fn notify_is_off_unless_enabled() {
    // Off by default: absent from the list, and a call is forbidden, to an Admin as to anyone.
    let rig = rig();
    let mut admin = signed_in(&rig, "root", Role::Admin).await;
    let args = json!({"swimlane": "sit1", "paths": ["a.yml"]});
    assert_eq!(
        call(&mut admin, "notify_services", &args).await.err(),
        "forbidden"
    );
    // Switched on: an Operator sees it and gets as far as the gate (the web UI link), a Viewer is still refused.
    let rig = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.tools.notify_services_enabled = true;
            s
        },
    );
    let mut operator = signed_in(&rig, "op", Role::Operator).await;
    let called = call(&mut operator, "notify_services", &args).await;
    assert_eq!(called.ok()["result"]["outcome"], "draft_link");
    let mut editor = signed_in(&rig, "ed", Role::Editor).await;
    assert_eq!(
        call(&mut editor, "notify_services", &args).await.err(),
        "forbidden"
    );
    let _ = Value::Null;
}
