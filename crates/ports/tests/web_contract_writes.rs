//! contract/08-web, the operations of `hub-writes` that the web app needs and the contract did not have: the pull-request
//! preview (`CG3`), the two restart offers (`CG5`), one proposal and the decided filter (`CG12`), one draft (`CG17`) and the
//! stored GitHub token with its expiry (`CG14`).
//!
//! Each operation is held to the conventions of this document (a role, the write headers, bounded lists and strings) and to
//! the facts of the server it will be mounted on, which are named in the comments. The operations tagged `writes` also have
//! a `LATER` row in `crates/hub-writes/tests/routes.rs`, which that crate's own test checks.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use serde_json::Value;

const SPEC: &str = include_str!("../../../api/openapi.yaml");
const SCHEMA_REF: &str = "#/components/schemas/";

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

fn ref_name(v: &Value) -> &str {
    let r = v["$ref"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a $ref, got {v}"));
    r.strip_prefix(SCHEMA_REF)
        .unwrap_or_else(|| panic!("not a schema $ref: {r}"))
}

fn strings(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn required_of(schema: &Value) -> BTreeSet<String> {
    schema.get("required").map(strings).unwrap_or_default()
}

fn declared_of(schema: &Value) -> BTreeSet<String> {
    schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no properties: {schema}"))
        .keys()
        .cloned()
        .collect()
}

/// A property schema as one short string, so that a table can say what the document must hold.
fn describe(s: &Value) -> String {
    if s.get("$ref").is_some() {
        return format!("ref:{}", ref_name(s));
    }
    if let Some(arms) = s.get("oneOf").and_then(Value::as_array) {
        return arms.iter().map(describe).collect::<Vec<_>>().join("|");
    }
    match s["type"].as_str() {
        Some("null") => "null".to_owned(),
        Some("integer") => format!("integer:{}", s["format"].as_str().unwrap_or("any")),
        Some("array") => format!("array<{}>", describe(&s["items"])),
        Some(other) => other.to_owned(),
        None => panic!("a property with neither $ref, oneOf nor type: {s}"),
    }
}

/// An operation by its id: method, path and definition.
fn operation<'a>(d: &'a Value, id: &str) -> (String, String, &'a Value) {
    for (path, item) in d["paths"].as_object().expect("paths") {
        for m in ["get", "put", "post", "delete", "patch"] {
            if let Some(op) = item.get(m)
                && op["operationId"] == id
            {
                return (m.to_uppercase(), path.clone(), op);
            }
        }
    }
    panic!("operation {id} is not in the spec");
}

/// The operation's parameters, references resolved.
fn params<'a>(d: &'a Value, op: &'a Value) -> Vec<&'a Value> {
    op["parameters"]
        .as_array()
        .map(|list| list.iter().map(|p| resolve(d, p)).collect())
        .unwrap_or_default()
}

fn param<'a>(d: &'a Value, op: &'a Value, location: &str, name: &str) -> Option<&'a Value> {
    params(d, op)
        .into_iter()
        .find(|p| p["in"] == location && p["name"] == name)
}

/// The resolved response of `code`.
fn response<'a>(d: &'a Value, op: &'a Value, code: &str) -> &'a Value {
    resolve(
        d,
        op["responses"]
            .get(code)
            .unwrap_or_else(|| panic!("{} documents no {code}", op["operationId"])),
    )
}

/// The schema of the JSON body of `code`.
fn json_body<'a>(d: &'a Value, op: &'a Value, code: &str) -> &'a Value {
    response(d, op, code)["content"]["application/json"]
        .get("schema")
        .unwrap_or_else(|| panic!("{} {code} has no JSON body", op["operationId"]))
}

/// What every state-free or state-changing operation of this file must have: a summary, a role, a `writes` or `me` tag,
/// a default problem, and problem+json for every error.
fn assert_conventions(d: &Value, id: &str, role: &str, tag: &str) -> (String, String) {
    let (method, path, op) = operation(d, id);
    assert!(
        op["summary"].as_str().is_some_and(|s| !s.is_empty()),
        "{id} has a summary"
    );
    assert_eq!(op["x-required-role"], role, "{id}: role");
    assert!(
        op["tags"].as_array().unwrap().iter().any(|t| t == tag),
        "{id} is tagged `{tag}`"
    );
    assert!(
        op["responses"].get("default").is_some(),
        "{id} has a default response"
    );
    for (code, r) in op["responses"].as_object().unwrap() {
        if code.starts_with('4') || code.starts_with('5') || code == "default" {
            let content = resolve(d, r)["content"]
                .as_object()
                .unwrap_or_else(|| panic!("{id} {code} has no body"));
            assert_eq!(
                content.keys().collect::<Vec<_>>(),
                ["application/problem+json"],
                "{id} {code}"
            );
        }
    }
    if method == "GET" {
        for write_header in ["X-CSRF-Token", "Idempotency-Key"] {
            assert!(param(d, op, "header", write_header).is_none(), "{id} is a read");
        }
    } else {
        for (header, shared) in [
            ("X-CSRF-Token", "CsrfToken"),
            ("Idempotency-Key", "IdempotencyKey"),
        ] {
            let p = param(d, op, "header", header).unwrap_or_else(|| panic!("{id} has no {header}"));
            assert_eq!(p["required"], true, "{id}: {header} is required");
            assert!(
                op["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["$ref"] == format!("#/components/parameters/{shared}")),
                "{id} uses the shared {shared}"
            );
        }
        assert!(
            op["responses"].get("403").is_some(),
            "{id} documents 403 (role, CSRF)"
        );
    }
    (method, path)
}

/// Every string of a schema has a bound, every array a `maxItems` (S11), looking only at the schema's own tree: a `$ref` is
/// bounded where it is defined. A constant, an enum and a typed `format` need none.
fn assert_bounded(d: &Value, name: &str) {
    fn walk(at: &str, s: &Value) {
        if let Some(obj) = s.as_object() {
            match obj.get("type").and_then(Value::as_str) {
                Some("string") => assert!(
                    obj.contains_key("maxLength")
                        || obj.contains_key("pattern")
                        || obj.contains_key("enum")
                        || obj.contains_key("const")
                        || obj.get("format").is_some_and(|f| f == "binary"),
                    "{at} is an unbounded string"
                ),
                Some("array") => assert!(obj.contains_key("maxItems"), "{at} is an array without maxItems"),
                _ => {}
            }
            for (k, v) in obj {
                if k == "properties" {
                    for (p, ps) in v.as_object().unwrap() {
                        walk(&format!("{at}.{p}"), ps);
                    }
                } else if matches!(k.as_str(), "items" | "oneOf" | "allOf" | "anyOf") {
                    match v {
                        Value::Array(arms) => arms
                            .iter()
                            .enumerate()
                            .for_each(|(i, a)| walk(&format!("{at}.{k}[{i}]"), a)),
                        other => walk(&format!("{at}.{k}"), other),
                    }
                }
            }
        }
    }
    let schema = d["components"]["schemas"]
        .get(name)
        .unwrap_or_else(|| panic!("schema {name} is not in the document"));
    walk(name, schema);
}

/// A closed object: no key the document does not list.
fn assert_closed(d: &Value, name: &str) {
    assert_eq!(
        d["components"]["schemas"][name]["additionalProperties"], false,
        "{name} is closed: a key the document does not list is a bug of the server"
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// CG3: previewPr
// ---------------------------------------------------------------------------------------------------------------------

/// `previewPr` is an Editor write that changes nothing: the headers of a write, no audit event, no record, and the answer
/// says where each file would go, what is unchanged, what the person should know and which PR is already open.
#[test]
fn pr_preview_is_an_editor_write_without_side_effects() {
    let d = doc();
    let (method, path) = assert_conventions(&d, "previewPr", "editor", "writes");
    assert_eq!((method.as_str(), path.as_str()), ("POST", "/api/v1/prs/preview"));
    let (_, _, op) = operation(&d, "previewPr");

    // The answers: createPr's refusals (a bad target is `invalid_content`, an unknown swimlane `unknown_swimlane`, both 422),
    // and nothing that a write that changes state would add: no job (202), no conflict (409), no lock (423).
    for code in ["200", "400", "401", "403", "422", "default"] {
        assert!(op["responses"].get(code).is_some(), "previewPr documents {code}");
    }
    for code in ["202", "404", "409", "413", "423"] {
        assert!(
            op["responses"].get(code).is_none(),
            "previewPr changes nothing, so it has no {code}"
        );
    }
    assert_eq!(
        op["responses"]["422"]["$ref"],
        "#/components/responses/Unprocessable"
    );

    let said = op["description"].as_str().unwrap_or("").to_lowercase();
    for (needle, fact) in [
        ("nothing is written", "that nothing is written"),
        ("audited", "that no audit event is written"),
        ("recorded", "that nothing is recorded"),
        (
            "`idempotency-key`",
            "that the key is required like on every write",
        ),
        ("own github token", "that the user's own token is used (S8)"),
        ("`invalid_content`", "the reason of a bad target"),
        ("`unknown_swimlane`", "the reason of an unknown swimlane"),
    ] {
        assert!(
            said.contains(needle),
            "the description of previewPr must state {fact} (`{needle}`)"
        );
    }

    // The request is a createPr request without the title and the body, so the same paths can be previewed and then raised.
    let schemas = &d["components"]["schemas"];
    let body = &op["requestBody"];
    assert_eq!(body["required"], true);
    assert_eq!(
        body["content"]["application/json"]["schema"]["$ref"],
        format!("{SCHEMA_REF}PrPreviewRequest")
    );
    let request = &schemas["PrPreviewRequest"];
    assert_closed(&d, "PrPreviewRequest");
    assert_eq!(declared_of(request), set(&["swimlane", "paths", "target"]));
    assert_eq!(
        required_of(request),
        set(&["swimlane", "paths"]),
        "the target is optional, as on createPr"
    );
    let create = &schemas["CreatePrRequest"]["properties"];
    for key in ["swimlane", "paths", "target"] {
        assert_eq!(
            request["properties"][key], create[key],
            "PrPreviewRequest.{key} is CreatePrRequest.{key}, so a preview and its PR cannot disagree about the input"
        );
    }
    let paths = &request["properties"]["paths"];
    assert_eq!(
        (paths["minItems"].as_u64(), paths["maxItems"].as_u64()),
        (Some(1), Some(100))
    );
    assert_eq!(paths["uniqueItems"], true);
}

/// The answer: every field is present (a nullable one is null), the lists are bounded and the words are `PrJob`'s.
#[test]
fn pr_preview_answer_is_bounded_and_says_where_each_file_goes() {
    let d = doc();
    let (_, _, op) = operation(&d, "previewPr");
    assert_eq!(json_body(&d, op, "200")["$ref"], format!("{SCHEMA_REF}PrPreview"));

    let schemas = &d["components"]["schemas"];
    let preview = &schemas["PrPreview"];
    assert_closed(&d, "PrPreview");
    let fields = [
        ("repo", "ref:ShortText"),
        ("destination", "ref:PrTarget"),
        ("base_branch", "ref:GitRef"),
        ("files", "array<ref:PrPreviewFile>"),
        ("unchanged", "array<ref:NfsPath>"),
        ("warnings", "array<ref:ShortText>"),
        ("open_pr", "ref:PrLink|null"),
    ];
    assert_eq!(
        declared_of(preview),
        fields.iter().map(|(k, _)| (*k).to_owned()).collect()
    );
    assert_eq!(
        required_of(preview),
        declared_of(preview),
        "every field is sent; `open_pr` is null when there is none"
    );
    for (key, want) in fields {
        assert_eq!(describe(&preview["properties"][key]), want, "PrPreview.{key}");
    }
    for key in ["files", "unchanged"] {
        assert_eq!(
            preview["properties"][key]["maxItems"], 100,
            "PrPreview.{key} is at most the request's 100 paths"
        );
    }
    // The warnings are the words of the job that will be created, bounded the same way.
    let job = &schemas["PrJob"]["properties"]["warnings"];
    assert_eq!(preview["properties"]["warnings"]["maxItems"], job["maxItems"]);
    assert_eq!(preview["properties"]["warnings"]["items"], job["items"]);

    let file = &schemas["PrPreviewFile"];
    assert_closed(&d, "PrPreviewFile");
    assert_eq!(
        declared_of(file),
        set(&["nfs_path", "repo_path", "exists_on_branch"])
    );
    assert_eq!(required_of(file), declared_of(file));
    assert_eq!(describe(&file["properties"]["nfs_path"]), "ref:NfsPath");
    assert_eq!(describe(&file["properties"]["repo_path"]), "ref:RepoPath");
    assert_eq!(file["properties"]["exists_on_branch"]["type"], "boolean");

    // Where the repository's name comes from and what the destination is are said, not left to the name.
    let said = |s: &Value| s["description"].as_str().unwrap_or("").to_lowercase();
    assert!(
        said(&preview["properties"]["repo"]).contains("full name"),
        "repo is the repository's full name"
    );
    assert!(
        said(&preview["properties"]["destination"]).contains("resolved"),
        "destination is the resolved target"
    );
    assert!(
        said(&preview["properties"]["open_pr"]).contains("open"),
        "open_pr is the PR that is already open"
    );
    assert!(
        said(&file["properties"]["exists_on_branch"]).contains("branch"),
        "exists_on_branch says on which branch"
    );

    for name in ["PrPreviewRequest", "PrPreview", "PrPreviewFile"] {
        assert_bounded(&d, name);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// CG5: the restart offers
// ---------------------------------------------------------------------------------------------------------------------

/// The most services an offer lists: `MAX_RESTART_OFFER` of `hub-writes/src/ops/restart.rs`.
const MAX_RESTART_OFFER: u64 = 2_000;

/// Both offers are Operator reads, like the restart they prepare, and their lists are bounded and say when they are cut.
#[test]
fn restart_offers_are_operator_reads_with_bounded_lists() {
    let d = doc();
    let restart_role = operation(&d, "restartService").2["x-required-role"].clone();

    for (id, want_path) in [
        (
            "getConfigServerRestartOffer",
            "/api/v1/swimlanes/{id}/config-server-restart",
        ),
        ("getRestartOffer", "/api/v1/swimlanes/{id}/restart-offer"),
    ] {
        let (method, path) = assert_conventions(&d, id, "operator", "writes");
        assert_eq!((method.as_str(), path.as_str()), ("GET", want_path), "{id}");
        let (_, _, op) = operation(&d, id);
        assert_eq!(
            op["x-required-role"], restart_role,
            "{id} needs the role of the restart it prepares"
        );
        for code in ["200", "400", "401", "403", "404", "default"] {
            assert!(op["responses"].get(code).is_some(), "{id} documents {code}");
        }
        // A bounded answer with a flag, not a list with a cursor.
        assert!(
            param(&d, op, "query", "cursor").is_none(),
            "{id} has no cursor: the list is cut at {MAX_RESTART_OFFER}"
        );
        assert!(param(&d, op, "query", "limit").is_none(), "{id} has no limit");
        let swimlane = param(&d, op, "path", "id").unwrap_or_else(|| panic!("{id} has no swimlane"));
        assert_eq!(swimlane["required"], true);
        assert_eq!(describe(&swimlane["schema"]), "ref:SwimlaneId");
        // What a screen shows for a change that has not happened yet must never be kept by a cache.
        let cache = &response(&d, op, "200")["headers"]["Cache-Control"]["schema"];
        assert_eq!(
            cache["enum"],
            serde_json::json!(["no-store"]),
            "{id} sends Cache-Control: no-store"
        );
    }

    // Restart-all names the change it is for.
    let (_, _, all) = operation(&d, "getRestartOffer");
    let path = param(&d, all, "query", "path").expect("getRestartOffer takes the path of the change");
    assert_eq!(path["required"], true);
    assert_eq!(describe(&path["schema"]), "ref:NfsPath");
    assert_eq!(
        json_body(&d, all, "200")["$ref"],
        format!("{SCHEMA_REF}RestartOffer")
    );
    let (_, _, cs) = operation(&d, "getConfigServerRestartOffer");
    assert!(
        param(&d, cs, "query", "path").is_none(),
        "the config-server restart does not depend on a path"
    );
    assert_eq!(
        json_body(&d, cs, "200")["$ref"],
        format!("{SCHEMA_REF}ConfigServerRestartOfferResult")
    );
}

/// The shapes of `hub-writes`' `RestartOffer` and `ConfigServerRestartOffer`, the `Option` around the second, and the words
/// that say when there is no offer and when a list is cut.
#[test]
fn restart_offer_schemas_are_bounded_and_say_when_they_are_cut() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    // The shapes of `hub-writes`' `RestartOffer`, `ConfigServerRestartOffer` and the `Option` around the second.
    let restart = &schemas["RestartOffer"];
    assert_closed(&d, "RestartOffer");
    assert_eq!(declared_of(restart), set(&["services", "truncated"]));
    assert_eq!(required_of(restart), declared_of(restart));
    assert_eq!(
        describe(&restart["properties"]["services"]),
        "array<ref:ServiceRef>"
    );
    assert_eq!(restart["properties"]["services"]["maxItems"], MAX_RESTART_OFFER);
    assert_eq!(restart["properties"]["truncated"]["type"], "boolean");

    let offer = &schemas["ConfigServerRestartOffer"];
    assert_closed(&d, "ConfigServerRestartOffer");
    assert_eq!(declared_of(offer), set(&["service", "affected", "truncated"]));
    assert_eq!(required_of(offer), declared_of(offer));
    assert_eq!(describe(&offer["properties"]["service"]), "ref:ServiceRef");
    assert_eq!(
        describe(&offer["properties"]["affected"]),
        "array<ref:ServiceRef>"
    );
    assert_eq!(offer["properties"]["affected"]["maxItems"], MAX_RESTART_OFFER);
    assert_eq!(offer["properties"]["truncated"]["type"], "boolean");

    let result = &schemas["ConfigServerRestartOfferResult"];
    assert_closed(&d, "ConfigServerRestartOfferResult");
    assert_eq!(declared_of(result), set(&["offer"]));
    assert_eq!(
        required_of(result),
        set(&["offer"]),
        "no offer is `null`, not an absent key and not a 404"
    );
    assert_eq!(
        describe(&result["properties"]["offer"]),
        "ref:ConfigServerRestartOffer|null"
    );

    // Said in words: when there is no offer, and that a cut list is not the whole of it (the screen must not look complete).
    let text = |s: &Value| s["description"].as_str().unwrap_or("").to_lowercase();
    let none = format!("{} {}", text(&result["properties"]["offer"]), text(result));
    for needle in ["c11", "deployment"] {
        assert!(
            none.contains(needle),
            "ConfigServerRestartOfferResult must say when the offer is null (`{needle}`)"
        );
    }
    for (what, s) in [
        ("RestartOffer.truncated", &restart["properties"]["truncated"]),
        (
            "ConfigServerRestartOffer.truncated",
            &offer["properties"]["truncated"],
        ),
    ] {
        let said = text(s);
        assert!(
            said.contains("not the whole") || said.contains("more than"),
            "{what} must say that the list is not all of the services"
        );
    }
    let affected = text(&offer["properties"]["affected"]);
    assert!(
        affected.contains("every other service"),
        "affected is every other service of the swimlane"
    );

    for name in [
        "RestartOffer",
        "ConfigServerRestartOffer",
        "ConfigServerRestartOfferResult",
    ] {
        assert_bounded(&d, name);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// CG12: one proposal, the decided tab and the documented 422
// ---------------------------------------------------------------------------------------------------------------------

/// The keys `Proposal` had before this commit: all of them stay, and none is added to the required ones.
const PROPOSAL_REQUIRED: &[&str] = &[
    "id",
    "author",
    "swimlane",
    "path",
    "expected",
    "new_hash",
    "status",
    "hunks",
    "setting_changes",
    "severity",
    "created_at",
    "expires_at",
    "decided_by",
    "decision_comment",
];

/// `proposals.action` of migration 0004: what a proposal is for.
const PROPOSAL_ACTIONS: &[&str] = &["edit", "upload", "delete", "revert", "restart", "pr", "notify"];

/// `getProposal` mirrors `listProposals`, and `Proposal` says what kind of change it is, how it came and which paths it has,
/// without a new required key (an old body is still a `Proposal`).
#[test]
fn proposal_additions_are_optional() {
    let d = doc();
    let schemas = &d["components"]["schemas"];

    let (method, path) = assert_conventions(&d, "getProposal", "viewer", "writes");
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("GET", "/api/v1/proposals/{id}")
    );
    let (_, _, one) = operation(&d, "getProposal");
    let (_, _, list) = operation(&d, "listProposals");
    assert_eq!(
        one["x-required-role"], list["x-required-role"],
        "the list already returns the same Proposal with its hunks, so one is no stricter than the list"
    );
    for code in ["200", "400", "401", "403", "404", "default"] {
        assert!(
            one["responses"].get(code).is_some(),
            "getProposal documents {code}"
        );
    }
    assert_eq!(json_body(&d, one, "200")["$ref"], format!("{SCHEMA_REF}Proposal"));
    assert!(
        one["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["$ref"] == "#/components/parameters/NumericId")
    );

    let proposal = &schemas["Proposal"];
    assert_closed(&d, "Proposal");
    assert_eq!(
        required_of(proposal),
        set(PROPOSAL_REQUIRED),
        "an old body is still a Proposal: nothing new is required"
    );
    let fields = [
        ("action", "ref:ProposalAction"),
        ("via", "ref:ProposalVia"),
        ("paths", "array<ref:NfsPath>"),
        ("decided_at", "ref:Timestamp|null"),
    ];
    for (key, want) in fields {
        assert_eq!(describe(&proposal["properties"][key]), want, "Proposal.{key}");
        assert!(!required_of(proposal).contains(key), "Proposal.{key} is optional");
    }
    assert_eq!(
        proposal["properties"]["paths"]["maxItems"], 100,
        "a proposal carries at most 100 paths (proposals.paths)"
    );

    // The wire names are the domain's and the migration's.
    assert_eq!(
        strings(&schemas["ProposalAction"]["enum"]),
        set(PROPOSAL_ACTIONS),
        "ProposalAction is the CHECK of proposals.action"
    );
    assert_eq!(
        strings(&schemas["ProposalVia"]["enum"]),
        domain::Via::ALL.iter().map(|v| v.as_str().to_owned()).collect(),
        "ProposalVia is domain::Via"
    );

    let text = |s: &Value| s["description"].as_str().unwrap_or("").to_lowercase();
    assert!(
        text(&proposal["properties"]["new_hash"]).contains("all-zero"),
        "an all-zero hash means that no content is written"
    );
    assert!(text(&proposal["properties"]["new_hash"]).contains("no content"));
    assert!(
        text(&proposal["properties"]["paths"]).contains("namespace/name"),
        "a restart's one path is `namespace/name`"
    );
    assert!(
        text(&proposal["properties"]["path"]).contains("first"),
        "`path` is the first of `paths`"
    );

    // The decided tab: one request, and not together with a status.
    let decided = param(&d, list, "query", "decided").expect("listProposals takes `decided`");
    assert_eq!(decided["required"], false);
    assert_eq!(decided["schema"]["type"], "boolean");
    assert_eq!(
        decided["schema"]["enum"],
        serde_json::json!([true]),
        "only `true` has a meaning, so `false` is refused"
    );
    let said = text(decided);
    for needle in ["approved", "applied", "rejected", "`status`", "400"] {
        assert!(said.contains(needle), "`decided` must say `{needle}`");
    }
    for name in ["status", "swimlane", "cursor", "limit"] {
        assert!(
            param(&d, list, "query", name).is_some(),
            "listProposals keeps `{name}`"
        );
    }

    for name in ["ProposalAction", "ProposalVia"] {
        assert_bounded(&d, name);
    }
}

/// Approving and rejecting a proposal that is not pending (or has expired) is a 422 `not_allowed`, and a decision that
/// finds the lease held is 423: both are documented on both operations.
#[test]
fn decisions_document_422_and_423() {
    let d = doc();
    for id in ["approveProposal", "rejectProposal"] {
        let (_, _, op) = operation(&d, id);
        assert_eq!(op["x-required-role"], "admin", "{id}");
        assert_eq!(
            op["responses"]["422"]["$ref"], "#/components/responses/Unprocessable",
            "{id} documents 422"
        );
        assert_eq!(
            op["responses"]["423"]["$ref"], "#/components/responses/Locked",
            "{id} documents 423"
        );
        let said = op["description"].as_str().unwrap_or("").to_lowercase();
        for needle in ["not pending", "`not_allowed`", "423"] {
            assert!(
                said.contains(needle),
                "the description of {id} must say `{needle}`"
            );
        }
        for code in ["200", "400", "401", "403", "404", "default"] {
            assert!(op["responses"].get(code).is_some(), "{id} keeps {code}");
        }
    }
    // The stale-proposal conflict stays where it was.
    let (_, _, approve) = operation(&d, "approveProposal");
    assert_eq!(
        approve["responses"]["409"]["$ref"],
        "#/components/responses/Conflict"
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// CG17: one draft
// ---------------------------------------------------------------------------------------------------------------------

/// `getDraft` is the REST face of `WriteService::draft`: an Editor read of the caller's own draft, with no side effect and no
/// key. A draft that is not the caller's is a 404, the same as one that does not exist.
#[test]
fn get_draft_is_an_editor_read_of_the_authors_draft() {
    let d = doc();
    let (method, path) = assert_conventions(&d, "getDraft", "editor", "writes");
    assert_eq!((method.as_str(), path.as_str()), ("GET", "/api/v1/drafts/{id}"));
    let (_, _, op) = operation(&d, "getDraft");
    let (_, _, apply) = operation(&d, "applyDraft");
    assert_eq!(
        op["x-required-role"], apply["x-required-role"],
        "the draft is read at the role that created and applies it"
    );

    // The same path parameter as `applyDraft`, the draft's own id.
    let id = param(&d, op, "path", "id").expect("getDraft takes the draft id");
    assert_eq!(id["required"], true);
    assert_eq!(describe(&id["schema"]), "ref:DraftId");
    assert_eq!(
        id,
        param(&d, apply, "path", "id").unwrap(),
        "getDraft and applyDraft name the draft alike"
    );

    // The answers: the draft, and the refusals of a read. Nothing that a write would add.
    assert_eq!(json_body(&d, op, "200")["$ref"], format!("{SCHEMA_REF}Draft"));
    for code in ["200", "400", "401", "403", "404", "default"] {
        assert!(op["responses"].get(code).is_some(), "getDraft documents {code}");
    }
    for code in ["409", "413", "422", "423"] {
        assert!(
            op["responses"].get(code).is_none(),
            "getDraft changes nothing, so it has no {code}"
        );
    }
    assert_eq!(op["responses"]["404"]["$ref"], "#/components/responses/NotFound");
    let cache = &response(&d, op, "200")["headers"]["Cache-Control"]["schema"];
    assert_eq!(
        cache["enum"],
        serde_json::json!(["no-store"]),
        "a draft is its author's and is never cached"
    );

    let said = op["description"].as_str().unwrap_or("").to_lowercase();
    for (needle, fact) in [
        ("author", "that only the author may read it"),
        ("404", "that another user's draft is a 404"),
        ("applied", "that an applied draft is still returned"),
        ("expired", "that an expired draft is still returned"),
        ("`status`", "that the status says which"),
        ("nothing is written", "that nothing is written"),
        ("audit", "that nothing is audited"),
        (
            "engine",
            "that the severity and the effects are the engine's, as of the call",
        ),
    ] {
        assert!(
            said.contains(needle),
            "the description of getDraft must state {fact} (`{needle}`)"
        );
    }
}

/// Every operation this file adds names its audit action, or says why it has none (rule 8: one audit event per state
/// change). A read changes nothing; the one non-GET, `previewPr`, writes and records nothing, as `previewFileEffect` does.
#[test]
fn new_operations_name_their_audit_action_or_say_why_not() {
    let d = doc();
    // (operationId, the audit action, or why there is none)
    let table: &[(&str, &str)] = &[
        ("previewPr", "none: no state change"),
        ("getConfigServerRestartOffer", "none: a read"),
        ("getRestartOffer", "none: a read"),
        ("getProposal", "none: a read"),
        ("getDraft", "none: a read"),
    ];
    for (id, audit) in table {
        let (method, _, op) = operation(&d, id);
        assert!(
            audit.starts_with("none: "),
            "{id}: every operation of this table is state-free, so it has no action"
        );
        if method != "GET" {
            let said = op["description"].as_str().unwrap_or("").to_lowercase();
            assert!(
                said.contains("nothing is written") && said.contains("audited"),
                "{id} is a non-GET without an audit event, so it must say that nothing is written or audited"
            );
        }
    }
}
