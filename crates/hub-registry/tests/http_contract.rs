//! A43 (T6): every response of the fifteen read operations matches `api/openapi.yaml`: the status is one the operation declares, the
//! media type is one its response declares, the body validates against its schema (types, required, `additionalProperties`, enums,
//! `oneOf`, bounds), and the headers it declares are there. Success bodies of every kind the service can make, and the error bodies the
//! contract names (400, 401, 403, 404) and the `default` one (503). The walker is `support/openapi.rs`, with its own tests and negative
//! controls (`openapi_walker`); here a drifted response is made on purpose to show the contract test notices.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use domain::{AgentReply, HubCommand, SwimlaneId};
use serde_json::{Value, json};
use support::http::{Http, Reply};
use support::openapi::Spec;

/// One request and what the contract says it must be.
struct Case {
    op: &'static str,
    uri: String,
    user: Option<&'static str>,
    headers: Vec<(&'static str, String)>,
    status: u16,
}

fn case(op: &'static str, uri: &str, status: u16) -> Case {
    Case {
        op,
        uri: uri.to_owned(),
        user: Some("viewer"),
        headers: Vec::new(),
        status,
    }
}

fn as_user(mut c: Case, user: Option<&'static str>) -> Case {
    c.user = user;
    c
}

fn with_header(mut c: Case, name: &'static str, value: &str) -> Case {
    c.headers.push((name, value.to_owned()));
    c
}

/// Whatever is wrong with `reply` as an answer of operation `op` (the method and path template come from the spec).
fn violations(spec: &Spec, op_id: &str, reply: &Reply) -> Vec<String> {
    let (_, method, template) = spec
        .operation_ids()
        .into_iter()
        .find(|(id, _, _)| id == op_id)
        .unwrap_or_else(|| panic!("{op_id} is not an operation of the spec"));
    let op = spec.operation(&method, &template).unwrap();
    let status = reply.status.as_u16();
    let mut out = Vec::new();
    let Some(response) = spec.response(op, status) else {
        return vec![format!("status {status} is not declared by {op_id}")];
    };
    let media_types = spec.media_types(response);
    if reply.body.is_empty() {
        if !media_types.is_empty() && status != 304 {
            out.push(format!(
                "an empty body, but {op_id} {status} declares {media_types:?}"
            ));
        }
    } else {
        let content_type = reply
            .header("content-type")
            .map(|c| c.split(';').next().unwrap_or_default().trim().to_owned());
        match content_type {
            Some(ct) if media_types.contains(&ct) => match serde_json::from_slice::<Value>(&reply.body) {
                Ok(body) => {
                    let schema = spec.body_schema(response, &ct).unwrap();
                    out.extend(spec.check(schema, &body).into_iter().map(|v| format!("body{v}")));
                }
                Err(e) => out.push(format!("the body is not JSON: {e}")),
            },
            other => out.push(format!("content type {other:?} is not one of {media_types:?}")),
        }
    }
    for header in spec.header_names(response) {
        if reply.header(&header).is_none() {
            out.push(format!("declared header `{header}` is missing"));
        }
    }
    out
}

fn served(h: &Http) {
    h.reads.gateway.attach(
        &SwimlaneId::parse("sit1").unwrap(),
        Arc::new(|cmd: &HubCommand| match cmd {
            HubCommand::FetchServed { request_id, .. } => Some(AgentReply::Served {
                request_id: request_id.clone(),
                status: 200,
                bytes: Bytes::from_static(b"port: 2\nname: ${NOT_THERE}\n"),
            }),
            _ => None,
        }),
    );
}

const HASH: &str = "07e4599f1a1c623dcc82d95606770a9e50d62a014fefe1fe99ce9bb58922d419";

/// A case for every kind of answer of every operation.
async fn cases(h: &Http) -> Vec<Case> {
    // The hash of `core.yml` as the tree shows it, for the `blob` source.
    let tree = h.view("/api/v1/swimlanes/sit1/tree?limit=500").await.json();
    let core_hash = tree["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "core.yml")
        .map_or(HASH.to_owned(), |e| e["hash"].as_str().unwrap().to_owned());
    let sw = "/api/v1/swimlanes/sit1";
    let served_uri = format!("{sw}/served?application=core&tenant=sit1&file=core.yml");
    let mut out = vec![
        // listSwimlanes
        case("listSwimlanes", "/api/v1/swimlanes", 200),
        case("listSwimlanes", "/api/v1/swimlanes?limit=1", 200),
        case("listSwimlanes", "/api/v1/swimlanes?limit=0", 400),
        // getSwimlane
        case("getSwimlane", sw, 200),
        case("getSwimlane", "/api/v1/swimlanes/nope", 404),
        case("getSwimlane", "/api/v1/swimlanes/NOPE", 400),
        // listSwimlaneTree
        case("listSwimlaneTree", &format!("{sw}/tree"), 200),
        case("listSwimlaneTree", &format!("{sw}/tree?prefix=keys"), 200),
        case(
            "listSwimlaneTree",
            &format!("{sw}/tree?drift=untracked&limit=2"),
            200,
        ),
        case("listSwimlaneTree", &format!("{sw}/tree?prefix=..%2Fx"), 400),
        case("listSwimlaneTree", "/api/v1/swimlanes/nope/tree", 404),
        // listDrift
        case("listDrift", &format!("{sw}/drift"), 200),
        case(
            "listDrift",
            &format!("{sw}/drift?state=unknown&prefix=core-sit1.yml"),
            200,
        ),
        case("listDrift", &format!("{sw}/drift?state=nope"), 400),
        case("listDrift", "/api/v1/swimlanes/nope/drift", 404),
        // getFileContent
        case(
            "getFileContent",
            &format!("{sw}/files/content?path=core.yml"),
            200,
        ),
        case(
            "getFileContent",
            &format!("{sw}/files/content?path=crlf.txt"),
            200,
        ),
        case(
            "getFileContent",
            &format!("{sw}/files/content?path=blob.bin"),
            200,
        ),
        case(
            "getFileContent",
            &format!("{sw}/files/content?path=keys%2Fserver.pem"),
            200,
        ),
        case(
            "getFileContent",
            &format!("{sw}/files/content?path=core.yml&source=blob&hash={core_hash}"),
            200,
        ),
        case(
            "getFileContent",
            &format!("{sw}/files/content?path=missing.yml"),
            404,
        ),
        case(
            "getFileContent",
            &format!("{sw}/files/content?path=core.yml&source=git&repo=base&ref=nope"),
            404,
        ),
        case("getFileContent", &format!("{sw}/files/content"), 400),
        // listFileHistory
        case(
            "listFileHistory",
            &format!("{sw}/files/history?path=core.yml"),
            200,
        ),
        case(
            "listFileHistory",
            &format!("{sw}/files/history?path=keys%2Fserver.pem"),
            200,
        ),
        case("listFileHistory", &format!("{sw}/files/history"), 400),
        case(
            "listFileHistory",
            "/api/v1/swimlanes/nope/files/history?path=core.yml",
            404,
        ),
        // compareFile
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=core.yml",
            200,
        ),
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit1&file=core.yml",
            200,
        ),
        case(
            "compareFile",
            "/api/v1/compare?left=effective%3Asit1&right=effective%3Asit2&file=core.yml",
            200,
        ),
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=crlf.txt",
            200,
        ),
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=blob.bin",
            200,
        ),
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=keys%2Fserver.pem",
            200,
        ),
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=notes.txt",
            200,
        ),
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2",
            400,
        ),
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Anope&right=nfs%3Asit2&file=core.yml",
            404,
        ),
        // compareTree
        case(
            "compareTree",
            "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2",
            200,
        ),
        case(
            "compareTree",
            "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2&limit=1",
            200,
        ),
        case(
            "compareTree",
            "/api/v1/compare/tree?left=effective%3Asit1&right=effective%3Asit3",
            200,
        ),
        case(
            "compareTree",
            "/api/v1/compare/tree?left=effective%3Asit1&right=effective%3Asit1",
            200,
        ),
        case(
            "compareTree",
            "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2&prefix=keys",
            200,
        ),
        case("compareTree", "/api/v1/compare/tree?left=nfs%3Asit1", 400),
        // getGrid
        case(
            "getGrid",
            "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2,sit3",
            200,
        ),
        case(
            "getGrid",
            "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2,sit3&only_differing=true&limit=2",
            200,
        ),
        case("getGrid", "/api/v1/grid?file=notes.txt&swimlanes=sit1,sit2", 200),
        case(
            "getGrid",
            "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2&channel=web",
            200,
        ),
        case("getGrid", "/api/v1/grid?file=core.yml", 400),
        case("getGrid", "/api/v1/grid?file=core.yml&swimlanes=sit1,nope", 404),
        // searchSettings
        case("searchSettings", "/api/v1/search/settings?q=port", 200),
        case(
            "searchSettings",
            "/api/v1/search/settings?q=token&swimlanes=sit1&file=core.yml",
            200,
        ),
        case("searchSettings", "/api/v1/search/settings?q=zzzz-nothing", 200),
        case("searchSettings", "/api/v1/search/settings?q=", 400),
        case(
            "searchSettings",
            "/api/v1/search/settings?q=a&swimlanes=nope",
            404,
        ),
        // searchText
        case("searchText", "/api/v1/search/text?pattern=timeout", 200),
        case(
            "searchText",
            "/api/v1/search/text?pattern=t.%2Bt&regex=true&case_sensitive=true&max_total=2&max_per_file=1",
            200,
        ),
        case(
            "searchText",
            "/api/v1/search/text?pattern=timeout&swimlanes=sit1,sit2&path_glob=**%2F*.txt",
            200,
        ),
        case("searchText", "/api/v1/search/text?pattern=zzzz-nothing", 200),
        case("searchText", "/api/v1/search/text?pattern=", 400),
        case("searchText", "/api/v1/search/text?pattern=%28&regex=true", 400),
        // listFindings
        case("listFindings", "/api/v1/findings", 200),
        case(
            "listFindings",
            "/api/v1/findings?swimlane=sit1&kinds=drift,orphan_file&min_severity=low&limit=3",
            200,
        ),
        case("listFindings", "/api/v1/findings?kinds=nope", 400),
        // getEffectiveConfig
        case(
            "getEffectiveConfig",
            &format!("{sw}/effective?file=core.yml&tenant=sit1"),
            200,
        ),
        case(
            "getEffectiveConfig",
            &format!("{sw}/effective?file=notes.txt&tenant=sit1"),
            200,
        ),
        case(
            "getEffectiveConfig",
            &format!("{sw}/effective?file=core.yml&tenant=sit1&channel=web"),
            200,
        ),
        case(
            "getEffectiveConfig",
            &format!("{sw}/effective?file=missing.yml&tenant=sit1"),
            404,
        ),
        case(
            "getEffectiveConfig",
            &format!("{sw}/effective?file=core.yml&tenant=sit9"),
            404,
        ),
        case(
            "getEffectiveConfig",
            &format!("{sw}/effective?file=core.yml"),
            400,
        ),
        // listPendingRestarts
        case("listPendingRestarts", &format!("{sw}/pending-restarts"), 200),
        case(
            "listPendingRestarts",
            "/api/v1/swimlanes/sit2/pending-restarts",
            200,
        ),
        case(
            "listPendingRestarts",
            "/api/v1/swimlanes/nope/pending-restarts",
            404,
        ),
        case(
            "listPendingRestarts",
            &format!("{sw}/pending-restarts?limit=501"),
            400,
        ),
        // getServedFile
        case("getServedFile", &served_uri, 200),
        case(
            "getServedFile",
            &format!("{sw}/served?application=core&tenant=sit1&file=keys%2Fserver.pem"),
            200,
        ),
        case(
            "getServedFile",
            &format!("{sw}/served?application=core&tenant=sit9&file=core.yml"),
            404,
        ),
        case(
            "getServedFile",
            &format!("{sw}/served?application=core&tenant=sit1"),
            400,
        ),
    ];
    // Optional parameters that no case above has used.
    out.extend([
        case(
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=core.yml&channel=web",
            200,
        ),
        case(
            "searchText",
            "/api/v1/search/text?pattern=timeout&swimlanes=sit1,sit2&path_glob=**%2F*.txt",
            200,
        ),
        case(
            "getServedFile",
            &format!("{sw}/served?application=core&tenant=sit1&channel=web&file=core.yml"),
            200,
        ),
    ]);
    // The second page of every paged list: a real cursor of its own query.
    let paged = [
        ("listSwimlanes", "/api/v1/swimlanes".to_owned(), "/next"),
        ("listSwimlaneTree", format!("{sw}/tree"), "/next"),
        ("listDrift", format!("{sw}/drift"), "/next"),
        (
            "listFileHistory",
            format!("{sw}/files/history?path=notes.txt"),
            "/next",
        ),
        (
            "compareTree",
            "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2".to_owned(),
            "/entries/next",
        ),
        (
            "getGrid",
            "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2,sit3".to_owned(),
            "/rows/next",
        ),
        (
            "searchSettings",
            "/api/v1/search/settings?q=port".to_owned(),
            "/next",
        ),
        ("listFindings", "/api/v1/findings".to_owned(), "/next"),
        ("listPendingRestarts", format!("{sw}/pending-restarts"), "/next"),
    ];
    for (op, base, pointer) in paged {
        let sep = if base.contains('?') { '&' } else { '?' };
        let first = h.view(&format!("{base}{sep}limit=1")).await.json();
        let next = first
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{op}: the data has no second page to ask for"))
            .to_owned();
        out.push(case(op, &format!("{base}{sep}limit=1&cursor={next}"), 200));
    }
    // 304 where the contract declares it: the four `content` operations.
    for (op, uri) in [
        ("getFileContent", format!("{sw}/files/content?path=core.yml")),
        (
            "compareFile",
            "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=core.yml".to_owned(),
        ),
        (
            "getEffectiveConfig",
            format!("{sw}/effective?file=core.yml&tenant=sit1"),
        ),
        ("getServedFile", served_uri),
    ] {
        let tag = h.view(&uri).await.header("etag").unwrap().to_owned();
        out.push(with_header(case(op, &uri, 304), "if-none-match", &tag));
    }
    // 401 and 403 on every operation.
    let mut denied = Vec::new();
    for c in &out {
        if c.status == 200 && !denied.iter().any(|(op, _): &(&str, String)| *op == c.op) {
            denied.push((c.op, c.uri.clone()));
        }
    }
    for (op, uri) in denied {
        out.push(as_user(case(op, &uri, 401), None));
        out.push(as_user(case(op, &uri, 403), Some("pending")));
    }
    out
}

#[tokio::test]
async fn every_response_of_every_read_operation_matches_the_contract() {
    let Some(h) = Http::seeded().await else { return };
    served(&h);
    let spec = Spec::load();
    let cases = cases(&h).await;
    let mut failures: Vec<String> = Vec::new();
    for c in &cases {
        let headers: Vec<(&str, &str)> = c.headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let reply = h.get_with(c.user, &c.uri, &headers).await;
        if reply.status.as_u16() != c.status {
            failures.push(format!(
                "{} {}: expected {}, got {} {}",
                c.op,
                c.uri,
                c.status,
                reply.status,
                reply.text()
            ));
            continue;
        }
        for v in violations(&spec, c.op, &reply) {
            failures.push(format!("{} {} ({}): {v}", c.op, c.uri, c.status));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} responses break the contract:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn every_status_the_contract_names_for_a_read_operation_is_exercised() {
    let Some(h) = Http::seeded().await else { return };
    served(&h);
    let spec = Spec::load();
    let cases = cases(&h).await;
    let ids = [
        "listSwimlanes",
        "getSwimlane",
        "listSwimlaneTree",
        "listDrift",
        "getFileContent",
        "listFileHistory",
        "compareFile",
        "compareTree",
        "getGrid",
        "searchSettings",
        "searchText",
        "listFindings",
        "getEffectiveConfig",
        "listPendingRestarts",
        "getServedFile",
    ];
    for id in ids {
        let (_, method, template) = spec
            .operation_ids()
            .into_iter()
            .find(|(i, _, _)| i == id)
            .unwrap();
        let op = spec.operation(&method, &template).unwrap();
        for status in op["responses"]
            .as_object()
            .unwrap()
            .keys()
            .filter_map(|s| s.parse::<u16>().ok())
        {
            assert!(
                cases.iter().any(|c| c.op == id && c.status == status),
                "{id} declares {status} and no case exercises it"
            );
        }
    }
    // The spec's `parameters` of each operation are what the table of cases uses: every declared query parameter appears in some
    // request of its operation, so a parameter added to the spec without a case is found here.
    for id in ids {
        let (_, method, template) = spec
            .operation_ids()
            .into_iter()
            .find(|(i, _, _)| i == id)
            .unwrap();
        let op = spec.operation(&method, &template).unwrap();
        let used: BTreeSet<String> = cases
            .iter()
            .filter(|c| c.op == id)
            .flat_map(|c| {
                c.uri
                    .split_once('?')
                    .map(|(_, q)| {
                        q.split('&')
                            .filter_map(|p| p.split('=').next())
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect();
        for p in spec.parameters(op).iter().filter(|p| p.location == "query") {
            assert!(
                used.contains(&p.name),
                "{id}: no case uses the query parameter `{}`",
                p.name
            );
        }
    }
}

// ------------------------------------------------------------------------------------------------ negative controls

#[tokio::test]
async fn the_contract_test_notices_a_response_that_drifts_from_the_spec() {
    let Some(h) = Http::seeded().await else { return };
    let spec = Spec::load();
    let good = h.view("/api/v1/swimlanes/sit1").await;
    assert!(
        violations(&spec, "getSwimlane", &good).is_empty(),
        "{:?}",
        violations(&spec, "getSwimlane", &good)
    );

    let tamper = |edit: &dyn Fn(&mut Value)| {
        let mut body = good.json();
        edit(&mut body);
        Reply {
            status: good.status,
            headers: good.headers.clone(),
            body: Bytes::from(serde_json::to_vec(&body).unwrap()),
        }
    };
    let drifted = [
        (
            "a required field is gone",
            tamper(&|b| {
                b.as_object_mut().unwrap().remove("agent");
            }),
        ),
        (
            "a field the spec does not know",
            tamper(&|b| b["surprise"] = json!(1)),
        ),
        (
            "a field of the wrong type",
            tamper(&|b| b["finding_count"] = json!("8")),
        ),
        (
            "an enum value outside its set",
            tamper(&|b| b["agent"] = json!("sleeping")),
        ),
        (
            "a nested field of the wrong type",
            tamper(&|b| b["services"][0]["cache_ttl_secs"] = json!("20")),
        ),
        ("an id outside its pattern", tamper(&|b| b["id"] = json!("SIT 1"))),
        (
            "a list over its bound",
            tamper(&|b| b["services"] = json!(vec![b["services"][0].clone(); 501])),
        ),
        (
            "null where a value is required",
            tamper(&|b| b["auto_notify"] = json!(null)),
        ),
    ];
    for (what, reply) in &drifted {
        assert!(
            !violations(&spec, "getSwimlane", reply).is_empty(),
            "not noticed: {what}"
        );
    }
    // A status the operation does not declare, a body with the wrong media type, and a missing declared header.
    let teapot = Reply {
        status: http::StatusCode::IM_A_TEAPOT,
        headers: good.headers.clone(),
        body: good.body.clone(),
    };
    // `default` declares an error body, so a teapot with a swimlane in it is a body that is not a problem.
    assert!(!violations(&spec, "getSwimlane", &teapot).is_empty());
    let mut plain = good.headers.clone();
    plain.insert("content-type", "text/plain".parse().unwrap());
    let wrong_type = Reply {
        status: good.status,
        headers: plain,
        body: good.body.clone(),
    };
    assert!(
        violations(&spec, "getSwimlane", &wrong_type)
            .iter()
            .any(|v| v.contains("content type"))
    );
    let content = h.view("/api/v1/swimlanes/sit1/files/content?path=core.yml").await;
    let mut headers = content.headers.clone();
    headers.remove("etag");
    let untagged = Reply {
        status: content.status,
        headers,
        body: content.body.clone(),
    };
    assert!(
        violations(&spec, "getFileContent", &untagged)
            .iter()
            .any(|v| v.contains("`etag` is missing"))
    );
    // A 304 that carries a body is not noticed by the schema (a 304 declares none), but a 200 that is empty is.
    let empty = Reply {
        status: http::StatusCode::OK,
        headers: good.headers.clone(),
        body: Bytes::new(),
    };
    assert!(!violations(&spec, "getSwimlane", &empty).is_empty());
}

#[tokio::test]
async fn known_deviation_an_unadopted_swimlane_lists_no_tenants_where_the_spec_wants_one() {
    // Spec question for the human (reported with T6): `SwimlaneSummary.tenants` has `minItems: 1`, but a swimlane that has not been
    // adopted has no tenants yet, and `[]` is what the domain says. The service answers `[]`. This test pins exactly that one
    // violation, so that it fails when the spec or the service is changed either way.
    let Some(h) = Http::seeded().await else { return };
    h.reads.world.rig.add_swimlane("sit9", &[], false).await;
    let spec = Spec::load();
    let reply = h.view("/api/v1/swimlanes").await;
    assert_eq!(reply.status, 200);
    let found = violations(&spec, "listSwimlanes", &reply);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("/tenants: fewer than 1 items (0)"), "{found:?}");
    let detail = h.view("/api/v1/swimlanes/sit9").await;
    assert_eq!(violations(&spec, "getSwimlane", &detail).len(), 1);
}
