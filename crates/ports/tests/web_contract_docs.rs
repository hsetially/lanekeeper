//! contract/08-web, `CG11`: the sanitised HTML of a doc (`renderDoc`, `RenderedDoc`, `DocSearch::render`) and the optional
//! `anchor` of `DocHit` and `FlagStatus`.
//!
//! This is the one HTML sink the CSP lets through (S12), so the allowlist is a stated requirement. It is held in three
//! places that must agree: the description of `RenderedDoc.html` in `api/openapi.yaml`, the checker
//! (`ports::conformance::html_violations`) and the corpus of attacks (`ports::conformance::XSS_CORPUS`) that every
//! `DocSearch` implementation renders in `ports::conformance::doc_search`. The tests below pin the document to the domain,
//! the document to the checker, the checker to a table of pages it must refuse, and the conformance suite to a set of
//! implementations that each break one rule (a suite that nothing can fail proves nothing).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    // The table of pages the checker must refuse is one list of data, and splitting it would hide that each row is alone.
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use async_trait::async_trait;
use domain::{
    ContentHash, DocGrepHit, DocHeading, DocHit, DocId, DocPath, DocText, FeatureQuery, FeatureStatus,
    FlagStatus, LineRange, LogicalFile, RenderedDoc, SettingPath, ShortText, User,
};
use ports::conformance::{
    self, ALLOWED_HTML_TAGS, DocScenario, XSS_CORPUS, html_violations, is_heading_id, rendered_doc_violations,
};
use ports::fakes::FakeDocSearch;
use ports::{
    DocError, DocSearch, HEADING_ID_PATTERN, MAX_DOC_HEADINGS, MAX_HEADING_ID_BYTES, MAX_RENDER_SOURCE_BYTES,
    MAX_RENDERED_HTML_BYTES, content_hash,
};
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

fn wire_keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
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

/// The backticked words of `text`, in order.
fn backticked(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

fn sample_rendered() -> RenderedDoc {
    RenderedDoc {
        path: DocPath::parse("/git/main/guides/alpha.md").unwrap(),
        html: "<h1 id=\"doc-alpha\">Alpha</h1>\n<p>text</p>\n".to_owned(),
        headings: vec![DocHeading {
            id: ShortText::parse("doc-alpha").unwrap(),
            level: 1,
            text: ShortText::parse("Alpha").unwrap(),
            line: 1,
        }],
        content_hash: ContentHash::from_bytes([0xab; 32]),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The document

#[test]
fn render_doc_is_a_cacheable_content_read_with_bounded_headings() {
    let d = doc();
    let op = &d["paths"]["/api/v1/docs/fs/render"]["get"];
    assert!(
        op.is_object(),
        "GET /api/v1/docs/fs/render is not in the document"
    );
    assert_eq!(op["operationId"], "renderDoc");
    assert_eq!(
        op["x-required-role"], "viewer",
        "it mirrors readDoc: whoever may read a doc may read its page"
    );
    assert_eq!(
        strings(&op["tags"]),
        ["content", "docs"].map(String::from).into(),
        "`content` brings If-None-Match, the ETag and the body-less 304 (content_endpoints_have_etag)"
    );

    let params: Vec<&Value> = op["parameters"]
        .as_array()
        .expect("parameters")
        .iter()
        .map(|p| resolve(&d, p))
        .collect();
    let named = |n: &str| {
        params
            .iter()
            .find(|p| p["name"] == n)
            .unwrap_or_else(|| panic!("no parameter `{n}`"))
    };
    let path = named("path");
    assert_eq!(path["in"], "query");
    assert_eq!(path["required"], true);
    let path_schema = op["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| resolve(&d, p)["name"] == "path")
        .unwrap();
    assert_eq!(
        ref_name(&resolve(&d, path_schema)["schema"]),
        "DocPath",
        "a path, like readDoc"
    );
    assert_eq!(named("If-None-Match")["in"], "header");
    assert_eq!(params.len(), 2, "path and If-None-Match, nothing else");

    let ok = resolve(&d, &op["responses"]["200"]);
    assert_eq!(
        ref_name(&ok["content"]["application/json"]["schema"]),
        "RenderedDoc"
    );
    assert!(ok["headers"].get("ETag").is_some(), "200 sends an ETag");
    let not_modified = resolve(&d, &op["responses"]["304"]);
    assert!(not_modified.get("content").is_none(), "304 has no body");
    assert!(
        not_modified["headers"].get("ETag").is_some(),
        "304 repeats the ETag"
    );
    for code in ["400", "401", "403", "404", "413", "default"] {
        let r = resolve(&d, &op["responses"][code]);
        assert_eq!(
            r["content"]
                .as_object()
                .unwrap_or_else(|| panic!("{code} has no body"))
                .keys()
                .collect::<Vec<_>>(),
            ["application/problem+json"],
            "{code}"
        );
    }
}

#[test]
fn rendered_doc_is_the_domains_and_every_field_is_bounded() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let json = serde_json::to_value(sample_rendered()).unwrap();
    let rd = &schemas["RenderedDoc"];
    assert!(rd.is_object(), "RenderedDoc is not in the document");
    assert_eq!(rd["additionalProperties"], false);
    assert_eq!(
        declared_of(rd),
        wire_keys(&json),
        "the properties are the keys the domain writes"
    );
    assert_eq!(required_of(rd), wire_keys(&json), "every field is required");
    assert_eq!(describe(&rd["properties"]["path"]), "ref:DocPath");
    assert_eq!(describe(&rd["properties"]["content_hash"]), "ref:ContentHash");
    let html = &rd["properties"]["html"];
    assert_eq!(html["type"], "string");
    assert_eq!(
        html["maxLength"].as_u64(),
        Some(MAX_RENDERED_HTML_BYTES as u64),
        "the page is bounded (S11): a longer render is refused with 413, never cut"
    );
    let headings = &rd["properties"]["headings"];
    assert_eq!(describe(headings), "array<ref:DocHeading>");
    assert_eq!(headings["maxItems"].as_u64(), Some(MAX_DOC_HEADINGS as u64));

    let heading = &schemas["DocHeading"];
    assert!(heading.is_object(), "DocHeading is not in the document");
    let json = serde_json::to_value(&sample_rendered().headings[0]).unwrap();
    assert_eq!(heading["additionalProperties"], false);
    assert_eq!(declared_of(heading), wire_keys(&json));
    assert_eq!(required_of(heading), wire_keys(&json));
    let id = &heading["properties"]["id"];
    assert_eq!(id["type"], "string");
    assert_eq!(
        id["pattern"], HEADING_ID_PATTERN,
        "the server-chosen prefix and the shape of a heading id are the checker's"
    );
    assert_eq!(id["maxLength"].as_u64(), Some(MAX_HEADING_ID_BYTES as u64));
    assert_eq!(describe(&heading["properties"]["text"]), "ref:ShortText");
    let level = &heading["properties"]["level"];
    assert_eq!(
        (level["minimum"].as_u64(), level["maximum"].as_u64()),
        (Some(1), Some(6))
    );
    assert_eq!(heading["properties"]["line"]["minimum"].as_u64(), Some(1));
}

/// The allowlist is a requirement stated in the document, and the document states the one the checker enforces.
#[test]
fn the_allowlist_is_stated_in_the_schema_and_is_the_one_the_checker_enforces() {
    let d = doc();
    let text = d["components"]["schemas"]["RenderedDoc"]["properties"]["html"]["description"]
        .as_str()
        .expect("RenderedDoc.html has a description that states the allowlist");

    let elements = text
        .split_once("Elements:")
        .and_then(|(_, rest)| rest.split_once("Attributes:"))
        .expect("the description has an `Elements:` part followed by an `Attributes:` part")
        .0;
    let listed: BTreeSet<&str> = backticked(elements).into_iter().collect();
    let enforced: BTreeSet<&str> = ALLOWED_HTML_TAGS.iter().copied().collect();
    assert_eq!(
        listed, enforced,
        "the elements the description allows are the elements the checker allows"
    );
    assert_eq!(enforced.len(), ALLOWED_HTML_TAGS.len(), "no tag twice");
    for must_not in [
        "script", "style", "iframe", "svg", "form", "object", "embed", "link", "meta", "base",
    ] {
        assert!(!enforced.contains(must_not), "{must_not} is never allowed");
    }

    for needle in [
        "`id` on `h1` to `h6` only",
        "`doc-`",
        "`href` and `rel` on `a`",
        "`src` and `alt` on `img`",
        "Nothing else",
        "no `style`",
        "no event handler",
        "`rel=\"noopener noreferrer\"`",
        "absolute `https:` URL without credentials",
        "never `//`",
        "`/docs/view?path=",
        "`javascript:`",
        "`data:`",
        "raw HTML in the Markdown source is data",
        "No comment",
        "never inside an `a`",
        "Trusted Types policy `lanekeeper-docs`",
    ] {
        assert!(
            text.contains(needle),
            "the allowlist text must say `{needle}`; a reword that drops a rule is a deliberate act"
        );
    }
    assert!(
        text.contains(HEADING_ID_PATTERN) || text.contains("lower case letters and digits"),
        "the id shape is stated"
    );
}

/// The pipeline requirements are in the operation, because they are what the owner of the docs index implements.
#[test]
fn render_doc_states_the_pipeline_it_requires() {
    let d = doc();
    let text = d["paths"]["/api/v1/docs/fs/render"]["get"]["description"]
        .as_str()
        .expect("renderDoc has a description");
    for needle in [
        "secret scan",
        "before the Markdown is parsed",
        "[REDACTED: N chars, rule]",
        "no reveal for docs",
        "413",
        "never truncates",
        "spawn_blocking",
        "cached by content hash",
        "scan configuration",
        "ETag",
        "Trusted Types policy `lanekeeper-docs`",
    ] {
        assert!(
            text.contains(needle),
            "renderDoc must say `{needle}`; a reword that drops a requirement is a deliberate act"
        );
    }
}

#[test]
fn doc_anchor_additions_are_optional() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let hit = |anchor: Option<&str>| DocHit {
        doc: DocId::from(1),
        title: ShortText::parse("Alpha").unwrap(),
        path: DocPath::parse("/git/main/guides/alpha.md").unwrap(),
        heading_path: vec![ShortText::parse("Alpha").unwrap()],
        snippet: "line".to_owned(),
        score: 0.5,
        related_files: vec![LogicalFile::parse("tx-api/core.yml").unwrap()],
        anchor: anchor.map(|a| ShortText::parse(a).unwrap()),
    };
    let flag = |anchor: Option<&str>| FlagStatus {
        flag: ShortText::parse("enableAccountSorting").unwrap(),
        file: LogicalFile::parse("tx-api/core.yml").unwrap(),
        channel: None,
        setting: SettingPath::parse("feature.sorting").unwrap(),
        template_default: None,
        values: Vec::new(),
        anchor: anchor.map(|a| ShortText::parse(a).unwrap()),
    };
    let cases = [
        (
            "DocHit",
            serde_json::to_value(hit(None)).unwrap(),
            serde_json::to_value(hit(Some("doc-alpha"))).unwrap(),
            &[
                "doc",
                "title",
                "path",
                "heading_path",
                "snippet",
                "score",
                "related_files",
            ][..],
        ),
        (
            "FlagStatus",
            serde_json::to_value(flag(None)).unwrap(),
            serde_json::to_value(flag(Some("doc-alpha"))).unwrap(),
            &["flag", "file", "channel", "setting", "template_default", "values"][..],
        ),
    ];
    for (name, without, with, was_required) in cases {
        let schema = &schemas[name];
        assert_eq!(schema["additionalProperties"], false, "{name}");
        let anchor = &schema["properties"]["anchor"];
        assert!(anchor.is_object(), "{name} has no `anchor`");
        assert_eq!(describe(anchor), "ref:ShortText|null", "{name}.anchor");
        assert_eq!(
            required_of(schema),
            was_required.iter().map(|k| (*k).to_owned()).collect(),
            "{name}: `anchor` is optional and nothing that was required changed"
        );
        // The domain writes the key only when it has one, so an old document and an old mock stay valid.
        assert!(
            !wire_keys(&without).contains("anchor"),
            "{name}: an absent anchor is not written"
        );
        let mut with_keys = wire_keys(&without);
        with_keys.insert("anchor".to_owned());
        assert_eq!(wire_keys(&with), with_keys);
        assert_eq!(
            declared_of(schema),
            with_keys,
            "{name}: the properties are the keys the domain writes"
        );
        assert_eq!(with["anchor"], "doc-alpha");
    }
    // Old JSON, with no `anchor` and with a null one, reads back with none.
    let old: DocHit = serde_json::from_value(serde_json::to_value(hit(None)).unwrap()).unwrap();
    assert_eq!(old.anchor, None);
    let mut with_null = serde_json::to_value(hit(None)).unwrap();
    with_null["anchor"] = Value::Null;
    assert_eq!(serde_json::from_value::<DocHit>(with_null).unwrap().anchor, None);
    let old: FlagStatus = serde_json::from_value(serde_json::to_value(flag(None)).unwrap()).unwrap();
    assert_eq!(old.anchor, None);
}

#[test]
fn a_rendered_doc_round_trips_and_its_debug_prints_sizes_only() {
    let sample = sample_rendered();
    let json = serde_json::to_string(&sample).unwrap();
    assert_eq!(serde_json::from_str::<RenderedDoc>(&json).unwrap(), sample);
    let shown = format!("{sample:?}");
    assert!(
        !shown.contains("Alpha") && !shown.contains("<p>"),
        "Debug shows the page: {shown}"
    );
    assert!(
        shown.contains(&sample.html.len().to_string()),
        "Debug names the size of the page: {shown}"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The checker

#[test]
fn a_page_inside_the_allowlist_has_no_violation() {
    let page = "<h1 id=\"doc-title\">Title</h1>\n<p>Some <em>text</em> and <strong>more</strong>, a &lt;b&gt; &amp; \
                <code>x</code><br />\n<a href=\"https://docs.example/a?x=1&amp;y=2\" rel=\"noopener noreferrer\">out</a> \
                <a href=\"/docs/view?path=%2Fgit%2Fmain%2Fa.md\" rel=\"noopener noreferrer\">in</a> \
                <a href=\"#doc-title\" rel=\"noopener noreferrer\">up</a> <img src=\"/a.png\" alt=\"a &quot;b&quot;\"></p>\n\
                <hr>\n<blockquote><p>q</p></blockquote>\n<pre><code>1 &lt; 2\n</code></pre>\n\
                <ul><li>a</li><li>b</li></ul><ol><li>c</li></ol>\n\
                <table><thead><tr><th>a</th></tr></thead><tbody><tr><td>b</td></tr></tbody></table>\n";
    assert_eq!(html_violations(page), Vec::<String>::new());
    assert!(html_violations("").is_empty());
    assert!(html_violations("just text, with no markup at all").is_empty());
    assert!(
        is_heading_id("doc-a") && is_heading_id("doc-a-2") && !is_heading_id("a") && !is_heading_id("doc-")
    );
}

/// Every page below leaves the allowlist in the way its label says; the checker must refuse each one.
#[test]
fn the_checker_refuses_every_way_out_of_the_allowlist() {
    let a = |href: &str| format!("<a href=\"{href}\" rel=\"noopener noreferrer\">x</a>");
    let cases: Vec<(&str, String)> = vec![
        ("script element", "<script>alert(1)</script>".into()),
        ("script in upper case", "<SCRIPT>alert(1)</SCRIPT>".into()),
        ("style element", "<style>p{}</style>".into()),
        ("iframe", "<iframe src=\"/x\"></iframe>".into()),
        ("svg", "<svg onload=\"x\"></svg>".into()),
        ("form and input", "<form><input name=\"p\"></form>".into()),
        ("object", "<object data=\"/x\"></object>".into()),
        ("details", "<details><summary>x</summary></details>".into()),
        ("base", "<base href=\"https://evil.example/\">".into()),
        (
            "meta refresh",
            "<meta http-equiv=\"refresh\" content=\"0\">".into(),
        ),
        ("comment", "<p>x</p><!-- x -->".into()),
        ("conditional comment", "<!--[if IE]><p>x</p><![endif]-->".into()),
        ("doctype", "<!DOCTYPE html>".into()),
        ("cdata", "<![CDATA[x]]>".into()),
        ("processing instruction", "<?php echo 1 ?>".into()),
        ("event handler", "<p onclick=\"alert(1)\">x</p>".into()),
        (
            "event handler on an image",
            "<img src=\"/a.png\" onerror=\"alert(1)\">".into(),
        ),
        ("style attribute", "<p style=\"color:red\">x</p>".into()),
        ("class attribute", "<p class=\"x\">x</p>".into()),
        (
            "target attribute",
            "<a href=\"https://d.example/\" rel=\"noopener noreferrer\" target=\"_blank\">x</a>".into(),
        ),
        ("srcset", "<img src=\"/a.png\" srcset=\"/b.png 2x\">".into()),
        ("data attribute", "<p data-x=\"1\">x</p>".into()),
        ("id on a paragraph", "<p id=\"doc-x\">x</p>".into()),
        ("id with no prefix", "<h1 id=\"admin\">x</h1>".into()),
        ("id with upper case", "<h1 id=\"doc-Admin\">x</h1>".into()),
        ("id with an underscore", "<h1 id=\"doc-__proto__\">x</h1>".into()),
        ("id with an empty slug", "<h1 id=\"doc-\">x</h1>".into()),
        (
            "id twice in a page",
            "<h1 id=\"doc-x\">a</h1><h2 id=\"doc-x\">b</h2>".into(),
        ),
        (
            "the same attribute twice",
            "<h1 id=\"doc-x\" id=\"doc-y\">x</h1>".into(),
        ),
        ("javascript link", a("javascript:alert(1)")),
        ("javascript link in mixed case", a("JaVaScRiPt:alert(1)")),
        ("javascript link with an entity", a("&#106;avascript:alert(1)")),
        (
            "javascript link with a named entity",
            a("javascript&colon;alert(1)"),
        ),
        ("data link", a("data:text/html;base64,PHNjcmlwdD4=")),
        ("http link", a("http://insecure.example/")),
        ("mailto link", a("mailto:a@example.com")),
        ("protocol-relative link", a("//evil.example/")),
        ("link with a backslash", a("/\\evil.example/")),
        ("link with credentials", a("https://docs.example@evil.example/")),
        ("link with an empty host", a("https:///x")),
        ("relative link", a("guide.md")),
        ("link with a space", a("https://d.example/a b")),
        (
            "link with a quote in the value",
            "<a href=\"https://d.example/\"onclick=\"x\" rel=\"noopener noreferrer\">x</a>".into(),
        ),
        ("link without rel", "<a href=\"https://d.example/\">x</a>".into()),
        (
            "link with another rel",
            "<a href=\"https://d.example/\" rel=\"nofollow\">x</a>".into(),
        ),
        ("link without href", "<a rel=\"noopener noreferrer\">x</a>".into()),
        (
            "link inside a link",
            format!("<a href=\"/a\" rel=\"noopener noreferrer\">{}</a>", a("/b")),
        ),
        ("fragment that is no heading id", a("#main")),
        (
            "external image",
            "<img src=\"https://evil.example/p.png\" alt=\"\">".into(),
        ),
        (
            "data image",
            "<img src=\"data:image/png;base64,AAAA\" alt=\"\">".into(),
        ),
        (
            "protocol-relative image",
            "<img src=\"//evil.example/p.png\" alt=\"\">".into(),
        ),
        ("image without src", "<img alt=\"x\">".into()),
        ("single-quoted value", "<img src='/a.png'>".into()),
        ("unquoted value", "<img src=/a.png>".into()),
        ("attribute with no value", "<img src=\"/a.png\" alt>".into()),
        (
            "an entity outside the five",
            "<img src=\"/a.png\" alt=\"&copy;\">".into(),
        ),
        (
            "a hex entity the text does not list",
            "<img src=\"/a.png\" alt=\"&#x27;\">".into(),
        ),
        (
            "a raw angle bracket in a value",
            "<img src=\"/a.png\" alt=\"<b>\">".into(),
        ),
        ("unterminated tag", "<p".into()),
        ("unterminated value", "<img src=\"/a.png".into()),
        ("less-than in text", "<p>a < b</p>".into()),
        ("less-than at the end", "<p>x</p><".into()),
        ("closing tag with a space", "<p>x</p >".into()),
        ("closing tag of a void element", "<br></br>".into()),
        ("self-closing non-void element", "<p/>".into()),
        ("unclosed element", "<p>x".into()),
        ("closing tag that does not match", "<p><em>x</p></em>".into()),
        ("closing tag with nothing open", "</p>".into()),
        ("control character", "<p>a\u{0}b</p>".into()),
        ("escape character", "<p>a\u{1b}[31mb</p>".into()),
        ("c1 control character", "<p>a\u{85}b</p>".into()),
    ];
    for (label, html) in &cases {
        assert!(
            !html_violations(html).is_empty(),
            "the checker accepts a page with a {label}: {html}"
        );
    }
    // Each label is unique, and none of the refused pages is also a page the checker should accept.
    let labels: BTreeSet<&str> = cases.iter().map(|(l, _)| *l).collect();
    assert_eq!(labels.len(), cases.len());
}

#[test]
fn rendered_doc_violations_check_the_outline_against_the_page() {
    let ok = sample_rendered();
    assert_eq!(rendered_doc_violations(&ok), Vec::<String>::new());

    let mut r = ok.clone();
    r.headings[0].id = ShortText::parse("doc-elsewhere").unwrap();
    assert!(
        rendered_doc_violations(&r)
            .iter()
            .any(|v| v.contains("not in the html")),
        "a heading id the page does not carry"
    );

    let mut r = ok.clone();
    r.headings[0].level = 2;
    assert!(
        !rendered_doc_violations(&r).is_empty(),
        "a heading on another level than its element"
    );

    let mut r = ok.clone();
    r.headings[0].id = ShortText::parse("admin").unwrap();
    assert!(!rendered_doc_violations(&r).is_empty(), "an id with no prefix");

    let mut r = ok.clone();
    r.headings[0].level = 7;
    assert!(!rendered_doc_violations(&r).is_empty(), "level 7");

    let mut r = ok.clone();
    r.headings[0].line = 0;
    assert!(!rendered_doc_violations(&r).is_empty(), "lines are 1-based");

    let mut r = ok.clone();
    r.headings.push(r.headings[0].clone());
    assert!(!rendered_doc_violations(&r).is_empty(), "the same id twice");

    let mut r = ok.clone();
    r.html.push_str("<script>x</script>");
    assert!(!rendered_doc_violations(&r).is_empty(), "the page is checked too");

    let mut r = ok;
    r.html = format!("<p>{}</p>", "x".repeat(MAX_RENDERED_HTML_BYTES));
    assert!(
        rendered_doc_violations(&r).iter().any(|v| v.contains("bytes")),
        "a page over the bound"
    );
}

#[test]
fn the_corpus_covers_every_way_the_description_names() {
    let names: BTreeSet<&str> = XSS_CORPUS.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names.len(),
        XSS_CORPUS.len(),
        "names are unique: they become paths"
    );
    for (name, source) in XSS_CORPUS {
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{name}: a corpus name is a path component"
        );
        assert!(!source.is_empty() && source.len() < 4096, "{name}");
    }
    // The attacks the plan names, each by at least one entry.
    let all: String = XSS_CORPUS.iter().map(|(_, s)| *s).collect::<Vec<_>>().join("\n");
    for needle in [
        "<script",
        "<img",
        "onerror",
        "javascript:",
        "style=",
        "<iframe",
        "<svg",
        "<form",
        "data:image",
        "https://evil.example",
        "<!--",
        "onclick",
        "vbscript:",
        "data:text/html",
        "http://",
        "//evil.example",
        "[REDACTED: 12 chars, api_key]",
    ] {
        assert!(all.contains(needle), "the corpus has no entry with `{needle}`");
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The suite, and implementations that each break one rule

#[tokio::test(start_paused = true)]
async fn render_output_stays_inside_the_allowlist() {
    conformance::render_output_stays_inside_the_allowlist(&FakeDocSearch::new()).await;
}

/// The one way a wrapper of the fake leaves the contract.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Break {
    /// Forwards everything, and breaks nothing.
    Nothing,
    /// Masks the planted secret in the page, as an implementation that scans before it renders does.
    MasksTheSecret,
    /// Keeps the default of `render`.
    KeepsTheRenderDefault,
    /// Appends the raw text of the doc to the page.
    EmitsTheTextRaw,
    /// Answers a doc over the bound with a page instead of a refusal.
    ForgetsTheBound,
    /// Lets the planted secret reach the page.
    LeaksTheSecret,
    /// Names a heading id that the page does not carry.
    HeadingIdNotInThePage,
    /// Derives the content hash from the path, not from the text.
    HashFollowsThePath,
    /// Gives a search hit an anchor that names no heading of its page.
    AnchorNamesNoHeading,
    /// Forgets to cut the outline at the bound.
    OutlineWithoutBound,
}

const PLANTED: &str = "hunter2-planted-value-4711";

struct Wrapped {
    inner: FakeDocSearch,
    how: Break,
    planted: Mutex<BTreeMap<DocPath, String>>,
}

impl Wrapped {
    fn new(how: Break) -> Self {
        Self {
            inner: FakeDocSearch::new(),
            how,
            planted: Mutex::new(BTreeMap::new()),
        }
    }
}

#[async_trait]
impl DocSearch for Wrapped {
    async fn search(&self, u: &User, q: &str, limit: usize) -> Result<Vec<DocHit>, DocError> {
        let mut hits = self.inner.search(u, q, limit).await?;
        if self.how == Break::AnchorNamesNoHeading {
            for h in &mut hits {
                h.anchor = Some(ShortText::parse("doc-no-such-heading").unwrap());
            }
        }
        Ok(hits)
    }
    async fn related(&self, u: &User, f: &LogicalFile) -> Result<Vec<DocHit>, DocError> {
        self.inner.related(u, f).await
    }
    async fn read(&self, u: &User, path: &DocPath, lines: Option<LineRange>) -> Result<DocText, DocError> {
        self.inner.read(u, path, lines).await
    }
    async fn grep(
        &self,
        u: &User,
        pattern: &str,
        regex: bool,
        context: u8,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        self.inner.grep(u, pattern, regex, context).await
    }
    async fn feature_status(&self, u: &User, q: FeatureQuery) -> Result<FeatureStatus, DocError> {
        self.inner.feature_status(u, q).await
    }
    async fn related_to(
        &self,
        u: &User,
        f: &LogicalFile,
        setting: Option<&SettingPath>,
        limit: usize,
    ) -> Result<Vec<DocHit>, DocError> {
        self.inner.related_to(u, f, setting, limit).await
    }
    async fn grep_in(
        &self,
        u: &User,
        pattern: &str,
        regex: bool,
        context: u8,
        prefix: Option<&DocPath>,
        limit: usize,
    ) -> Result<Vec<DocGrepHit>, DocError> {
        self.inner
            .grep_in(u, pattern, regex, context, prefix, limit)
            .await
    }

    async fn render(&self, u: &User, path: &DocPath) -> Result<RenderedDoc, DocError> {
        if self.how == Break::KeepsTheRenderDefault {
            return Err(DocError::Unavailable);
        }
        let honest = self.inner.render(u, path).await;
        let mut page = match honest {
            Ok(page) => page,
            Err(DocError::BadRequest) if self.how == Break::ForgetsTheBound => {
                return Ok(RenderedDoc {
                    path: path.clone(),
                    html: "<p>too big, rendered anyway</p>".to_owned(),
                    headings: Vec::new(),
                    content_hash: ContentHash::from_bytes([0; 32]),
                });
            }
            Err(e) => return Err(e),
        };
        match self.how {
            Break::EmitsTheTextRaw => {
                let text = self.inner.read(u, path, None).await?.text;
                page.html.push_str(&text);
            }
            Break::MasksTheSecret => {
                let marker = format!("[REDACTED: {} chars, test_rule]", PLANTED.len());
                page.html = page.html.replace(PLANTED, &marker);
            }
            Break::HeadingIdNotInThePage => {
                if let Some(h) = page.headings.first_mut() {
                    h.id = ShortText::parse("doc-not-in-the-page").unwrap();
                }
            }
            Break::HashFollowsThePath => page.content_hash = content_hash(path.as_str().as_bytes()),
            Break::OutlineWithoutBound => {
                let last = page.headings.last().cloned();
                if let Some(last) = last {
                    if page.headings.len() == MAX_DOC_HEADINGS {
                        let mut extra = last;
                        extra.id = ShortText::parse("doc-one-too-many").unwrap();
                        page.headings.push(extra);
                    }
                }
            }
            _ => {}
        }
        Ok(page)
    }
}

#[async_trait]
impl DocScenario for Wrapped {
    async fn seed(&self, path: DocPath, title: &str, text: &str) {
        FakeDocSearch::seed(&self.inner, path, title, text);
    }

    async fn plant_secret(&self, path: DocPath) -> Option<String> {
        if !matches!(self.how, Break::MasksTheSecret | Break::LeaksTheSecret) {
            return None;
        }
        FakeDocSearch::seed(
            &self.inner,
            path.clone(),
            "Planted",
            &format!("# Planted\n\nthe value is {PLANTED} and nobody may see it\n"),
        );
        self.planted.lock().unwrap().insert(path, PLANTED.to_owned());
        Some(PLANTED.to_owned())
    }
}

#[tokio::test(start_paused = true)]
async fn a_wrapper_that_breaks_nothing_passes_the_suite() {
    conformance::doc_search(&Wrapped::new(Break::Nothing)).await;
}

#[tokio::test(start_paused = true)]
async fn an_implementation_that_masks_the_planted_secret_passes() {
    let w = Wrapped::new(Break::MasksTheSecret);
    conformance::render_output_stays_inside_the_allowlist(&w).await;
    assert_eq!(w.planted.lock().unwrap().len(), 1, "the suite did plant a secret");
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "left: Unavailable")]
async fn a_doc_search_that_keeps_the_render_default_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::KeepsTheRenderDefault)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "element <script> is not in the allowlist")]
async fn a_render_that_emits_the_text_raw_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::EmitsTheTextRaw)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "must be refused")]
async fn a_render_that_ignores_the_bound_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::ForgetsTheBound)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "a flagged value reached the rendered page")]
async fn a_render_that_leaks_a_flagged_value_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::LeaksTheSecret)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "not in the html")]
async fn an_outline_that_disagrees_with_the_page_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::HeadingIdNotInThePage)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "same text, same content hash")]
async fn a_content_hash_that_follows_the_path_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::HashFollowsThePath)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "outline is cut at the bound")]
async fn an_outline_with_no_bound_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::OutlineWithoutBound)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "names no heading")]
async fn a_hit_whose_anchor_names_no_heading_fails() {
    conformance::doc_search(&Wrapped::new(Break::AnchorNamesNoHeading)).await;
}

// ---------------------------------------------------------------------------------------------------------------
// The fake

#[tokio::test(start_paused = true)]
async fn the_fake_refuses_a_doc_over_the_source_bound_and_renders_one_at_it() {
    let fake = FakeDocSearch::new();
    let viewer = ports::conformance::sample::active(1, domain::Role::Viewer);
    let p = DocPath::parse("/git/main/big.md").unwrap();
    fake.seed(p.clone(), "Big", &"a".repeat(MAX_RENDER_SOURCE_BYTES + 1));
    assert_eq!(fake.render(&viewer, &p).await.unwrap_err(), DocError::BadRequest);
    fake.seed(p.clone(), "Big", &"a".repeat(MAX_RENDER_SOURCE_BYTES));
    let page = fake.render(&viewer, &p).await.unwrap();
    assert!(page.html.len() <= MAX_RENDERED_HTML_BYTES);
    assert_eq!(rendered_doc_violations(&page), Vec::<String>::new());
}

#[tokio::test(start_paused = true)]
async fn the_fake_numbers_duplicate_headings_apart_and_ignores_line_endings() {
    let fake = FakeDocSearch::new();
    let viewer = ports::conformance::sample::active(1, domain::Role::Viewer);
    let lf = DocPath::parse("/git/main/lf.md").unwrap();
    let crlf = DocPath::parse("/git/main/crlf.md").unwrap();
    fake.seed(lf.clone(), "Lf", "# A\n\n## Usage\n\n## Usage\n\n## Usage 2\n");
    fake.seed(
        crlf.clone(),
        "Crlf",
        "# A\r\n\r\n## Usage\r\n\r\n## Usage\r\n\r\n## Usage 2\r\n",
    );
    let (a, b) = (
        fake.render(&viewer, &lf).await.unwrap(),
        fake.render(&viewer, &crlf).await.unwrap(),
    );
    assert_eq!(a.html, b.html, "line endings do not reach the page");
    assert_eq!(a.headings, b.headings);
    assert_eq!(
        a.content_hash,
        content_hash("# A\n\n## Usage\n\n## Usage\n\n## Usage 2\n".as_bytes())
    );
    let ids: BTreeSet<&str> = a.headings.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids.len(), 4, "four headings, four ids: {ids:?}");
    assert_eq!(
        a.headings.iter().map(|h| h.line).collect::<Vec<_>>(),
        [1, 3, 5, 7],
        "lines are 1-based"
    );
    assert_eq!(rendered_doc_violations(&a), Vec::<String>::new());
}
