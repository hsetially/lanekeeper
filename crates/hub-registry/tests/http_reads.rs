//! The read API over HTTP (T6, S4, S11, S12, S21, A27, A30, A31): what a client gets back, headers and bodies, from the real router over
//! the real service and a real Postgres. The shape of every body is walked against the contract in `http_contract`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;

use bytes::Bytes;
use domain::{AgentReply, HubCommand, SwimlaneId};
use serde_json::Value;
use support::http::{Http, Reply, SECRET};

const CONTENT: &str = "/api/v1/swimlanes/sit1/files/content?path=core.yml";
const EFFECTIVE: &str = "/api/v1/swimlanes/sit1/effective?file=core.yml&tenant=sit1";
const COMPARE: &str = "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=core.yml";
const SERVED: &str = "/api/v1/swimlanes/sit1/served?application=core&tenant=sit1&file=core.yml";

/// An agent for `sit1` that serves a fixed answer and remembers nothing; `h.reads.gateway.received` says what it was asked.
fn serve(h: &Http, status: u16, body: &'static str) {
    h.reads.gateway.attach(
        &SwimlaneId::parse("sit1").unwrap(),
        Arc::new(move |cmd: &HubCommand| match cmd {
            HubCommand::FetchServed { request_id, .. } => Some(AgentReply::Served {
                request_id: request_id.clone(),
                status,
                bytes: Bytes::from_static(body.as_bytes()),
            }),
            _ => None,
        }),
    );
}

fn items(reply: &Reply) -> Vec<Value> {
    reply.json()["items"].as_array().unwrap().clone()
}

// ------------------------------------------------------------------------------------------------ entity tags

#[tokio::test]
async fn the_four_content_reads_have_an_etag_and_a_matching_if_none_match_is_a_304_without_a_body() {
    let Some(h) = Http::seeded().await else { return };
    serve(&h, 200, "port: 2\n");
    for uri in [CONTENT, EFFECTIVE, COMPARE, SERVED] {
        let first = h.view(uri).await;
        assert_eq!(first.status, 200, "{uri}: {}", first.text());
        let etag = first
            .header("etag")
            .unwrap_or_else(|| panic!("{uri} has no ETag"))
            .to_owned();
        assert!(
            etag.starts_with('"') && etag.ends_with('"'),
            "{uri}: a strong tag: {etag}"
        );
        assert_eq!(first.header("content-type"), Some("application/json"), "{uri}");
        assert_eq!(first.header("cache-control"), Some("private, no-cache"), "{uri}");
        assert_eq!(first.header("x-content-type-options"), Some("nosniff"), "{uri}");

        let same = h.get_with(Some("viewer"), uri, &[("if-none-match", &etag)]).await;
        assert_eq!(same.status, 304, "{uri}");
        assert!(same.body.is_empty(), "{uri}: no body on a 304");
        assert_eq!(
            same.header("etag"),
            Some(etag.as_str()),
            "{uri}: the 304 repeats the tag"
        );

        let weak = h
            .get_with(Some("viewer"), uri, &[("if-none-match", &format!("W/{etag}"))])
            .await;
        assert_eq!(weak.status, 304, "{uri}: a weak comparison");
        let list = h
            .get_with(
                Some("viewer"),
                uri,
                &[("if-none-match", &format!("\"x\", {etag}"))],
            )
            .await;
        assert_eq!(list.status, 304, "{uri}: a list of tags");
        let star = h.get_with(Some("viewer"), uri, &[("if-none-match", "*")]).await;
        assert_eq!(star.status, 304, "{uri}: any current representation");
        let other = h
            .get_with(Some("viewer"), uri, &[("if-none-match", "\"not-this-one\"")])
            .await;
        assert_eq!(other.status, 200, "{uri}: another tag is a full answer");
        let long = h
            .get_with(Some("viewer"), uri, &[("if-none-match", &"a".repeat(201))])
            .await;
        assert_eq!(
            long.status, 200,
            "{uri}: an If-None-Match over the contract's 200 bytes is ignored, not an error"
        );

        // A 304 is still authorised: the guard comes first.
        let refused = h
            .get_with(Some("pending"), uri, &[("if-none-match", &etag)])
            .await;
        assert_eq!(refused.status, 403, "{uri}");
        let anonymous = h.get_with(None, uri, &[("if-none-match", &etag)]).await;
        assert_eq!(anonymous.status, 401, "{uri}");
    }
}

#[tokio::test]
async fn a_changed_file_has_another_tag_and_the_old_one_no_longer_matches() {
    let Some(mut h) = Http::seeded().await else { return };
    let before = h.view(CONTENT).await;
    let old = before.header("etag").unwrap().to_owned();
    h.reads
        .world
        .nfs_change(&[("core.yml", "port: 9\nname: core\n")])
        .await;
    let after = h.view(CONTENT).await;
    assert_eq!(after.status, 200);
    assert_ne!(after.header("etag").unwrap(), old);
    let stale = h
        .get_with(Some("viewer"), CONTENT, &[("if-none-match", &old)])
        .await;
    assert_eq!(stale.status, 200, "the old tag is a new answer now");
    assert!(stale.text().contains("port: 9"));
}

#[tokio::test]
async fn lists_and_searches_carry_no_tag_are_not_stored_and_ignore_if_none_match() {
    let Some(h) = Http::seeded().await else { return };
    for uri in [
        "/api/v1/swimlanes",
        "/api/v1/swimlanes/sit1",
        "/api/v1/swimlanes/sit1/tree",
        "/api/v1/swimlanes/sit1/drift",
        "/api/v1/swimlanes/sit1/files/history?path=core.yml",
        "/api/v1/swimlanes/sit1/pending-restarts",
        "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2",
        "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2",
        "/api/v1/search/settings?q=port",
        "/api/v1/search/text?pattern=timeout",
        "/api/v1/findings",
    ] {
        let reply = h.get_with(Some("viewer"), uri, &[("if-none-match", "*")]).await;
        assert_eq!(reply.status, 200, "{uri}: {}", reply.text());
        assert!(
            reply.header("etag").is_none(),
            "{uri}: the contract declares no ETag here"
        );
        assert_eq!(reply.header("cache-control"), Some("private, no-store"), "{uri}");
        assert_eq!(reply.header("content-type"), Some("application/json"), "{uri}");
    }
}

// ------------------------------------------------------------------------------------------------ tenants (A30)

#[tokio::test]
async fn the_answer_names_the_tenant_that_stood_for_each_swimlane() {
    let Some(h) = Http::seeded().await else { return };
    // `sit1` deploys `sit1` and `sit2`; the first in sorted order stands for it.
    let grid = h
        .view("/api/v1/grid?file=core.yml&swimlanes=sit1,sit2,sit3")
        .await;
    assert_eq!(grid.status, 200, "{}", grid.text());
    assert_eq!(
        grid.header("x-lanekeeper-tenants"),
        Some("sit1=sit1,sit2=sit2,sit3=sit3")
    );
    let compare = h.view(COMPARE).await;
    assert_eq!(
        compare.header("x-lanekeeper-tenants"),
        Some("sit1=sit1,sit2=sit2")
    );
    let tree = h
        .view("/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit3")
        .await;
    assert_eq!(tree.header("x-lanekeeper-tenants"), Some("sit1=sit1,sit3=sit3"));
    // Effective sides of a whole-tree compare have no tenant to name; the header is absent, not empty.
    let eff = h
        .view("/api/v1/compare/tree?left=effective%3Asit1&right=effective%3Asit3")
        .await;
    assert_eq!(eff.status, 200, "{}", eff.text());
    assert!(eff.header("x-lanekeeper-tenants").is_none());
    // A read that has no tenant choice does not carry it.
    assert!(h.view(CONTENT).await.header("x-lanekeeper-tenants").is_none());
}

// ------------------------------------------------------------------------------------------------ errors (S21)

#[tokio::test]
async fn an_error_is_a_problem_with_a_code_and_nothing_of_the_input_or_the_server() {
    let Some(h) = Http::seeded().await else { return };
    let cases: Vec<(&str, u16, &str)> = vec![
        ("/api/v1/swimlanes/nope", 404, "not_found"),
        ("/api/v1/swimlanes/NOPE", 400, "bad_request"),
        (
            "/api/v1/swimlanes/sit1/files/content?path=missing.yml",
            404,
            "not_found",
        ),
        (
            "/api/v1/swimlanes/sit1/files/content?path=..%2F..%2Fetc%2Fpasswd",
            400,
            "bad_request",
        ),
        ("/api/v1/swimlanes/sit1/files/content", 400, "bad_request"),
        (
            "/api/v1/swimlanes/sit1/files/content?path=core.yml&unknown=1",
            400,
            "bad_request",
        ),
        (
            "/api/v1/swimlanes/sit1/effective?file=core.yml&tenant=sit9",
            404,
            "not_found",
        ),
        (
            "/api/v1/swimlanes/sit1/effective?file=core.yml",
            400,
            "bad_request",
        ),
        ("/api/v1/swimlanes/nope/tree", 404, "not_found"),
        (
            "/api/v1/compare?left=nfs%3Anope&right=nfs%3Asit2&file=core.yml",
            404,
            "not_found",
        ),
        ("/api/v1/grid?file=core.yml&swimlanes=sit1,nope", 404, "not_found"),
        (
            "/api/v1/search/text?pattern=%28unclosed&regex=true",
            400,
            "bad_request",
        ),
        ("/api/v1/findings?kinds=nope", 400, "bad_request"),
        ("/api/v1/swimlanes?limit=0", 400, "bad_request"),
    ];
    for (uri, status, code) in cases {
        let reply = h.view(uri).await;
        assert_eq!(reply.status.as_u16(), status, "{uri}: {}", reply.text());
        assert_eq!(
            reply.header("content-type"),
            Some("application/problem+json"),
            "{uri}"
        );
        let body = reply.json();
        assert_eq!(body["code"], code, "{uri}");
        assert_eq!(body["status"], status, "{uri}");
        let keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(
            keys.iter()
                .all(|k| ["type", "title", "status", "code", "request_id"].contains(k)),
            "{uri}: only the problem's own fields: {keys:?}"
        );
        let text = reply.text();
        for leak in [
            "passwd", "unknown", "unclosed", "SELECT", "postgres", "sqlx", "/home/", "panicked",
        ] {
            assert!(!text.contains(leak), "{uri}: the problem leaks {leak}: {text}");
        }
    }
}

// ------------------------------------------------------------------------------------------------ denied files (D79, S11)

#[tokio::test]
async fn a_denied_file_never_returns_content_through_any_route() {
    let Some(h) = Http::seeded().await else { return };
    serve(&h, 200, "-----BEGIN PRIVATE KEY-----\nMIIB-planted\n");
    let planted = ["MIIB-planted", "PRIVATE KEY"];

    let content = h
        .view("/api/v1/swimlanes/sit1/files/content?path=keys%2Fserver.pem")
        .await;
    assert_eq!(content.status, 200, "{}", content.text());
    let body = content.json();
    assert_eq!(body["content_withheld"], true);
    assert!(body["text"].is_null());

    let served = h
        .view("/api/v1/swimlanes/sit1/served?application=core&tenant=sit1&file=keys%2Fserver.pem")
        .await;
    assert_eq!(served.status, 200, "{}", served.text());
    assert_eq!(served.json()["content_withheld"], true);
    assert!(
        h.reads
            .gateway
            .received(&SwimlaneId::parse("sit1").unwrap())
            .iter()
            .all(|c| !matches!(c, HubCommand::FetchServed { .. })),
        "a denied file is not even asked for"
    );

    let compare = h
        .view("/api/v1/compare?left=nfs%3Asit1&right=baseline%3Asit1&file=keys%2Fserver.pem")
        .await;
    let effective = h
        .view("/api/v1/swimlanes/sit1/effective?file=keys%2Fserver.pem&tenant=sit1")
        .await;
    assert_eq!(effective.status, 404, "a denied file has no effective config");
    let text = h.view("/api/v1/search/text?pattern=planted").await;
    assert_eq!(text.status, 200);
    assert!(
        text.json()["hits"].as_array().unwrap().is_empty(),
        "denied files are not searched"
    );
    let settings = h.view("/api/v1/search/settings?q=BEGIN").await;
    let history = h
        .view("/api/v1/swimlanes/sit1/files/history?path=keys%2Fserver.pem")
        .await;
    for (name, reply) in [
        ("content", &content),
        ("served", &served),
        ("compare", &compare),
        ("effective", &effective),
        ("text", &text),
        ("settings", &settings),
        ("history", &history),
    ] {
        let text = reply.text();
        for p in planted {
            assert!(
                !text.contains(p),
                "{name} returned the denied file's content: {text}"
            );
        }
    }
}

// ------------------------------------------------------------------------------------------------ pages and cursors (A31)

#[tokio::test]
async fn a_list_pages_by_cursor_and_the_pages_are_the_whole_list() {
    let Some(h) = Http::seeded().await else { return };
    let whole: Vec<String> = items(&h.view("/api/v1/swimlanes/sit1/tree?limit=500").await)
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_owned())
        .collect();
    assert!(whole.len() >= 4, "{whole:?}");
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..50 {
        let uri = match &cursor {
            None => "/api/v1/swimlanes/sit1/tree?limit=1".to_owned(),
            Some(c) => format!("/api/v1/swimlanes/sit1/tree?limit=1&cursor={c}"),
        };
        let page = h.view(&uri).await;
        assert_eq!(page.status, 200, "{}", page.text());
        let body = page.json();
        let got = body["items"].as_array().unwrap();
        assert_eq!(got.len(), 1);
        seen.push(got[0]["path"].as_str().unwrap().to_owned());
        match body["next"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!(seen, whole);
}

#[tokio::test]
async fn a_cursor_is_for_the_query_that_made_it_and_cannot_reach_another_swimlanes_data() {
    let Some(h) = Http::seeded().await else { return };
    let page = h.view("/api/v1/swimlanes/sit1/tree?limit=1").await;
    let cursor = page.json()["next"].as_str().unwrap().to_owned();
    assert!(
        cursor.len() <= 512
            && cursor
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-=.".contains(&b))
    );
    // Another swimlane, another list, another folder, another sort of listing: all refused.
    for uri in [
        format!("/api/v1/swimlanes/sit2/tree?limit=1&cursor={cursor}"),
        format!("/api/v1/swimlanes/sit1/drift?limit=1&cursor={cursor}"),
        format!("/api/v1/swimlanes/sit1/tree?limit=1&prefix=keys&cursor={cursor}"),
        format!("/api/v1/swimlanes/sit1/tree?limit=1&drift=in_sync&cursor={cursor}"),
        format!("/api/v1/swimlanes/sit1/files/history?path=core.yml&cursor={cursor}"),
        format!("/api/v1/findings?cursor={cursor}"),
        format!("/api/v1/swimlanes?cursor={cursor}"),
        format!("/api/v1/search/settings?q=port&cursor={cursor}"),
        format!("/api/v1/grid?file=core.yml&swimlanes=sit1,sit2&cursor={cursor}"),
    ] {
        let reply = h.view(&uri).await;
        assert_eq!(reply.status, 400, "{uri}: {}", reply.text());
        assert_eq!(reply.code(), "bad_request");
    }
    // Garbage and over-long cursors are refused before anything is read.
    for cursor in ["not-a-cursor", "AAAA", &"A".repeat(512), &"A".repeat(513)] {
        let reply = h
            .view(&format!("/api/v1/swimlanes/sit1/tree?cursor={cursor}"))
            .await;
        assert_eq!(reply.status, 400, "{cursor}");
    }
    // The cursor does work for the query that made it.
    let again = h
        .view(&format!("/api/v1/swimlanes/sit1/tree?limit=1&cursor={cursor}"))
        .await;
    assert_eq!(again.status, 200);
}

// ------------------------------------------------------------------------------------------------ the served file

#[tokio::test]
async fn the_served_file_is_the_agents_answer_with_the_engines_role_and_the_placeholders_left() {
    let Some(h) = Http::seeded().await else { return };
    // No agent attached: the swimlane's agent is not there.
    let down = h.view(SERVED).await;
    assert_eq!(down.status, 503, "{}", down.text());
    assert_eq!(down.code(), "unavailable");

    serve(&h, 200, "port: 2\nname: ${NOT_THERE}\n");
    let ok = h.view(SERVED).await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    let body = ok.json();
    assert_eq!(body["content_withheld"], false);
    assert!(body["text"].as_str().unwrap().contains("port: 2"));
    assert_eq!(body["unresolved_placeholders"][0], "NOT_THERE");
    assert_eq!(body["tenant"], "sit1");
    assert_eq!(body["application"], "core");

    serve(&h, 404, "");
    assert_eq!(
        h.view(SERVED).await.status,
        404,
        "the config-server's own 404 is a 404"
    );
    serve(&h, 500, "boom");
    let broken = h.view(SERVED).await;
    assert_eq!(broken.status, 503);
    assert!(
        !broken.text().contains("boom"),
        "what the config-server said is not passed on"
    );
    // A tenant the swimlane does not deploy is not a request the config-server could answer for it.
    let unknown = h
        .view("/api/v1/swimlanes/sit1/served?application=core&tenant=sit9&file=core.yml")
        .await;
    assert_eq!(unknown.status, 404);
}

// ------------------------------------------------------------------------------------------------ masking (A28, S13)

#[tokio::test]
async fn a_flagged_value_is_masked_in_every_setting_surface() {
    let Some(h) = Http::seeded().await else { return };
    for (name, uri) in [
        ("grid", "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2"),
        ("effective", EFFECTIVE),
        ("settings search", "/api/v1/search/settings?q=token"),
        ("compare", COMPARE),
    ] {
        let reply = h.view(uri).await;
        assert_eq!(reply.status, 200, "{name}: {}", reply.text());
        let text = reply.text();
        assert!(!text.contains(SECRET), "{name} shows the secret: {text}");
        if name != "compare" {
            assert!(text.contains("[REDACTED: "), "{name} masks it: {text}");
        }
    }
    // Two swimlanes that hold *different* secrets of the same length and rule are still two different values for the compare.
    let grid = h
        .view("/api/v1/grid?file=core.yml&swimlanes=sit1,sit2&only_differing=true")
        .await;
    let paths: Vec<String> = items_of(&grid.json()["rows"])
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        paths.contains(&"token".to_owned()),
        "a secret in one swimlane and none in the other differs: {paths:?}"
    );
}

fn items_of(page: &Value) -> Vec<Value> {
    page["items"].as_array().unwrap().clone()
}
