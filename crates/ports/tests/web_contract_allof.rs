//! contract/08-web: an `allOf` whose closed member rejects the properties of its sibling can never validate a real body.
//!
//! `additionalProperties: false` looks at the `properties` of the schema that carries it and at nothing else, not at the
//! siblings of an `allOf`. So `allOf: [A, B]` with `A` closed rejects every key that only `B` declares, and a body that carries
//! one can never be valid. CG13 found it on `ConflictProblem` and `LockedProblem` (a closed extension next to the open
//! `Problem`); `SwimlaneDetail` had it too, and worse: both of its members were closed, `SwimlaneSummary` and the extension
//! with `services` and `auto_notify`, so not even the body of an old server validated. Three schemas that were already in the
//! document on `main` had it as well, `Me`, `SeverityRule` and `RetentionSettings`, and the human decided to fix them in this
//! PR with the technique of `SwimlaneDetail`: one closed object that repeats the properties of its members.
//!
//! This file scans the whole document (components, paths, responses, inline schemas) for the pattern, and expects to find
//! none: there is no list of known debt. It holds each of the four flat schemas to what the server sends: the members'
//! properties, each defined exactly as the member defines it, and the extra fields, in one closed object. Nothing here changes
//! what was valid before: no body ever validated against any of the four closed `allOf`s.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use domain::{
    ChangeKind, FindingKind, PickupState, Role, ServiceRef, ServiceState, Severity, ShortText,
    SwimlaneSummary, Timestamp, User, UserStatus,
};
use ports::conformance::sample::{user, user_id};
use serde_json::{Value, json};

const SPEC: &str = include_str!("../../../api/openapi.yaml");

fn doc() -> Value {
    serde_saphyr::from_str(SPEC).expect("api/openapi.yaml is valid YAML")
}

fn resolve<'a>(doc: &'a Value, v: &'a Value) -> &'a Value {
    let mut cur = v;
    for _ in 0..16 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
            return cur;
        };
        let pointer = r.strip_prefix('#').unwrap_or_else(|| panic!("external $ref {r}"));
        cur = doc
            .pointer(pointer)
            .unwrap_or_else(|| panic!("dangling $ref {r}"));
    }
    panic!("$ref chain too deep");
}

fn strings(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------------
// The scan
// ---------------------------------------------------------------------------------------------------------------------

/// The properties a schema declares, through `$ref` and through a nested `allOf`.
fn declared(d: &Value, schema: &Value) -> BTreeSet<String> {
    let s = resolve(d, schema);
    let mut out: BTreeSet<String> = s
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    for member in s.get("allOf").and_then(Value::as_array).into_iter().flatten() {
        out.extend(declared(d, member));
    }
    out
}

fn is_closed(d: &Value, schema: &Value) -> bool {
    resolve(d, schema).get("additionalProperties") == Some(&Value::Bool(false))
}

/// Every `allOf` of the document where a closed member does not declare a property of a sibling, as `location: what`.
/// The location is the JSON pointer of the schema that carries the `allOf`. A superset of "every member is closed": it also
/// finds a closed extension next to an open base, which is how CG13's defect looked.
fn offenders(d: &Value) -> BTreeSet<String> {
    fn walk(d: &Value, node: &Value, at: &str, out: &mut BTreeSet<String>) {
        match node {
            Value::Object(map) => {
                if let Some(members) = map.get("allOf").and_then(Value::as_array) {
                    for (i, closed) in members.iter().enumerate().filter(|(_, m)| is_closed(d, m)) {
                        let own = declared(d, closed);
                        for (j, sibling) in members.iter().enumerate().filter(|(j, _)| *j != i) {
                            let rejected: Vec<String> =
                                declared(d, sibling).difference(&own).cloned().collect();
                            if !rejected.is_empty() {
                                out.insert(format!(
                                    "{at}: member {i} is closed and rejects {rejected:?} of member {j}"
                                ));
                            }
                        }
                    }
                }
                for (k, v) in map {
                    walk(
                        d,
                        v,
                        &format!("{at}/{}", k.replace('~', "~0").replace('/', "~1")),
                        out,
                    );
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    walk(d, v, &format!("{at}/{i}"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(d, d, "#", &mut out);
    out
}

fn locations(found: &BTreeSet<String>) -> BTreeSet<String> {
    found
        .iter()
        .map(|f| f.split(": ").next().unwrap().to_owned())
        .collect()
}

#[test]
fn no_allof_closes_out_its_siblings() {
    let found = locations(&offenders(&doc()));
    assert!(
        found.is_empty(),
        "an allOf has a closed member that rejects what its sibling declares, so no real body validates: {found:#?}\n\
         (compose the schema as one closed object that repeats the fields, or open the members; `SwimlaneDetail` is the example)"
    );
}

/// The scan has teeth: it finds the two defects of this PR when they are put back, in a copy of the document.
#[test]
fn the_scan_finds_the_defects_it_was_written_for() {
    // CG13: a closed extension next to the open `Problem`. "Every member closed" would not see it.
    let mut d = doc();
    d["components"]["schemas"]["ConflictProblem"]["allOf"][1]["additionalProperties"] = json!(false);
    let found = locations(&offenders(&d));
    assert!(
        found.contains("#/components/schemas/ConflictProblem"),
        "{found:#?}"
    );

    // `SwimlaneDetail` as it was: the closed summary and a closed extension.
    let mut d = doc();
    d["components"]["schemas"]["SwimlaneDetail"] = closed_allof_detail();
    let found = locations(&offenders(&d));
    assert!(
        found.contains("#/components/schemas/SwimlaneDetail"),
        "{found:#?}"
    );

    // The three schemas that were fixed after `SwimlaneDetail`, each put back as it was.
    for (name, old_shape) in [
        ("Me", closed_allof_me()),
        ("SeverityRule", closed_allof_severity_rule()),
        ("RetentionSettings", closed_allof_retention()),
    ] {
        let mut d = doc();
        d["components"]["schemas"][name] = old_shape;
        let found = locations(&offenders(&d));
        assert!(
            found.contains(&format!("#/components/schemas/{name}")),
            "{name}: {found:#?}"
        );
        assert_eq!(
            found.len(),
            1,
            "{name} is the only offender once it is put back: {found:#?}"
        );
    }

    // An inline schema of a response is found too, not only the components.
    let mut d = doc();
    d["paths"]["/api/v1/swimlanes/{id}"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] =
        closed_allof_detail();
    let found = locations(&offenders(&d));
    assert!(
        found.contains(
            "#/paths/~1api~1v1~1swimlanes~1{id}/get/responses/200/content/application~1json/schema"
        ),
        "{found:#?}"
    );
}

/// And it does not cry wolf: members that are open, or closed over the properties of their siblings, are fine.
#[test]
fn the_scan_leaves_alone_what_validates() {
    let d = doc();
    // `TokenMissingAccessProblem` is an `allOf` of the open `Problem` and an open extension.
    let found = locations(&offenders(&d));
    assert!(
        !found.contains("#/components/schemas/TokenMissingAccessProblem"),
        "{found:#?}"
    );

    let mut d = doc();
    d["components"]["schemas"]["SwimlaneDetail"] = json!({
        "allOf": [
            {"type": "object", "properties": {"id": {"type": "string"}}, "additionalProperties": false},
            {"type": "object", "properties": {"id": {"type": "string"}}, "additionalProperties": false}
        ]
    });
    let found = locations(&offenders(&d));
    assert!(
        !found.contains("#/components/schemas/SwimlaneDetail"),
        "{found:#?}"
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// A small JSON Schema walker (the one of `web_contract_problems.rs`, kept in step by hand): `$ref`, `allOf`, `oneOf`,
// `anyOf`, `const`, `enum`, `type`, `required`, and `additionalProperties: false` the way the keyword is defined.
// ---------------------------------------------------------------------------------------------------------------------

fn check(d: &Value, schema: &Value, v: &Value, at: &str) -> Result<(), String> {
    let s = resolve(d, schema);
    if let Some(parts) = s.get("allOf").and_then(Value::as_array) {
        for p in parts {
            check(d, p, v, at)?;
        }
    }
    if let Some(arms) = s.get("oneOf").and_then(Value::as_array) {
        let ok = arms.iter().filter(|a| check(d, a, v, at).is_ok()).count();
        if ok != 1 {
            return Err(format!("{at}: matches {ok} arms of a oneOf, not exactly one"));
        }
    }
    if let Some(arms) = s.get("anyOf").and_then(Value::as_array)
        && !arms.iter().any(|a| check(d, a, v, at).is_ok())
    {
        return Err(format!("{at}: matches no arm of an anyOf"));
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        return Err(format!("{at}: {v} is not the constant {c}"));
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        return Err(format!("{at}: {v} is not one of {e:?}"));
    }
    if let Some(t) = s.get("type").and_then(Value::as_str) {
        let fits = match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            other => panic!("the walker does not know the type {other}"),
        };
        if !fits {
            return Err(format!("{at}: {v} is not of type {t}"));
        }
    }
    if let Some(obj) = v.as_object() {
        for r in s.get("required").map(strings).unwrap_or_default() {
            if !obj.contains_key(&r) {
                return Err(format!("{at}: `{r}` is required and absent"));
            }
        }
        let props = s.get("properties").and_then(Value::as_object);
        for (k, val) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(p) => check(d, p, val, &format!("{at}.{k}"))?,
                None if s.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!(
                        "{at}: `{k}` is not allowed (additionalProperties: false)"
                    ));
                }
                None => {}
            }
        }
    }
    if let Some(arr) = v.as_array() {
        if let Some(max) = s.get("maxItems").and_then(Value::as_u64)
            && arr.len() as u64 > max
        {
            return Err(format!("{at}: {} items, at most {max}", arr.len()));
        }
        if let Some(items) = s.get("items") {
            for (i, x) in arr.iter().enumerate() {
                check(d, items, x, &format!("{at}[{i}]"))?;
            }
        }
    }
    if let Some(text) = v.as_str()
        && let Some(max) = s.get("maxLength").and_then(Value::as_u64)
        && text.chars().count() as u64 > max
    {
        return Err(format!("{at}: more than {max} characters"));
    }
    if let Some(n) = v.as_i64() {
        if let Some(min) = s.get("minimum").and_then(Value::as_i64)
            && n < min
        {
            return Err(format!("{at}: {n} is below {min}"));
        }
        if let Some(max) = s.get("maximum").and_then(Value::as_i64)
            && n > max
        {
            return Err(format!("{at}: {n} is above {max}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// `SwimlaneDetail`
// ---------------------------------------------------------------------------------------------------------------------

/// `SwimlaneDetail` as it was before this fix, built from the live `SwimlaneSummary` and the original closed extension.
fn closed_allof_detail() -> Value {
    json!({
        "allOf": [
            {"$ref": "#/components/schemas/SwimlaneSummary"},
            {
                "type": "object",
                "properties": {
                    "services": {
                        "type": "array",
                        "items": {"$ref": "#/components/schemas/ServiceState"},
                        "maxItems": 500
                    },
                    "auto_notify": {"type": "boolean"}
                },
                "additionalProperties": false,
                "required": ["services", "auto_notify"]
            }
        ]
    })
}

fn full_summary() -> SwimlaneSummary {
    serde_json::from_value(json!({
        "id": "sit1", "display_name": "SIT 1", "tenants": ["sit1"], "agent": "connected", "last_scan_at": 1_700_000_000_000_i64,
        "base_version": {"tag": "v1.2.0", "distance": 3, "commit": "0123456789abcdef0123456789abcdef01234567"},
        "drift_counts": [["nfs_ahead", 2]], "finding_count": 4,
        "tier": "SIT", "project": "Core Banking",
        "baseline": {"state": "confirmed", "by": {"id": ports::conformance::sample::user_id(1), "display_name": "Priya"},
                     "at": 1_700_000_000_000_i64},
        "pickup_counts": [["needs_notify_or_restart", 2]],
        "syncing": {"job": {"name": "sync-job-1", "uid": "uid-1"}, "since": 1_700_000_001_000_i64}
    }))
    .unwrap()
}

fn old_summary() -> SwimlaneSummary {
    serde_json::from_value(json!({
        "id": "sit1", "display_name": "SIT 1", "tenants": ["sit1"], "agent": "connected", "last_scan_at": null,
        "base_version": null, "drift_counts": [["nfs_ahead", 2]], "finding_count": 4
    }))
    .unwrap()
}

/// The body of `getSwimlane` as the registry builds it: the summary's keys and `services` and `auto_notify`.
fn detail_body(summary: &SwimlaneSummary) -> Value {
    let service = ServiceState {
        service: ServiceRef::new("csp", "tx-infinity-api").unwrap(),
        pickup: PickupState::NeedsNotifyOrRestart,
        pending_since: None,
        live_by: None,
        notifications_enabled: true,
        cache_ttl_secs: 1200,
        pending_files: Vec::new(),
    };
    let mut body = serde_json::to_value(summary).unwrap();
    body["services"] = json!([service]);
    body["auto_notify"] = json!(true);
    body
}

/// The body that never validated: with the closed `allOf`, the walker refuses what the hub sends. This is the red of the
/// fix, kept as a test so that the reason for the flat schema stays on record.
#[test]
fn a_real_detail_body_never_validated_against_the_closed_allof() {
    let d = doc();
    let old = closed_allof_detail();
    for summary in [full_summary(), old_summary()] {
        let body = detail_body(&summary);
        let err =
            check(&d, &old, &body, "$").expect_err("a closed allOf cannot hold the keys of its sibling");
        assert!(err.contains("additionalProperties: false"), "{err}");
    }
    // Opening only the extension, as CG13 did for the problems, is not enough: `SwimlaneSummary` is closed too.
    let mut half = old.clone();
    half["allOf"][1]
        .as_object_mut()
        .unwrap()
        .remove("additionalProperties");
    let err = check(&d, &half, &detail_body(&full_summary()), "$").expect_err("the summary is closed");
    assert!(
        err.contains("`auto_notify` is not allowed") || err.contains("`services` is not allowed"),
        "{err}"
    );
}

#[test]
fn swimlane_detail_accepts_what_the_hub_sends_and_nothing_else() {
    let d = doc();
    let detail = &d["components"]["schemas"]["SwimlaneDetail"];
    for (label, summary) in [
        ("with the new fields", full_summary()),
        ("as an old server sends it", old_summary()),
    ] {
        let body = detail_body(&summary);
        check(&d, detail, &body, "$").unwrap_or_else(|e| panic!("a detail {label} is refused: {e}"));
    }
    let body = detail_body(&full_summary());

    let mut without_services = body.clone();
    without_services.as_object_mut().unwrap().remove("services");
    assert!(
        check(&d, detail, &without_services, "$").is_err(),
        "`services` stays required"
    );

    let mut without_auto_notify = body.clone();
    without_auto_notify.as_object_mut().unwrap().remove("auto_notify");
    assert!(
        check(&d, detail, &without_auto_notify, "$").is_err(),
        "`auto_notify` stays required"
    );

    let mut extra = body.clone();
    extra["undeclared"] = json!(1);
    assert!(check(&d, detail, &extra, "$").is_err(), "the detail stays closed");

    let mut too_many = body;
    too_many["services"] = Value::Array(vec![too_many["services"][0].clone(); 501]);
    assert!(
        check(&d, detail, &too_many, "$").is_err(),
        "`services` keeps its bound of 500"
    );
}

/// The detail is one closed object: the summary's properties, each defined exactly as the summary defines it, and the two more.
/// The copies cannot drift: a property added to the summary and not to the detail fails here, and so does a changed one.
#[test]
fn swimlane_detail_is_the_summary_and_two_more_fields_in_one_closed_object() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let summary = &schemas["SwimlaneSummary"];
    let detail = &schemas["SwimlaneDetail"];

    assert!(
        detail.get("allOf").is_none(),
        "an allOf of the closed summary cannot validate a real body"
    );
    assert_eq!(detail["type"], "object");
    assert_eq!(detail["additionalProperties"], false);
    assert!(
        detail["description"]
            .as_str()
            .is_some_and(|t| t.contains("allOf")),
        "the description says why the summary is not composed with allOf"
    );

    let summary_props = summary["properties"].as_object().unwrap();
    let detail_props = detail["properties"].as_object().unwrap();
    for (name, def) in summary_props {
        assert_eq!(
            detail_props.get(name),
            Some(def),
            "SwimlaneDetail.{name} is defined as SwimlaneSummary.{name} is"
        );
    }
    let extra: BTreeSet<String> = detail_props
        .keys()
        .filter(|k| !summary_props.contains_key(*k))
        .cloned()
        .collect();
    assert_eq!(
        extra,
        BTreeSet::from(["auto_notify".to_owned(), "services".to_owned()])
    );
    assert_eq!(
        detail_props["services"]["items"]["$ref"],
        "#/components/schemas/ServiceState"
    );
    assert_eq!(detail_props["services"]["maxItems"], 500);
    assert_eq!(detail_props["auto_notify"]["type"], "boolean");

    let mut required = summary.get("required").map(strings).unwrap_or_default();
    required.extend(["services".to_owned(), "auto_notify".to_owned()]);
    assert_eq!(detail.get("required").map(strings).unwrap_or_default(), required);

    // What the hub writes for a detail is the summary's keys and the two more, and nothing else.
    let keys: BTreeSet<String> = detail_body(&full_summary())
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, detail_props.keys().cloned().collect::<BTreeSet<_>>());
}

// ---------------------------------------------------------------------------------------------------------------------
// `Me`, `SeverityRule` and `RetentionSettings`
// ---------------------------------------------------------------------------------------------------------------------

/// `Me` as it was: `User` (closed) and a closed extension.
fn closed_allof_me() -> Value {
    json!({
        "allOf": [
            {"$ref": "#/components/schemas/User"},
            {
                "type": "object",
                "properties": {
                    "swimlane_access": {"type": "string", "enum": ["all"]},
                    "needs_github_token": {"type": "boolean"}
                },
                "additionalProperties": false
            }
        ]
    })
}

/// `SeverityRule` as it was: `{id}` (closed) and `SeverityRuleInput` (closed).
fn closed_allof_severity_rule() -> Value {
    json!({
        "allOf": [
            {
                "type": "object",
                "properties": {"id": {"$ref": "#/components/schemas/ConfigKey"}},
                "additionalProperties": false,
                "required": ["id"]
            },
            {"$ref": "#/components/schemas/SeverityRuleInput"}
        ]
    })
}

/// `RetentionSettings` as it was: `RetentionInput` (closed) and a closed extension.
fn closed_allof_retention() -> Value {
    json!({
        "allOf": [
            {"$ref": "#/components/schemas/RetentionInput"},
            {
                "type": "object",
                "properties": {
                    "updated_at": {"oneOf": [{"$ref": "#/components/schemas/Timestamp"}, {"type": "null"}]},
                    "updated_by": {"oneOf": [{"$ref": "#/components/schemas/UserId"}, {"type": "null"}]},
                    "dry_run_preview": {"oneOf": [{"$ref": "#/components/schemas/RetentionPreview"}, {"type": "null"}]}
                },
                "additionalProperties": false,
                "required": ["updated_at", "updated_by", "dry_run_preview"]
            }
        ]
    })
}

/// A flat schema is one closed object: no `allOf`, `additionalProperties: false`, a description that says why it repeats its
/// members, every property of every member defined exactly as the member defines it, exactly `extras` more, and the members'
/// required keys plus `extra_required`. Returns the properties of the flat schema.
fn assert_flat<'a>(
    d: &'a Value,
    name: &str,
    members: &[&str],
    extras: &[&str],
    extra_required: &[&str],
) -> &'a serde_json::Map<String, Value> {
    let schemas = &d["components"]["schemas"];
    let flat = &schemas[name];
    assert!(
        flat.get("allOf").is_none(),
        "{name}: an allOf of a closed member cannot validate a real body"
    );
    assert_eq!(flat["type"], "object", "{name}");
    assert_eq!(flat["additionalProperties"], false, "{name} stays closed");
    assert!(
        flat["description"].as_str().is_some_and(|t| t.contains("allOf")),
        "{name}: the description says why its members are repeated and not composed with allOf"
    );
    let props = flat["properties"].as_object().unwrap();
    let mut own: BTreeSet<String> = BTreeSet::new();
    let mut required = BTreeSet::new();
    for member in members {
        let member_schema = &schemas[*member];
        // The member stays what it was: closed, so that it is still the request body or the shape of a bare user.
        assert_eq!(
            member_schema["additionalProperties"], false,
            "{member} stays closed"
        );
        for (key, def) in member_schema["properties"].as_object().unwrap() {
            assert_eq!(
                props.get(key),
                Some(def),
                "{name}.{key} is defined as {member}.{key} is"
            );
            own.insert(key.clone());
        }
        required.extend(member_schema.get("required").map(strings).unwrap_or_default());
    }
    let more: BTreeSet<String> = props.keys().filter(|k| !own.contains(*k)).cloned().collect();
    assert_eq!(
        more,
        extras.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>(),
        "{name}: the properties that are not a member's"
    );
    required.extend(extra_required.iter().map(|s| (*s).to_owned()));
    assert_eq!(
        flat.get("required").map(strings).unwrap_or_default(),
        required,
        "{name}: the required keys of the members, and the extra ones, and no more"
    );
    props
}

/// The body of `getMe` as `hub-identity` builds it (`routes/me.rs`): the real `domain::User`, serialised, and the two more keys.
fn me_body(user: &User, needs_github_token: bool) -> Value {
    let mut body = serde_json::to_value(user).unwrap();
    body["swimlane_access"] = json!("all");
    body["needs_github_token"] = json!(needs_github_token);
    body
}

fn editor_with_everything() -> User {
    User {
        github_login: Some(ShortText::parse("octocat").unwrap()),
        last_seen_at: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
        ..user(1, Some(Role::Editor), UserStatus::Active)
    }
}

#[test]
fn a_real_me_body_never_validated_against_the_closed_allof() {
    let d = doc();
    let old = closed_allof_me();
    for u in [
        editor_with_everything(),
        user(2, Some(Role::Viewer), UserStatus::Active),
    ] {
        let err = check(&d, &old, &me_body(&u, true), "$")
            .expect_err("a closed allOf cannot hold the keys of its sibling");
        assert!(err.contains("additionalProperties: false"), "{err}");
    }
    // Not even the bare user: the closed extension rejects every key of `User`.
    let err = check(
        &d,
        &old,
        &serde_json::to_value(editor_with_everything()).unwrap(),
        "$",
    )
    .expect_err("the extension is closed");
    assert!(err.contains("is not allowed"), "{err}");
}

#[test]
fn me_accepts_what_the_hub_sends_and_nothing_else() {
    let d = doc();
    let me = &d["components"]["schemas"]["Me"];
    // Every user the hub can write: a role or none, with and without a GitHub login and a last-seen time, either flag.
    for u in [
        editor_with_everything(),
        user(2, Some(Role::Viewer), UserStatus::Active),
        user(3, None, UserStatus::Pending),
        user(4, Some(Role::Admin), UserStatus::Disabled),
    ] {
        for flag in [true, false] {
            check(&d, me, &me_body(&u, flag), "$")
                .unwrap_or_else(|e| panic!("`/me` of {u:?} is refused: {e}"));
        }
    }
    // An older server sends the user and neither extra key; both are optional, and each stands alone.
    let bare = serde_json::to_value(editor_with_everything()).unwrap();
    check(&d, me, &bare, "$").expect("a body of a server that predates both keys");
    let mut only_flag = bare.clone();
    only_flag["needs_github_token"] = json!(false);
    check(&d, me, &only_flag, "$").expect("needs_github_token alone");
    let mut only_access = bare;
    only_access["swimlane_access"] = json!("all");
    check(&d, me, &only_access, "$").expect("swimlane_access alone");

    let body = me_body(&editor_with_everything(), true);
    let mut extra = body.clone();
    extra["undeclared"] = json!(1);
    assert!(check(&d, me, &extra, "$").is_err(), "`Me` stays closed");
    for key in [
        "id",
        "email",
        "display_name",
        "role",
        "status",
        "requires_approval",
        "github_login",
        "last_seen_at",
    ] {
        let mut without = body.clone();
        without.as_object_mut().unwrap().remove(key);
        assert!(
            check(&d, me, &without, "$").is_err(),
            "`{key}` of `User` stays required"
        );
    }
    let mut other_access = body;
    other_access["swimlane_access"] = json!("some");
    assert!(
        check(&d, me, &other_access, "$").is_err(),
        "swimlane_access is `all`"
    );
}

#[test]
fn me_is_the_user_and_two_optional_fields_in_one_closed_object() {
    let d = doc();
    let props = assert_flat(
        &d,
        "Me",
        &["User"],
        &["needs_github_token", "swimlane_access"],
        &[],
    );
    assert_eq!(props["needs_github_token"]["type"], "boolean");
    assert_eq!(props["swimlane_access"]["type"], "string");
    assert_eq!(props["swimlane_access"]["enum"], json!(["all"]));
    // What the hub writes is the user's keys and the two more, and nothing else.
    let keys: BTreeSet<String> = me_body(&editor_with_everything(), true)
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, props.keys().cloned().collect::<BTreeSet<_>>());
    // The two new keys are optional: an old body and an old mock stay valid (`me_has_the_additive_needs_github_token_flag`).
    let required = strings(&d["components"]["schemas"]["Me"]["required"]);
    assert!(!required.contains("needs_github_token") && !required.contains("swimlane_access"));
}

/// The body of `adminPutSeverityRules` (200) and the items of `adminListSeverityRules`: the rule and its id. The enums are the
/// domain's, so the wire names are the ones the engine writes.
fn severity_rule_body() -> Value {
    json!({
        "id": "prod-hosts-critical",
        "name": "Production hosts",
        "match": {
            "finding_kinds": [serde_json::to_value(FindingKind::EnvironmentHostMismatch).unwrap()],
            "change_kinds": [serde_json::to_value(ChangeKind::Changed).unwrap()],
            "path_glob": "**/*.yml",
            "setting_glob": null
        },
        "severity": serde_json::to_value(Severity::Critical).unwrap(),
        "priority": 100,
        "enabled": true
    })
}

#[test]
fn a_real_severity_rule_body_never_validated_against_the_closed_allof() {
    let d = doc();
    let err = check(&d, &closed_allof_severity_rule(), &severity_rule_body(), "$")
        .expect_err("a closed allOf cannot hold the keys of its sibling");
    assert!(err.contains("additionalProperties: false"), "{err}");
}

#[test]
fn severity_rule_accepts_what_the_admin_routes_send_and_nothing_else() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let rule = &schemas["SeverityRule"];
    let body = severity_rule_body();
    check(&d, rule, &body, "$").unwrap_or_else(|e| panic!("a severity rule is refused: {e}"));
    // A rule with no conditions at all, and every severity.
    let mut open = body.clone();
    open["match"] = json!({});
    check(&d, rule, &open, "$").expect("a match has no required key");
    for sev in Severity::ALL {
        let mut b = body.clone();
        b["severity"] = serde_json::to_value(sev).unwrap();
        check(&d, rule, &b, "$").unwrap_or_else(|e| panic!("{sev:?}: {e}"));
    }
    // The page that lists them.
    let page = json!({"items": [body.clone(), open], "next": null});
    check(&d, &schemas["SeverityRulePage"], &page, "$")
        .unwrap_or_else(|e| panic!("the list page is refused: {e}"));

    let mut extra = body.clone();
    extra["undeclared"] = json!(1);
    assert!(
        check(&d, rule, &extra, "$").is_err(),
        "`SeverityRule` stays closed"
    );
    for key in ["id", "name", "match", "severity", "priority", "enabled"] {
        let mut without = body.clone();
        without.as_object_mut().unwrap().remove(key);
        assert!(check(&d, rule, &without, "$").is_err(), "`{key}` stays required");
    }
    let mut high = body.clone();
    high["priority"] = json!(10_001);
    assert!(
        check(&d, rule, &high, "$").is_err(),
        "`priority` keeps its bound of 10,000"
    );
    let mut long = body;
    long["name"] = json!("x".repeat(257));
    assert!(
        check(&d, rule, &long, "$").is_err(),
        "`name` keeps its bound of 256 characters"
    );
}

#[test]
fn severity_rule_is_the_input_and_an_id_in_one_closed_object() {
    let d = doc();
    let props = assert_flat(&d, "SeverityRule", &["SeverityRuleInput"], &["id"], &["id"]);
    assert_eq!(props["id"]["$ref"], "#/components/schemas/ConfigKey");
    let keys: BTreeSet<String> = severity_rule_body()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, props.keys().cloned().collect::<BTreeSet<_>>());
}

/// The body of `adminGetRetention` and of `adminPutRetention`: the settings and when and by whom they changed. `dry_run_preview`
/// is the numbers of `?dry_run=true` and null otherwise, and the two `updated_` keys are null for settings nobody has changed.
fn retention_body(preview: bool, changed: bool) -> Value {
    json!({
        "keep_last_versions": 50,
        "keep_days": 180,
        "updated_at": if changed { json!(Timestamp::from_unix_millis(1_700_000_000_000)) } else { Value::Null },
        "updated_by": if changed { json!(user_id(1)) } else { Value::Null },
        "dry_run_preview": if preview {
            json!({"versions_to_delete": 12, "bytes_to_free": 4096, "versions_kept_by_reference": 3})
        } else {
            Value::Null
        }
    })
}

#[test]
fn a_real_retention_body_never_validated_against_the_closed_allof() {
    let d = doc();
    let err = check(&d, &closed_allof_retention(), &retention_body(false, true), "$")
        .expect_err("a closed allOf cannot hold the keys of its sibling");
    assert!(err.contains("additionalProperties: false"), "{err}");
}

#[test]
fn retention_settings_accepts_what_the_admin_routes_send_and_nothing_else() {
    let d = doc();
    let retention = &d["components"]["schemas"]["RetentionSettings"];
    for (preview, changed) in [(false, true), (true, true), (false, false), (true, false)] {
        check(&d, retention, &retention_body(preview, changed), "$")
            .unwrap_or_else(|e| panic!("preview {preview}, changed {changed}: {e}"));
    }
    let body = retention_body(true, true);
    let mut extra = body.clone();
    extra["undeclared"] = json!(1);
    assert!(
        check(&d, retention, &extra, "$").is_err(),
        "`RetentionSettings` stays closed"
    );
    for key in [
        "keep_last_versions",
        "keep_days",
        "updated_at",
        "updated_by",
        "dry_run_preview",
    ] {
        let mut without = body.clone();
        without.as_object_mut().unwrap().remove(key);
        assert!(
            check(&d, retention, &without, "$").is_err(),
            "`{key}` stays required"
        );
    }
    for (key, value) in [
        ("keep_last_versions", 10_001),
        ("keep_last_versions", 0),
        ("keep_days", 3651),
        ("keep_days", 0),
    ] {
        let mut b = body.clone();
        b[key] = json!(value);
        assert!(
            check(&d, retention, &b, "$").is_err(),
            "`{key}` keeps its bounds: {value}"
        );
    }
    let mut negative = body;
    negative["dry_run_preview"]["bytes_to_free"] = json!(-1);
    assert!(
        check(&d, retention, &negative, "$").is_err(),
        "the preview keeps its bounds"
    );
}

#[test]
fn retention_settings_is_the_input_and_three_more_fields_in_one_closed_object() {
    let d = doc();
    let props = assert_flat(
        &d,
        "RetentionSettings",
        &["RetentionInput"],
        &["dry_run_preview", "updated_at", "updated_by"],
        &["dry_run_preview", "updated_at", "updated_by"],
    );
    let keys: BTreeSet<String> = retention_body(true, true)
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, props.keys().cloned().collect::<BTreeSet<_>>());
}
