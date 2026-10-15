//! Plan 07, T6 (D73, D76): the context that goes with a change. History, drift and the drift filter of `list_files` give each change's
//! attribution (source, confidence and actor) and its severity.
//!
//! D73: "The UI, the audit log and MCP show both the source and the confidence". A source with no confidence, or a confidence with
//! no source, would be read as more or less than the evidence says, so the two travel together or not at all. Every combination of
//! source and confidence, and every severity, is run through all three tools, so a field that one of them dropped for one value
//! is found.

use domain::{
    Actor, Attribution, AttributionSource, Confidence, DriftState, Evidence, JobRef, ObservationSource, Role,
    Severity, Timestamp, Version,
};
use serde_json::{Value, json};

use crate::fix::*;
use crate::rig::Rig;
use crate::tools_rig::{call, signed_in};

const PATH: &str = "tx-core/app.yml";

/// Every source with every confidence, and the severities in turn.
fn combinations() -> Vec<(AttributionSource, Confidence, Severity)> {
    let mut out = Vec::new();
    let mut n = 0;
    for source in AttributionSource::ALL {
        for confidence in Confidence::ALL {
            out.push((source, confidence, Severity::ALL[n % Severity::ALL.len()]));
            n += 1;
        }
    }
    out
}

fn actor_for(n: usize) -> Option<Actor> {
    match n % 4 {
        0 => Some(Actor::OsLogin { login: st("jdoe") }),
        1 => Some(Actor::User {
            user: crate::rig::uid("alice"),
        }),
        2 => Some(Actor::Job {
            job: JobRef::new("sync-sit1", "7c1e").unwrap(),
        }),
        _ => None,
    }
}

fn attribution_of(n: usize, source: AttributionSource, confidence: Confidence) -> Attribution {
    Attribution {
        source,
        confidence,
        actor: actor_for(n),
        evidence: Evidence {
            executable: (n % 2 == 0).then(|| st("vim")),
            login_identity: None,
            observed_at: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
        },
    }
}

/// The three tools, over the same 24 changes: `(tool, the items of its answer)`.
async fn answers() -> Vec<(&'static str, Vec<Value>)> {
    let rig = Rig::oauth();
    assert!(
        rig.registry
            .inner
            .seed(&sid("sit1"), &[(np(PATH), bytes("a: 1\n"))])
    );
    let mut files = Vec::new();
    let mut versions = Vec::new();
    for (n, (source, confidence, severity)) in combinations().into_iter().enumerate() {
        let attribution = attribution_of(n, source, confidence);
        let mut e = entry(&format!("dir/f{n:02}.yml"), DriftState::NfsAhead);
        e.attribution = Some(attribution.clone());
        e.severity = Some(severity);
        files.push(e);
        versions.push(Version {
            hash: hash_of(format!("v{n}").as_bytes()),
            observed_at: Timestamp::from_unix_millis(1_700_000_000_000 + i64::try_from(n).unwrap() * 1000),
            source: ObservationSource::Scan,
            size: 10,
            attribution,
            severity: Some(severity),
            commit: None,
        });
    }
    rig.registry.inner.set_drift(&sid("sit1"), files);
    rig.registry.set_history(&sid("sit1"), &np(PATH), versions);
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    let limit = json!({"limit": 200});
    let with = |mut base: Value| {
        base.as_object_mut()
            .unwrap()
            .extend(limit.as_object().unwrap().clone());
        base
    };
    let drift = call(&mut c, "get_drift", &with(json!({"swimlane": "sit1"}))).await;
    let listed = call(
        &mut c,
        "list_files",
        &with(json!({"swimlane": "sit1", "drift": "nfs_ahead"})),
    )
    .await;
    let history = call(
        &mut c,
        "get_history",
        &with(json!({"swimlane": "sit1", "path": PATH})),
    )
    .await;
    let items = |called: &crate::tools_rig::Called, key: &str| called.ok()[key].as_array().unwrap().clone();
    vec![
        ("get_drift", items(&drift, "files")),
        ("list_files", items(&listed, "files")),
        ("get_history", items(&history, "versions")),
    ]
}

#[tokio::test]
async fn attribution_and_severity_present_in_history_drift_and_list_files() {
    let expected = combinations();
    for (tool, items) in answers().await {
        assert_eq!(items.len(), expected.len(), "{tool}: every change is listed");
        // The three come in the order the registry gave them.
        let ordered: Vec<&Value> = items.iter().collect();
        for (n, ((source, confidence, severity), item)) in expected.iter().zip(ordered).enumerate() {
            let attribution = &item["attribution"];
            assert_eq!(attribution["source"], source.as_str(), "{tool} #{n}");
            assert_eq!(attribution["confidence"], confidence.as_str(), "{tool} #{n}");
            assert_eq!(item["severity"], severity.as_str(), "{tool} #{n}");
            // The actor, where there is one, and the executable that the sentinel saw.
            match actor_for(n) {
                Some(Actor::OsLogin { .. }) => {
                    assert_eq!(
                        attribution["actor"],
                        json!({"kind": "os_login", "id": "jdoe"}),
                        "{tool} #{n}"
                    );
                }
                Some(Actor::User { user }) => {
                    assert_eq!(
                        attribution["actor"],
                        json!({"kind": "user", "id": user.to_string()}),
                        "{tool} #{n}"
                    );
                }
                Some(Actor::Job { .. }) => {
                    assert_eq!(attribution["actor"]["kind"], "job", "{tool} #{n}");
                    assert_eq!(attribution["actor"]["id"], "sync-sit1", "{tool} #{n}");
                }
                None => assert_eq!(attribution["actor"], Value::Null, "{tool} #{n}"),
            }
            assert_eq!(
                attribution["executable"],
                if n % 2 == 0 { json!("vim") } else { Value::Null },
                "{tool} #{n}"
            );
        }
    }
}

#[tokio::test]
async fn source_and_confidence_always_shown_together() {
    for (tool, items) in answers().await {
        for item in &items {
            let attribution = &item["attribution"];
            // Both keys, both strings: never one without the other.
            assert!(attribution["source"].is_string(), "{tool}: {attribution}");
            assert!(attribution["confidence"].is_string(), "{tool}: {attribution}");
        }
    }
    // A change with no attribution at all (a file the sentinel has not looked at) shows neither, not half of one.
    let rig = Rig::oauth();
    assert!(
        rig.registry
            .inner
            .seed(&sid("sit1"), &[(np("a.yml"), bytes("a: 1\n"))])
    );
    rig.registry
        .inner
        .set_drift(&sid("sit1"), vec![entry("a.yml", DriftState::NfsAhead)]);
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    let called = call(&mut c, "get_drift", &json!({"swimlane": "sit1"})).await;
    assert_eq!(called.ok()["files"][0]["attribution"], Value::Null);
    assert_eq!(called.ok()["files"][0]["severity"], Value::Null);
    // And the contract itself requires the pair: the schema of an attribution has both in `required`.
    let contract = crate::tools_rig::contract();
    for tool in ["get_drift", "list_files", "get_history"] {
        let spec = contract.tools().iter().find(|t| t.name() == tool).unwrap();
        let schema = Value::Object((**spec.result()).clone());
        let required = schema["$defs"]["Attribution"]["required"].as_array().unwrap();
        for field in ["source", "confidence"] {
            assert!(
                required.iter().any(|r| r == field),
                "{tool}: the schema does not require `{field}` of an attribution"
            );
        }
    }
}
