//! Plan 07, T7 (D85, D86, Q37; S4, S14): `notify_services`, the only tool that asks the config-server to refresh files.
//!
//! The config-server's `/update-resources` endpoint has no authentication (Q37). So the tool is Operator only, is off until Q37 is
//! answered, asks the person before it acts, and does nothing but call the write service's `notify`, which audits the call and sends
//! it through the swimlane's agent (never from the hub). The write service is a fake here; what these tests pin is what this crate
//! asks of it, and when. The question the person reads is a golden file (`tests/golden/notify_services*.snap`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use domain::Role;
use hub_mcp::auth::pat::TokenSettings;
use hub_mcp::config::{AuthMode, ConfirmMode};
use serde_json::{Value, json};

use crate::capture;
use crate::confirm::{
    accept, answer, approval_client, ask, decline, finish, forms, no_mutation, seed, send_again,
};
use crate::fix::{np, sid};
use crate::rig::Rig;
use crate::support::elicitation_capabilities;
use crate::tools_rig::{call, signed_in, signed_in_with};

const PATHS: [&str; 3] = ["tx-core/app-sit1.yml", "tx-core/app.yml", "channels.yml"];

fn args() -> Value {
    json!({"swimlane": "sit1", "paths": PATHS})
}

/// A deployment where the switch is on and the write budget does not get in the way of a test that makes several calls.
fn enabled(confirm: ConfirmMode) -> Rig {
    Rig::build(AuthMode::Oauth, false, TokenSettings::default(), |mut s| {
        s.tools.notify_services_enabled = true;
        s.config.confirm_mode = confirm;
        s.limits.write_burst = 100;
        s.limits.write_per_minute = 6_000;
        s
    })
}

fn sent(rig: &Rig) -> Vec<(domain::SwimlaneId, Vec<domain::NfsPath>)> {
    rig.writes.inner.notifications()
}

fn expected_paths() -> Vec<domain::NfsPath> {
    PATHS.iter().map(|p| np(p)).collect()
}

/// The names of the tools `tools/list` shows to the caller.
async fn listed(client: &mut crate::support::Client) -> BTreeSet<String> {
    let id = client.id();
    let response = client
        .post(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}))
        .await;
    let mut sse = crate::support::Sse::new(response.into_body());
    let (_, message) = sse.until_response(id).await.unwrap();
    message["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------------
// Who may ask, and what is asked
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn notify_services_needs_operator_and_confirmation() {
    let rig = enabled(ConfirmMode::Elicitation);
    seed(&rig);

    // A Viewer and an Editor are refused before their arguments are read, and nothing is asked and no port is touched.
    for (key, role) in [("viewer", Role::Viewer), ("editor", Role::Editor)] {
        let mut client = forms(&rig, key, role).await;
        for arguments in [json!({"nonsense": [1, 2]}), args()] {
            let called = call(&mut client, "notify_services", &arguments).await;
            assert_eq!(called.err(), "forbidden", "{role:?}");
        }
    }
    assert!(
        rig.writes.log.all().is_empty(),
        "a port was called: {:?}",
        rig.writes.log.all()
    );

    // An Operator is asked, with a form of exactly one boolean, and nothing is sent while they decide.
    let mut op = forms(&rig, "op", Role::Operator).await;
    let asked = ask(&mut op, "notify_services", &args()).await;
    let schema = &asked.question["params"]["requestedSchema"];
    assert_eq!(schema["required"], json!(["confirm"]), "{schema}");
    let properties = schema["properties"].as_object().unwrap();
    assert_eq!(properties.len(), 1, "{schema}");
    assert_eq!(properties["confirm"]["type"], "boolean");
    no_mutation(&rig);
    assert!(sent(&rig).is_empty());

    // The question names what will happen: the swimlane, every path (there are few), and that the agent calls the config-server's
    // endpoint, which has no authentication.
    let message = asked.message();
    assert!(message.chars().count() <= 1_500, "{message}");
    assert!(message.contains("swimlane sit1"), "{message}");
    for path in PATHS {
        assert!(
            message.contains(&format!("\"{path}\"")),
            "{path} is not in {message}"
        );
    }
    assert!(message.contains("3 files"), "{message}");
    assert!(message.contains("supplied by the assistant"), "{message}");
    assert!(message.contains("agent"), "{message}");
    assert!(message.contains("/update-resources"), "{message}");
    assert!(message.contains("no authentication"), "{message}");
    assert!(
        message.ends_with("https://hub.example.test/swimlanes/sit1"),
        "{message}"
    );

    // Only the exact accept sends it, once, and the result is the write service's.
    let called = answer(&op, asked, &accept(), "notify_services").await;
    let data = called.ok();
    assert_eq!(data["result"]["outcome"], "applied");
    assert!(data["result"]["audit_id"].is_u64(), "{data}");
    assert_eq!(data["result"]["hash"], Value::Null);
    assert_eq!(sent(&rig), vec![(sid("sit1"), expected_paths())]);
    assert_eq!(rig.writes.log.of("notify").len(), 1);
    assert_eq!(rig.writes.mutations().len(), 1, "{:?}", rig.writes.log.all());

    // An Admin may ask too (the floor is Operator, and an Admin has it).
    let mut admin = forms(&rig, "root", Role::Admin).await;
    let asked = ask(&mut admin, "notify_services", &args()).await;
    answer(&admin, asked, &accept(), "notify_services").await.ok();
    assert_eq!(rig.writes.log.of("notify").len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_notification_that_is_not_confirmed_is_not_sent() {
    let rig = enabled(ConfirmMode::Elicitation);
    seed(&rig);
    let mut op = forms(&rig, "op", Role::Operator).await;

    let asked = ask(&mut op, "notify_services", &args()).await;
    let called = answer(&op, asked, &decline(), "notify_services").await;
    assert_eq!(called.err(), "confirmation_declined");

    // An accept that says `confirm: false` is a decline.
    let asked = ask(&mut op, "notify_services", &args()).await;
    let called = answer(
        &op,
        asked,
        &json!({"action": "accept", "content": {"confirm": false}}),
        "notify_services",
    )
    .await;
    assert_eq!(called.err(), "confirmation_declined");

    // A cancel is a cancel; an accept whose confirm is not the boolean `true` is no confirmation (the T3 tests try fourteen shapes
    // against the gate, and it is the same gate).
    let asked = ask(&mut op, "notify_services", &args()).await;
    let called = answer(&op, asked, &json!({"action": "cancel"}), "notify_services").await;
    assert_eq!(called.err(), "confirmation_cancelled");
    for reply in [
        json!({"action": "accept", "content": {"confirm": "true"}}),
        json!({"action": "accept", "content": {"confirm": true, "extra": 1}}),
        json!({"action": "accept"}),
    ] {
        let asked = ask(&mut op, "notify_services", &args()).await;
        let called = answer(&op, asked, &reply, "notify_services").await;
        let code = called.err();
        assert!(
            ["confirmation_cancelled", "confirmation_declined"].contains(&code.as_str()),
            "{reply} gave {code}"
        );
    }

    // A question that nobody answers ends on the confirmation timeout.
    let asked = ask(&mut op, "notify_services", &args()).await;
    let called = finish(asked, "notify_services").await;
    assert_eq!(called.err(), "confirmation_cancelled");

    no_mutation(&rig);
    assert!(sent(&rig).is_empty());
    assert!(rig.writes.log.of("notify").is_empty());
}

#[tokio::test]
async fn a_client_that_cannot_be_asked_and_draft_mode_get_a_link_and_touch_no_port() {
    // No form support: the answer is a link to the web UI, and the write service is not called, not even to read.
    let rig = enabled(ConfirmMode::Elicitation);
    seed(&rig);
    rig.writes.log.clear();
    let mut op = signed_in(&rig, "op", Role::Operator).await;
    let called = call(&mut op, "notify_services", &args()).await;
    let data = called.ok();
    assert_eq!(data["result"]["outcome"], "draft_link");
    for null in ["hash", "audit_id", "proposal_id"] {
        assert_eq!(data["result"][null], Value::Null);
    }
    assert_eq!(
        called.envelope()["web_link"],
        "https://hub.example.test/swimlanes/sit1"
    );
    assert!(rig.writes.log.all().is_empty(), "{:?}", rig.writes.log.all());
    assert!(sent(&rig).is_empty());

    // Draft mode: the deployment says it never asks, even a client that could be.
    let rig = enabled(ConfirmMode::Draft);
    seed(&rig);
    rig.writes.log.clear();
    let mut op = forms(&rig, "op", Role::Operator).await;
    let (id, sse) = op.call_tool("notify_services", &args()).await;
    let (before, response) = sse.unwrap().until_response(id).await.unwrap();
    assert!(
        before.iter().all(|m| m["method"] != "elicitation/create"),
        "a question was asked in draft mode"
    );
    assert_eq!(
        response["result"]["structuredContent"]["data"]["result"]["outcome"],
        "draft_link"
    );
    assert!(rig.writes.log.all().is_empty(), "{:?}", rig.writes.log.all());
    assert!(sent(&rig).is_empty());
}

#[tokio::test]
async fn a_user_who_needs_approval_gets_a_proposal_and_nothing_is_sent() {
    let rig = enabled(ConfirmMode::Elicitation);
    seed(&rig);
    let mut op = approval_client(&rig, "op", Role::Operator).await;
    let asked = ask(&mut op, "notify_services", &args()).await;
    // The person is told what will happen: a proposal, not a notification.
    let message = asked.message();
    assert!(
        message.contains("approval") && message.contains("proposal"),
        "{message}"
    );
    let called = answer(&op, asked, &accept(), "notify_services").await;
    let data = called.ok();
    assert_eq!(data["result"]["outcome"], "proposal_created");
    assert!(data["result"]["proposal_id"].is_u64(), "{data}");
    // The summary does not say "written": nothing was sent either.
    assert!(
        called.envelope()["summary"]
            .as_str()
            .unwrap()
            .contains("Nothing was done yet"),
        "{}",
        called.envelope()
    );
    assert!(
        sent(&rig).is_empty(),
        "the config-server was told before an admin approved"
    );
}

#[tokio::test]
async fn a_retry_of_the_same_request_notifies_once() {
    let rig = enabled(ConfirmMode::Elicitation);
    seed(&rig);
    let mut op = forms(&rig, "op", Role::Operator).await;
    let asked = ask(&mut op, "notify_services", &args()).await;
    let id = asked.id;
    let first = answer(&op, asked, &accept(), "notify_services").await;
    let audit_id = first.ok()["result"]["audit_id"].clone();

    // The client sends the same request again, with the id it used: the first outcome is returned and nobody is asked.
    let (before, again) = send_again(&op, id, "notify_services", &args()).await;
    assert!(before.is_empty(), "the retry asked a question: {before:?}");
    assert_eq!(again.ok()["result"]["audit_id"], audit_id);
    assert_eq!(rig.writes.log.of("notify").len(), 1);
    assert_eq!(sent(&rig).len(), 1);
}

#[tokio::test]
async fn a_swimlane_the_write_service_does_not_know_is_not_found() {
    let rig = enabled(ConfirmMode::Elicitation);
    seed(&rig);
    let mut op = forms(&rig, "op", Role::Operator).await;
    let asked = ask(
        &mut op,
        "notify_services",
        &json!({"swimlane": "nowhere", "paths": ["a.yml"]}),
    )
    .await;
    let called = answer(&op, asked, &accept(), "notify_services").await;
    assert_eq!(called.err(), "not_found");
    assert!(sent(&rig).is_empty());
}

// ---------------------------------------------------------------------------------------------------------------------
// Off by default
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn notify_is_off_unless_enabled() {
    // Off by default (Q37): absent from the list, and a call is forbidden, to an Admin as to anyone, and no port is touched.
    let rig = Rig::oauth();
    seed(&rig);
    rig.writes.log.clear();
    for role in [Role::Viewer, Role::Editor, Role::Operator, Role::Admin] {
        let mut client = signed_in_with(
            &rig,
            &format!("off-{}", role.as_str()),
            role,
            &elicitation_capabilities(),
        )
        .await;
        assert!(
            !listed(&mut client).await.contains("notify_services"),
            "{role:?} is shown a tool that is off"
        );
        for arguments in [args(), json!({"nonsense": 1})] {
            let called = call(&mut client, "notify_services", &arguments).await;
            assert_eq!(called.err(), "forbidden", "{role:?}");
        }
    }
    assert!(rig.writes.log.all().is_empty(), "{:?}", rig.writes.log.all());
    assert!(sent(&rig).is_empty());

    // Switched on: an Operator and an Admin see it, a Viewer and an Editor neither see it nor may call it.
    let rig = enabled(ConfirmMode::Elicitation);
    seed(&rig);
    for (role, shown) in [
        (Role::Viewer, false),
        (Role::Editor, false),
        (Role::Operator, true),
        (Role::Admin, true),
    ] {
        let mut client = forms(&rig, &format!("on-{}", role.as_str()), role).await;
        assert_eq!(
            listed(&mut client).await.contains("notify_services"),
            shown,
            "{role:?}"
        );
        if !shown {
            let called = call(&mut client, "notify_services", &args()).await;
            assert_eq!(called.err(), "forbidden", "{role:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Only the write service, which audits
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn notify_audits_through_the_write_service_only() {
    let rig = enabled(ConfirmMode::Elicitation);
    seed(&rig);
    let mut op = forms(&rig, "op", Role::Operator).await;
    rig.writes.log.clear();
    let asked = ask(&mut op, "notify_services", &args()).await;
    let called = answer(&op, asked, &accept(), "notify_services").await;
    let data = called.ok();

    // Exactly one call reached the write service, and it was `notify`, for the caller, as the MCP door, under a key made by this
    // crate from the request (D68). The audit event is the write service's, in its own transaction (rule 8); the id in the result
    // is the one it returned.
    let calls = rig.writes.log.all();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].starts_with("notify key=mcp-"), "{calls:?}");
    assert!(calls[0].contains(" via=mcp "), "{calls:?}");
    assert!(
        calls[0].contains(&format!("user={}", crate::rig::uid("op"))),
        "{calls:?}"
    );
    let key = calls[0]
        .split_whitespace()
        .find_map(|word| word.strip_prefix("key="))
        .unwrap();
    assert_eq!(key.len(), 68, "{key}");
    assert!(data["result"]["audit_id"].is_u64(), "{data}");
    assert_eq!(rig.writes.mutations(), calls);

    // The crate has no other way to reach the endpoint or to write an audit event for it: the endpoint is named in the question
    // and nowhere else, the crate has no HTTP client, and only the personal-token code uses the audit log.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = rust_sources(&root.join("src"));
    assert!(sources.len() > 20);
    for (path, text) in &sources {
        let in_question = path.ends_with("confirm/message.rs");
        for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
            if line.contains("update-resources") {
                assert!(
                    in_question,
                    "{} names the config-server's endpoint outside the question: {line}",
                    path.display()
                );
            }
            if !path.starts_with(root.join("src/auth")) {
                for audit in ["AuditLog", "audit.record", "AuditEvent", "AuditAction"] {
                    assert!(
                        !line.contains(audit),
                        "{} writes an audit event of its own ({audit}): {line}",
                        path.display()
                    );
                }
            }
        }
    }
    // The names of the crates it depends on (the comments of the manifest may name what it avoids).
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let dependencies: BTreeSet<&str> = manifest
        .split("[dev-dependencies]")
        .next()
        .unwrap()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split(['=', '.', ' ']).next())
        .filter(|name| !name.is_empty() && !name.starts_with('['))
        .collect();
    assert!(
        dependencies.contains("axum") && dependencies.contains("rmcp"),
        "{dependencies:?}"
    );
    for client in [
        "reqwest",
        "ureq",
        "isahc",
        "surf",
        "hyper",
        "hyper-util",
        "hyper-rustls",
        "attohttpc",
        "awc",
        "tonic",
        "curl",
    ] {
        assert!(
            !dependencies.contains(client),
            "the crate depends on an HTTP client ({client}), and only the agent may call the config-server"
        );
    }
}

fn rust_sources(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            out.push((path, text));
        }
    }
    out
}

#[tokio::test(start_paused = true)]
async fn a_notification_logs_the_outcome_and_none_of_the_paths() {
    // The server is built before the capture starts, as in the log test of the other write tools.
    let rig = enabled(ConfirmMode::Elicitation);
    let logs = capture::start();
    rig.writes.inner.seed(
        &sid("sit1"),
        &[(np("planted/notify-path-5b7a.yml"), crate::fix::bytes("a: 1\n"))],
    );
    let mut op = forms(&rig, "op", Role::Operator).await;
    let arguments = json!({"swimlane": "sit1", "paths": ["planted/notify-path-5b7a.yml"]});
    let asked = ask(&mut op, "notify_services", &arguments).await;
    answer(&op, asked, &accept(), "notify_services").await.ok();
    let asked = ask(&mut op, "notify_services", &arguments).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    answer(&op, asked, &decline(), "notify_services").await.err();

    let log = logs.text();
    assert!(log.contains("tool=\"notify_services\""), "{log}");
    assert!(log.contains("outcome=\"confirmation_declined\""), "{log}");
    for never in [
        "planted/notify-path-5b7a",
        "notify-path-5b7a",
        "update-resources",
        "supplied by the assistant",
        "hub.example.test",
        "tok-op",
    ] {
        assert!(!log.contains(never), "the log holds `{never}`:\n{log}");
    }
}
