//! contract/08-web: an `allOf` whose closed member rejects the properties of its sibling can never validate a real body.
//!
//! `additionalProperties: false` looks at the `properties` of the schema that carries it and at nothing else, not at the
//! siblings of an `allOf`. So `allOf: [A, B]` with `A` closed rejects every key that only `B` declares, and a body that carries
//! one can never be valid. CG13 found it on `ConflictProblem` and `LockedProblem` (a closed extension next to the open
//! `Problem`); `SwimlaneDetail` had it too, and worse: both of its members were closed, `SwimlaneSummary` and the extension
//! with `services` and `auto_notify`, so not even the body of an old server validated.
//!
//! This file scans the whole document (components, paths, responses, inline schemas) for the pattern, with an explicit list
//! of the offenders that were already in the document and are not fixed here, and holds `SwimlaneDetail` to what the hub
//! sends: the summary and the two more fields, in one closed object, with the summary's definitions repeated and held equal.
//! Nothing here changes what was valid before: no body ever validated against the closed `allOf`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use domain::{PickupState, ServiceRef, ServiceState, SwimlaneSummary};
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

/// The schemas that had this defect before this PR and still have it. Known debt: not fixed here, because each is an existing
/// operation's schema that this PR has no brief to change, and the human has not decided how (a flat object, as
/// `SwimlaneDetail` is now, or open members). Found by this scan on `origin/main`; each one never validated a real body.
///   `Me` (`getMe`): `allOf [User, {needs_github_token, swimlane_access}]`, `User` and the extension both closed.
///   `SeverityRule` (the severity rule routes): `allOf [{id}, SeverityRuleInput]`, both closed.
///   `RetentionSettings` (the retention routes): `allOf [RetentionInput, {dry_run_preview, updated_at, updated_by}]`, both closed.
/// An entry is removed when its schema is fixed, and the test below fails until it is.
const KNOWN_DEBT: [&str; 3] = [
    "#/components/schemas/Me",
    "#/components/schemas/RetentionSettings",
    "#/components/schemas/SeverityRule",
];

fn locations(found: &BTreeSet<String>) -> BTreeSet<String> {
    found
        .iter()
        .map(|f| f.split(": ").next().unwrap().to_owned())
        .collect()
}

#[test]
fn no_allof_closes_out_its_siblings() {
    let found = locations(&offenders(&doc()));
    let debt: BTreeSet<String> = KNOWN_DEBT.iter().map(|s| (*s).to_owned()).collect();
    let new: Vec<_> = found.difference(&debt).collect();
    assert!(
        new.is_empty(),
        "an allOf has a closed member that rejects what its sibling declares, so no real body validates: {new:#?}\n\
         (compose the schema as one closed object that repeats the fields, or open the members; `SwimlaneDetail` is the example)"
    );
}

#[test]
fn the_known_debt_is_still_debt() {
    let found = locations(&offenders(&doc()));
    for entry in KNOWN_DEBT {
        assert!(
            found.contains(entry),
            "{entry} no longer has the defect: remove it from KNOWN_DEBT"
        );
    }
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
