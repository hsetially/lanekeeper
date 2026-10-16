//! contract/08-web: the two checks that cover the whole PR, not one item.
//!
//! The plan's traceability names `new_operations_name_their_audit_action_or_say_why_not` ("a table of every operationId added
//! here") and `new_schemas_are_bounded` ("every array in a schema added here has `maxItems`, every string `maxLength` or a
//! `pattern`"). The item files each carry a version scoped to what that item adds. These are the global ones: every
//! operation, schema, response, header and added property of the twelve commits, listed once, against the document.
//!
//! The lists are the difference between `api/openapi.yaml` and `origin/main` (777b590), worked out mechanically when the
//! slices were integrated: 8 operations, 39 schemas, 3 responses and 1 header, and the properties added to 11 existing
//! schemas (a value added to the `EventKind` enum and the descriptions of a few others add no property). A later change to the contract adds its own rows, which is the point of keeping them in one place: a new
//! operation names its audit action (rule 8, S9), and a new schema is bounded (S11).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;

const SPEC: &str = include_str!("../../../api/openapi.yaml");

fn doc() -> Value {
    serde_saphyr::from_str(SPEC).expect("api/openapi.yaml is valid YAML")
}

/// The operation with this id: its method, path and definition.
fn operation<'a>(d: &'a Value, id: &str) -> Option<(String, String, &'a Value)> {
    for (path, item) in d["paths"].as_object()? {
        for m in ["get", "put", "post", "delete", "patch"] {
            if let Some(op) = item.get(m)
                && op["operationId"] == id
            {
                return Some((m.to_uppercase(), path.clone(), op));
            }
        }
    }
    None
}

fn resolve<'a>(d: &'a Value, v: &'a Value) -> &'a Value {
    let mut cur = v;
    for _ in 0..16 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
            return cur;
        };
        let pointer = r.strip_prefix('#').unwrap_or_else(|| panic!("external $ref {r}"));
        cur = d.pointer(pointer).unwrap_or_else(|| panic!("dangling $ref {r}"));
    }
    panic!("$ref chain too deep");
}

// ---------------------------------------------------------------------------------------------------------------------
// What the twelve commits add
// ---------------------------------------------------------------------------------------------------------------------

/// `(operationId, method, the audit action, or why there is none)`. Every row is state-free: a read, or the one POST that
/// previews a pull request and writes nothing (the `previewFileEffect` precedent). A state-changing operation would name its
/// `AuditAction` here instead, and the operations of the deferred items (adoption, admin swimlanes) add theirs when they come.
const NEW_OPERATIONS: [(&str, &str, &str); 8] = [
    ("previewPr", "POST", "none: no state change"),
    ("getConfigServerRestartOffer", "GET", "none: a read"),
    ("getRestartOffer", "GET", "none: a read"),
    ("getProposal", "GET", "none: a read"),
    ("getDraft", "GET", "none: a read"),
    ("getGithubCredential", "GET", "none: a read"),
    ("listGitRefs", "GET", "none: a read"),
    ("renderDoc", "GET", "none: a read"),
];

/// The 39 schemas of `components.schemas` that `origin/main` does not have: the event payloads (`CG1a`, `CG1b`), the problem
/// vocabulary (CG13), the summary additions (`CG4a`), the pull request preview (CG3), the restart offers (CG5), the proposal
/// vocabulary (CG12), the Git refs (CG10) and the rendered doc (CG11).
const NEW_SCHEMAS: [&str; 39] = [
    "BaselineState",
    "BaselineStatus",
    "BranchKind",
    "ConfigServerRestartOffer",
    "ConfigServerRestartOfferResult",
    "DocHeading",
    "DomainEvent",
    "DomainEventAccessRequestCreated",
    "DomainEventAgentStatusChanged",
    "DomainEventAttributionUpdated",
    "DomainEventDocIndexed",
    "DomainEventDriftChanged",
    "DomainEventFileObserved",
    "DomainEventFindingsChanged",
    "DomainEventGitHeadMoved",
    "DomainEventPendingRestartChanged",
    "DomainEventPickupChanged",
    "DomainEventPrJobProgress",
    "DomainEventPrStateChanged",
    "DomainEventProposalChanged",
    "DomainEventResync",
    "DomainEventSwimlaneChanged",
    "DomainEventSyncWindowClosed",
    "DomainEventSyncWindowOpened",
    "DuplicatePrProblem",
    "GitRefEntry",
    "GitRefKind",
    "GitRefPage",
    "JobRef",
    "PrPreview",
    "PrPreviewFile",
    "PrPreviewRequest",
    "ProposalAction",
    "ProposalVia",
    "RenderedDoc",
    "RestartOffer",
    "SyncingInfo",
    "TokenMissingAccessProblem",
    "UserRef",
];

/// Schemas of the list above that are open or composed on purpose, so they carry no `additionalProperties: false` of their own.
///   `DomainEvent`: a `oneOf` of the closed payloads, not an object.
///   `TokenMissingAccessProblem`: an `allOf` of the open `Problem` and an open extension, so that a real body validates.
const NOT_CLOSED_OBJECTS: [&str; 2] = ["DomainEvent", "TokenMissingAccessProblem"];

/// The properties the commits add to schemas that `origin/main` already has (every one optional).
const NEW_PROPERTIES: [(&str, &[&str]); 11] = [
    ("Problem", &["code"]),
    ("FileEntry", &["logical_file", "tenant"]),
    ("SettingHit", &["logical_file", "tenant"]),
    ("TextHit", &["logical_file", "tenant"]),
    ("Finding", &["logical_file", "tenant"]),
    (
        "SwimlaneSummary",
        &["baseline", "pickup_counts", "project", "syncing", "tier"],
    ),
    ("Proposal", &["action", "decided_at", "paths", "via"]),
    ("GithubCredential", &["expires_at", "expires_soon"]),
    ("DocHit", &["anchor"]),
    ("FlagStatus", &["anchor"]),
    // `SwimlaneDetail` repeats the summary's properties since `web_contract_allof.rs`, so all of them are checked.
    (
        "SwimlaneDetail",
        &[
            "agent",
            "auto_notify",
            "base_version",
            "baseline",
            "display_name",
            "drift_counts",
            "finding_count",
            "id",
            "last_scan_at",
            "pickup_counts",
            "project",
            "services",
            "syncing",
            "tenants",
            "tier",
        ],
    ),
];

/// The responses and the header the commits add to `components`.
const NEW_RESPONSES: [&str; 3] = ["ConflictState", "ServiceUnavailable", "TooManyRequests"];
const NEW_HEADERS: [&str; 1] = ["RetryAfter"];

// ---------------------------------------------------------------------------------------------------------------------
// Check 1: every new operation names its audit action, or says why it has none
// ---------------------------------------------------------------------------------------------------------------------

/// What is wrong with the audit stance of the new operations, empty when nothing is.
fn audit_gaps(d: &Value) -> Vec<String> {
    let mut gaps = Vec::new();
    for (id, method, audit) in NEW_OPERATIONS {
        let Some((found_method, path, op)) = operation(d, id) else {
            gaps.push(format!("{id} is not in the document"));
            continue;
        };
        if found_method != method {
            gaps.push(format!("{id} is a {found_method}, the table says {method}"));
        }
        if !audit.starts_with("none: ") {
            // A state change names the action, which has to be in the description for the owner to find it.
            if !op["description"].as_str().unwrap_or("").contains(audit) {
                gaps.push(format!(
                    "{id} writes {audit} but its description does not name it"
                ));
            }
            continue;
        }
        if found_method == "GET" {
            continue;
        }
        // A non-GET with no audit event must say that nothing is written or audited (rule 8 does not apply, and a reader of
        // the document must not assume a write).
        let said = op["description"].as_str().unwrap_or("").to_lowercase();
        if !(said.contains("nothing is written") && said.contains("audited")) {
            gaps.push(format!(
                "{id} ({found_method} {path}) has no audit event and does not say that nothing is written or audited"
            ));
        }
    }
    gaps
}

#[test]
fn new_operations_name_their_audit_action_or_say_why_not() {
    let gaps = audit_gaps(&doc());
    assert!(gaps.is_empty(), "{gaps:#?}");
}

/// The table has teeth: a non-GET that stops saying that it writes nothing, an operation that is not there, and a method that
/// changed are all found.
#[test]
fn the_audit_check_finds_a_missing_stance() {
    let mut d = doc();
    let (_, path, _) = operation(&d, "previewPr").unwrap();
    d["paths"][path.as_str()]["post"]["description"] = Value::String("Previews a pull request.".into());
    let gaps = audit_gaps(&d);
    assert!(gaps.iter().any(|g| g.starts_with("previewPr")), "{gaps:#?}");

    let mut d = doc();
    d["paths"]["/api/v1/git/refs"]["get"]["operationId"] = Value::String("listRefs".into());
    let gaps = audit_gaps(&d);
    assert!(
        gaps.iter()
            .any(|g| g.contains("listGitRefs is not in the document")),
        "{gaps:#?}"
    );

    let mut d = doc();
    let item = d["paths"]["/api/v1/drafts/{id}"].as_object_mut().unwrap();
    let get = item.remove("get").unwrap();
    item.insert("post".into(), get);
    let gaps = audit_gaps(&d);
    assert!(gaps.iter().any(|g| g.starts_with("getDraft")), "{gaps:#?}");
}

// ---------------------------------------------------------------------------------------------------------------------
// Check 2: every new schema is bounded
// ---------------------------------------------------------------------------------------------------------------------

/// Every string of a schema's own tree has a bound and every array a `maxItems` (S11). A `$ref` is bounded where it is
/// defined, a constant and an enum need no bound, and a typed `format: binary` is a stream, not a string.
fn unbounded(at: &str, s: &Value, out: &mut Vec<String>) {
    let Some(obj) = s.as_object() else { return };
    match obj.get("type").and_then(Value::as_str) {
        Some("string")
            if !["maxLength", "pattern", "enum", "const"]
                .iter()
                .any(|k| obj.contains_key(*k))
                && obj.get("format").is_none_or(|f| f != "binary") =>
        {
            out.push(format!("{at} is a string without maxLength or pattern"));
        }
        Some("array") if !obj.contains_key("maxItems") => {
            out.push(format!("{at} is an array without maxItems"));
        }
        _ => {}
    }
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (k, v) in props {
            unbounded(&format!("{at}.{k}"), v, out);
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(v) = obj.get(key) {
            unbounded(&format!("{at}.{key}"), v, out);
        }
    }
    for key in ["oneOf", "anyOf", "allOf", "prefixItems"] {
        for (i, v) in obj
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            unbounded(&format!("{at}.{key}[{i}]"), v, out);
        }
    }
}

/// What is unbounded or open in what the commits add, empty when nothing is.
fn bound_gaps(d: &Value) -> Vec<String> {
    let mut gaps = Vec::new();
    let schemas = &d["components"]["schemas"];

    for name in NEW_SCHEMAS {
        let Some(schema) = schemas.get(name) else {
            gaps.push(format!("schema {name} is not in the document"));
            continue;
        };
        unbounded(name, schema, &mut gaps);
        // Every new object is closed, so that a key the document does not list is a bug of the server, not a surprise to the
        // client. The two that are not objects, or are composed on purpose, are named above.
        if !NOT_CLOSED_OBJECTS.contains(&name)
            && schema["additionalProperties"] != false
            && schema["type"] == "object"
        {
            gaps.push(format!("{name} is an object that is not closed"));
        }
    }

    for (name, props) in NEW_PROPERTIES {
        let Some(schema) = schemas.get(name) else {
            gaps.push(format!("schema {name} is not in the document"));
            continue;
        };
        for prop in props {
            match schema["properties"].get(*prop) {
                Some(p) => unbounded(&format!("{name}.{prop}"), p, &mut gaps),
                None => gaps.push(format!("{name}.{prop} is not in the document")),
            }
            // Optional, so that an old client, an old mock and an old server stay valid. `SwimlaneDetail` is a whole schema
            // with its own `required` list, which `web_contract_allof.rs` pins.
            if name != "SwimlaneDetail"
                && schema["required"]
                    .as_array()
                    .is_some_and(|r| r.iter().any(|x| x == prop))
            {
                gaps.push(format!(
                    "{name}.{prop} is required, so an old body would be invalid"
                ));
            }
        }
        if schema["type"] == "object" && schema["additionalProperties"] != false && name != "Problem" {
            gaps.push(format!("{name} is not closed"));
        }
    }

    // The bounds of what a new operation takes: its own parameters and the body of its request.
    for (id, _, _) in NEW_OPERATIONS {
        let Some((_, _, op)) = operation(d, id) else {
            continue;
        };
        for p in op["parameters"].as_array().into_iter().flatten() {
            let p = resolve(d, p);
            if p["schema"].get("$ref").is_none() {
                unbounded(
                    &format!("{id}.{}", p["name"].as_str().unwrap_or("?")),
                    &p["schema"],
                    &mut gaps,
                );
            }
        }
        if let Some(body) = op["requestBody"]["content"]["application/json"].get("schema") {
            match body.get("$ref").and_then(Value::as_str) {
                Some(r) => {
                    let name = r.rsplit('/').next().unwrap_or("");
                    if !NEW_SCHEMAS.contains(&name) {
                        gaps.push(format!(
                            "{id}: the request body {name} is not in the list of new schemas"
                        ));
                    }
                }
                None => unbounded(&format!("{id}.requestBody"), body, &mut gaps),
            }
        }
    }

    // A throttle and an outage say how long to wait, in a number with a ceiling.
    for name in NEW_HEADERS {
        let header = &d["components"]["headers"][name];
        let s = &header["schema"];
        if s["type"] != "integer" || !s["maximum"].is_u64() {
            gaps.push(format!("header {name} is not an integer with a maximum"));
        }
    }
    for name in NEW_RESPONSES {
        let Some(r) = d["components"]["responses"].get(name) else {
            gaps.push(format!("response {name} is not in the document"));
            continue;
        };
        for (hname, h) in r["headers"].as_object().into_iter().flatten() {
            let s = resolve(d, &resolve(d, h)["schema"]);
            if s["type"] != "integer" || !s["maximum"].is_u64() {
                gaps.push(format!("{name}: header {hname} is not an integer with a maximum"));
            }
        }
        if r["content"]
            .as_object()
            .is_none_or(|c| c.keys().ne(["application/problem+json"]))
        {
            gaps.push(format!(
                "{name} does not answer with application/problem+json only"
            ));
        }
    }
    gaps
}

#[test]
fn new_schemas_are_bounded() {
    let gaps = bound_gaps(&doc());
    assert!(gaps.is_empty(), "{gaps:#?}");
}

/// The check has teeth in every slice: a bound removed from a schema of each item is found, and so is a schema that opens.
#[test]
fn the_bound_check_finds_a_missing_bound_in_each_item() {
    // (a pointer to the bound to remove, the key to remove, a word the report must carry)
    for (pointer, key, said) in [
        // CG1a: the payload of an event.
        (
            "/components/schemas/DomainEventPickupChanged/properties/services",
            "maxItems",
            "DomainEventPickupChanged.services",
        ),
        // CG13: the problem for a duplicate PR.
        (
            "/components/schemas/DuplicatePrProblem/properties/title",
            "maxLength",
            "DuplicatePrProblem.title",
        ),
        // CG3: the preview.
        (
            "/components/schemas/PrPreview/properties/warnings",
            "maxItems",
            "PrPreview.warnings",
        ),
        // CG5: the offers.
        (
            "/components/schemas/RestartOffer/properties/services",
            "maxItems",
            "RestartOffer.services",
        ),
        // CG10: the refs.
        (
            "/components/schemas/GitRefPage/properties/items",
            "maxItems",
            "GitRefPage.items",
        ),
        // CG11: the rendered doc.
        (
            "/components/schemas/RenderedDoc/properties/html",
            "maxLength",
            "RenderedDoc.html",
        ),
        // CG4a: a property added to an existing schema.
        (
            "/components/schemas/SwimlaneSummary/properties/pickup_counts",
            "maxItems",
            "SwimlaneSummary.pickup_counts",
        ),
        // CG12: another.
        (
            "/components/schemas/Proposal/properties/paths",
            "maxItems",
            "Proposal.paths",
        ),
        // CG1b: the new event.
        (
            "/components/schemas/DomainEventSwimlaneChanged/properties/swimlane",
            "pattern",
            "DomainEventSwimlaneChanged.swimlane",
        ),
    ] {
        let mut d = doc();
        let node = d
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("{pointer} is not in the document"));
        if node.get(key).is_none() {
            // The bound lives behind a `$ref` or a `oneOf`: replace the property by an unbounded one of the same kind.
            let kind = if key == "maxItems" { "array" } else { "string" };
            *node = serde_json::json!({ "type": kind });
        } else {
            node.as_object_mut().unwrap().remove(key);
        }
        let gaps = bound_gaps(&d);
        assert!(
            gaps.iter().any(|g| g.contains(said)),
            "removing {key} at {pointer} was not found: {gaps:#?}"
        );
    }

    // A new object that opens up is found, a schema that is not there is found, and a required addition is found.
    let mut d = doc();
    d["components"]["schemas"]["PrPreview"]["additionalProperties"] = Value::Bool(true);
    assert!(
        bound_gaps(&d)
            .iter()
            .any(|g| g.starts_with("PrPreview is an object that is not closed"))
    );

    let mut d = doc();
    d["components"]["schemas"]
        .as_object_mut()
        .unwrap()
        .remove("UserRef");
    assert!(
        bound_gaps(&d)
            .iter()
            .any(|g| g.contains("UserRef is not in the document"))
    );

    let mut d = doc();
    d["components"]["schemas"]["Proposal"]["required"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("via".into()));
    assert!(
        bound_gaps(&d)
            .iter()
            .any(|g| g.contains("Proposal.via is required"))
    );
}
