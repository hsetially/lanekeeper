//! S11, S12, P12 (T6): every query parameter of the fifteen read operations is typed and bounded, as the spec says it is. The probes
//! come from `api/openapi.yaml` itself (each parameter's type, range, length, item bound, enum and `uniqueItems`), so a bound that is
//! declared and not enforced fails here, and a parameter added to the spec has to be given a baseline request. A request over the
//! network edge never reaches a handler with an input that is out of bounds: it is a 400 before any read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation
)]

mod support;

use std::collections::BTreeMap;
use std::fmt::Write;

use serde_json::Value;
use support::http::Http;
use support::openapi::Spec;

type Baseline = (&'static str, Vec<(&'static str, &'static str)>);

/// A request that is valid, for each operation: the path and the query it needs.
fn baselines() -> BTreeMap<&'static str, Baseline> {
    let sw = "/api/v1/swimlanes/sit1";
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    BTreeMap::from([
        ("listSwimlanes", ("/api/v1/swimlanes", vec![])),
        ("getSwimlane", (sw, vec![])),
        ("listSwimlaneTree", (leak(format!("{sw}/tree")), vec![])),
        ("listDrift", (leak(format!("{sw}/drift")), vec![])),
        (
            "getFileContent",
            (leak(format!("{sw}/files/content")), vec![("path", "core.yml")]),
        ),
        (
            "listFileHistory",
            (leak(format!("{sw}/files/history")), vec![("path", "core.yml")]),
        ),
        (
            "compareFile",
            (
                "/api/v1/compare",
                vec![("left", "nfs:sit1"), ("right", "nfs:sit2"), ("file", "core.yml")],
            ),
        ),
        (
            "compareTree",
            (
                "/api/v1/compare/tree",
                vec![("left", "nfs:sit1"), ("right", "nfs:sit2")],
            ),
        ),
        (
            "getGrid",
            (
                "/api/v1/grid",
                vec![("file", "core.yml"), ("swimlanes", "sit1,sit2")],
            ),
        ),
        ("searchSettings", ("/api/v1/search/settings", vec![("q", "port")])),
        (
            "searchText",
            ("/api/v1/search/text", vec![("pattern", "timeout")]),
        ),
        ("listFindings", ("/api/v1/findings", vec![])),
        (
            "getEffectiveConfig",
            (
                leak(format!("{sw}/effective")),
                vec![("file", "core.yml"), ("tenant", "sit1")],
            ),
        ),
        (
            "listPendingRestarts",
            (leak(format!("{sw}/pending-restarts")), vec![]),
        ),
        (
            "getServedFile",
            (
                leak(format!("{sw}/served")),
                vec![("application", "core"), ("tenant", "sit1"), ("file", "core.yml")],
            ),
        ),
    ])
}

/// Percent-encode everything but unreserved characters and the comma.
fn enc(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b',') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn uri(path: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return path.to_owned();
    }
    let query: Vec<String> = params.iter().map(|(n, v)| format!("{n}={}", enc(v))).collect();
    format!("{path}?{}", query.join("&"))
}

/// The baseline with `name` set to `value` (added when the baseline has none).
fn with(base: &[(&'static str, &'static str)], name: &str, value: String) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = base
        .iter()
        .filter(|(n, _)| *n != name)
        .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
        .collect();
    out.push((name.to_owned(), value));
    out
}

async fn status(h: &Http, path: &str, params: &[(String, String)]) -> u16 {
    let borrowed: Vec<(&str, String)> = params.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
    h.view(&uri(path, &borrowed)).await.status.as_u16()
}

#[tokio::test]
async fn every_baseline_is_a_valid_request() {
    let Some(h) = Http::seeded().await else { return };
    for (op, (path, base)) in baselines() {
        let params: Vec<(String, String)> = base
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect();
        let got = status(&h, path, &params).await;
        assert!(matches!(got, 200 | 503), "{op}: {got}");
    }
}

#[tokio::test]
async fn every_declared_parameter_is_typed_and_bounded_as_the_spec_declares() {
    let Some(h) = Http::seeded().await else { return };
    let spec = Spec::load();
    let baselines = baselines();
    let mut probes = 0;
    for (id, method, template) in spec.operation_ids() {
        let Some((path, base)) = baselines.get(id.as_str()) else {
            continue; // not one of the fifteen
        };
        assert_eq!(method, "GET", "{id}");
        let op = spec.operation(&method, &template).unwrap();
        let declared: Vec<_> = spec
            .parameters(op)
            .into_iter()
            .filter(|p| p.location == "query")
            .collect();
        // The baseline uses only declared parameters, and every required one.
        for (name, _) in base {
            assert!(
                declared.iter().any(|p| p.name == *name),
                "{id}: the baseline has `{name}`, the spec does not"
            );
        }
        for p in declared.iter().filter(|p| p.required) {
            assert!(
                base.iter().any(|(n, _)| *n == p.name),
                "{id}: the baseline lacks the required `{}`",
                p.name
            );
            let without: Vec<(String, String)> = base
                .iter()
                .filter(|(n, _)| *n != p.name)
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect();
            assert_eq!(
                status(&h, path, &without).await,
                400,
                "{id}: `{}` is required",
                p.name
            );
            probes += 1;
        }
        // An undeclared parameter, and a declared one twice.
        assert_eq!(
            status(&h, path, &with(base, "unknown_parameter", "1".to_owned())).await,
            400,
            "{id}"
        );
        assert_eq!(
            status(&h, path, &with(base, "x", String::new())).await,
            400,
            "{id}: an empty unknown one"
        );
        if let Some((name, value)) = base.first() {
            let twice = format!(
                "{}&{name}={}",
                uri(path, &[(name, (*value).to_owned())]),
                enc(value)
            );
            assert_eq!(
                h.view(&twice).await.status.as_u16(),
                400,
                "{id}: `{name}` given twice"
            );
            probes += 1;
        }
        probes += 2;

        for p in &declared {
            let schema = spec.deref(&p.schema).clone();
            // `hash`, `repo` and `ref` only mean something with their `source`, so they are probed with it in the baseline.
            let extended: Vec<(&'static str, &'static str)> = match p.name.as_str() {
                "hash" => base.iter().copied().chain([("source", "blob")]).collect(),
                "repo" => base
                    .iter()
                    .copied()
                    .chain([("source", "git"), ("ref", "main")])
                    .collect(),
                "ref" => base
                    .iter()
                    .copied()
                    .chain([("source", "git"), ("repo", "base")])
                    .collect(),
                _ => base.clone(),
            };
            let base = &extended;
            // `refused` are refused whatever the spec says (stricter than it: non-canonical numbers, controls, empties); `by_spec` are
            // refused only if the spec's own schema for the parameter refuses them, so the test cannot disagree with the contract about
            // what an identifier or a path is.
            let mut refused: Vec<String> = Vec::new();
            let mut by_spec: Vec<String> = Vec::new();
            let mut accepted: Vec<String> = Vec::new();
            match schema.get("type").and_then(Value::as_str) {
                Some("integer") => {
                    let min = schema["minimum"].as_i64().unwrap();
                    let max = schema["maximum"].as_i64().unwrap();
                    accepted.extend([min.to_string(), max.to_string()]);
                    refused.extend([
                        (min - 1).to_string(),
                        (max + 1).to_string(),
                        "abc".to_owned(),
                        "-1".to_owned(),
                        "+1".to_owned(),
                        "1.0".to_owned(),
                        "1e2".to_owned(),
                        "0x10".to_owned(),
                        " 1".to_owned(),
                        String::new(),
                        "99999999999999999999".to_owned(),
                    ]);
                }
                Some("boolean") => {
                    accepted.extend(["true".to_owned(), "false".to_owned()]);
                    refused.extend(["yes", "1", "0", "TRUE", "True", "", "t"].map(str::to_owned));
                }
                Some("array") => {
                    let max = schema["maxItems"].as_u64().unwrap() as usize;
                    let item = spec.deref(&schema["items"]).clone();
                    let items = |n: usize| -> Vec<String> {
                        if let Some(values) = item.get("enum").and_then(Value::as_array) {
                            values
                                .iter()
                                .take(n)
                                .map(|v| v.as_str().unwrap().to_owned())
                                .collect()
                        } else {
                            (0..n).map(|i| format!("s{i}")).collect()
                        }
                    };
                    // One more than the bound, an empty item, a repeated item and a bad item. (An enum has fewer values than a
                    // bound that is a little loose, so the over-long list is built from distinct made-up names then.)
                    let over: Vec<String> = (0..=max).map(|i| format!("s{i}")).collect();
                    if item.get("enum").is_none() {
                        by_spec.push(over.join(","));
                    }
                    accepted.push(items(max).join(","));
                    // Structure: an empty item or a repeated one is refused whatever the items are.
                    refused.extend(["a1,,b2", "a1,", ",a1", "a1,a1", ","].map(str::to_owned));
                    by_spec.extend(["A1", "a1,A1", "not-a-kind", "a1,not-a-kind"].map(str::to_owned));
                }
                Some("string") => {
                    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
                        accepted.extend(values.iter().map(|v| v.as_str().unwrap().to_owned()));
                        refused.push(String::new());
                        by_spec.extend(["not-a-value", "IN_SYNC", " untracked"].map(str::to_owned));
                    } else if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
                        let max = max as usize;
                        by_spec.push("a".repeat(max + 1));
                        refused.push(String::new());
                        if schema.get("pattern").is_none() {
                            // A path of exactly the bound: components of 100 characters, which is what a real path looks like.
                            let value = if matches!(p.name.as_str(), "path" | "prefix" | "file") {
                                let mut path = String::new();
                                while path.len() < max {
                                    if !path.is_empty() {
                                        path.push('/');
                                    }
                                    path.push_str(&"a".repeat(100.min(max - path.len().min(max))));
                                }
                                path.truncate(max);
                                path
                            } else {
                                "a".repeat(max)
                            };
                            accepted.push(value);
                        }
                        // Bytes, not characters, are the bound where the type says bytes.
                        if p.name == "q" || p.name == "pattern" {
                            refused.push("é".repeat(max / 2 + 1));
                        }
                        // Controls never get in.
                        refused.push("a\u{0}b".to_owned());
                        refused.push("a\nb".to_owned());
                    }
                    if schema.get("pattern").is_some() {
                        // The alphabets of identifiers and paths: shell, traversal, control and wide characters are refused.
                        by_spec.extend(
                            [
                                "../x",
                                "a b",
                                "a;b",
                                "A",
                                "a\u{0}",
                                "..",
                                "/etc/passwd",
                                "a\\b",
                                "é",
                                "a%2Fb",
                                "~/x",
                            ]
                            .map(str::to_owned),
                        );
                    }
                }
                other => panic!("{id}: `{}` has a type the probes do not know: {other:?}", p.name),
            }
            // What the spec's own schema for the parameter refuses must be refused (a value it allows is not a probe).
            for candidate in by_spec {
                let instance = if schema.get("type").and_then(Value::as_str) == Some("array") {
                    Value::Array(
                        candidate
                            .split(',')
                            .map(|i| Value::String(i.to_owned()))
                            .collect(),
                    )
                } else {
                    Value::String(candidate.clone())
                };
                if !spec.check(&p.schema, &instance).is_empty() {
                    refused.push(candidate);
                }
            }
            for value in refused {
                let got = status(&h, path, &with(base, &p.name, value.clone())).await;
                assert_eq!(got, 400, "{id}: `{}`={value:?} must be refused", p.name);
                probes += 1;
            }
            if p.name == "source" {
                // `blob` and `git` need their own parameters (`hash`; `repo` and `ref`), which the unit tests of `query` and the contract
                // test give them; on its own each is a 400 by design.
                accepted.retain(|v| v == "nfs" || v == "baseline");
            }
            for value in accepted {
                let got = status(&h, path, &with(base, &p.name, value.clone())).await;
                assert_ne!(got, 400, "{id}: `{}`={value:?} is within the bounds", p.name);
                probes += 1;
            }
        }
    }
    assert!(probes > 400, "{probes}");
}

#[tokio::test]
async fn pagination_is_capped_at_five_hundred_and_defaults_to_a_hundred() {
    let Some(h) = Http::seeded().await else { return };
    for uri in [
        "/api/v1/swimlanes",
        "/api/v1/swimlanes/sit1/tree",
        "/api/v1/swimlanes/sit1/drift",
        "/api/v1/swimlanes/sit1/files/history?path=core.yml",
        "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2",
        "/api/v1/grid?file=core.yml&swimlanes=sit1",
        "/api/v1/search/settings?q=port",
        "/api/v1/findings",
        "/api/v1/swimlanes/sit1/pending-restarts",
    ] {
        let sep = if uri.contains('?') { '&' } else { '?' };
        for (limit, want) in [
            ("1", 200),
            ("500", 200),
            ("0", 400),
            ("501", 400),
            ("1000000", 400),
            ("-1", 400),
            ("", 400),
        ] {
            let got = h.view(&format!("{uri}{sep}limit={limit}")).await.status.as_u16();
            assert_eq!(got, want, "{uri} limit={limit:?}");
        }
        assert_eq!(h.view(uri).await.status, 200, "{uri}: no limit is the default");
    }
    // The default really is a hundred: a list of more than 100 items stops at 100 and has a next page.
    let many: Vec<String> = (0..120).map(|i| format!("f{i:03}.txt")).collect();
    let mut h = h;
    let files: Vec<(&str, &str)> = many.iter().map(|f| (f.as_str(), "x\n")).collect();
    h.reads.world.nfs_change(&files).await;
    let page = h.view("/api/v1/swimlanes/sit1/tree").await.json();
    assert_eq!(page["items"].as_array().unwrap().len(), 100);
    assert!(page["next"].is_string());
    let all = h.view("/api/v1/swimlanes/sit1/tree?limit=500").await.json();
    assert!(all["items"].as_array().unwrap().len() > 120);
}

#[tokio::test]
async fn the_raw_query_is_bounded_and_must_be_text() {
    let Some(h) = Http::seeded().await else { return };
    let big = format!("/api/v1/swimlanes?limit=5&x={}", "a".repeat(9 * 1024));
    assert_eq!(h.view(&big).await.status, 400);
    // 100 parameters is not a request any operation takes.
    let many: Vec<String> = (0..100).map(|i| format!("p{i}=1")).collect();
    assert_eq!(
        h.view(&format!("/api/v1/swimlanes?{}", many.join("&")))
            .await
            .status,
        400
    );
    // Not UTF-8 after decoding, and a path made of escapes, are refused, never read.
    for bad in [
        "/api/v1/swimlanes/sit1/files/content?path=%FF%FE",
        "/api/v1/swimlanes/sit1/files/content?path=%C0%AE%C0%AE%2Fetc",
        "/api/v1/swimlanes/sit1/files/content?path=%2e%2e%2fetc%2fpasswd",
        "/api/v1/swimlanes/sit1/files/content?path=core.yml%00.txt",
        "/api/v1/swimlanes/sit1/files/content?path=a%5C..%5Cb",
        "/api/v1/swimlanes/sit1/tree?prefix=%2e%2e",
        "/api/v1/swimlanes/sit1/tree?prefix=%2Fetc",
    ] {
        let reply = h.view(bad).await;
        assert_eq!(reply.status, 400, "{bad}: {}", reply.text());
        assert!(
            !reply.text().contains("passwd") && !reply.text().contains("etc"),
            "{bad}"
        );
    }
    // The path parameter: an id that is not an id never reaches the database.
    for bad in [
        "/api/v1/swimlanes/%00",
        "/api/v1/swimlanes/..",
        "/api/v1/swimlanes/a%2Fb",
        "/api/v1/swimlanes/%FF",
    ] {
        let status = h.view(bad).await.status.as_u16();
        assert!(matches!(status, 400 | 404), "{bad}: {status}");
    }
}
