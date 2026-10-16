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
    self, ALLOWED_HTML_TAGS, DocScenario, SecretPlant, XSS_CORPUS, html_violations, is_heading_id,
    rendered_doc_violations,
};
use ports::fakes::FakeDocSearch;
use ports::{
    DocError, DocSearch, HEADING_ID_PATTERN, MAX_DOC_HEADINGS, MAX_HEADING_ID_BYTES, MAX_HTML_NESTING_DEPTH,
    MAX_RENDER_SOURCE_BYTES, MAX_RENDERED_HTML_BYTES, MAX_URL_BYTES, content_hash,
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

/// A browser drops a tab and a line break from a URL before it reads it, and reads `/\` as `//`. The text must say what a
/// URL value is, in the words the checker enforces, or an implementation that reads the text emits `/<tab>/evil.example/`
/// (a path in the text's words, another origin in a browser's).
#[test]
fn the_allowlist_text_states_what_a_url_value_is_and_how_deep_a_page_nests() {
    let d = doc();
    let text = d["components"]["schemas"]["RenderedDoc"]["properties"]["html"]["description"]
        .as_str()
        .expect("RenderedDoc.html has a description that states the allowlist");
    let paragraph = |starts: &str| {
        text.split("\n\n")
            .find(|p| p.starts_with(starts))
            .unwrap_or_else(|| panic!("no paragraph that starts `{starts}`"))
    };
    let links = paragraph("Links:");
    let images = paragraph("Images:");
    let markup = paragraph("Markup:");
    for needle in [
        format!("printable ASCII of at most {MAX_URL_BYTES} characters"),
        "no space, tab, line break or other control character".to_owned(),
        "no backslash".to_owned(),
        "a quote".to_owned(),
        "a backtick".to_owned(),
        "drops a tab or a line break from a URL".to_owned(),
        "reads `/\\` as `//`".to_owned(),
    ] {
        assert!(
            links.contains(&needle),
            "the Links paragraph must say `{needle}`: it is the rule of every URL value"
        );
    }
    assert!(
        images.contains("URL value"),
        "the Images paragraph must say that `src` is a URL value, with the rule of Links"
    );
    let depth = format!("nested at most {MAX_HTML_NESTING_DEPTH} deep");
    assert!(
        markup.contains(&depth),
        "the Markup paragraph must say `{depth}`: an unbounded tower is a denial of service for the reader"
    );
    assert!(
        markup.contains("dropped") && markup.contains("content is kept"),
        "the Markup paragraph must say what a render does with a container beyond the limit"
    );
    assert!(
        markup.contains("neither of those inside a URL value"),
        "the Markup paragraph lets line feed and tab through, so it must say where they may not be"
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
        // A browser drops a tab, a line break and a carriage return from a URL before it reads it, so each of these is
        // another address than the one the checker would read. `is_plain_url` is the only check that refuses a tab or a
        // line break inside a value (the page-wide control check lets both through).
        ("path with a tab in it", a("/\t/evil.example/")),
        ("path with a line feed in it", a("/\n/evil.example/")),
        ("path with a carriage return in it", a("/\r/evil.example/")),
        ("https link ending in a tab", a("https://d.example/\t")),
        ("https link ending in a line feed", a("https://d.example/\n")),
        ("https link with a tab in the host", a("https://d.exa\tmple/")),
        ("path with a non-ascii letter", a("/caf\u{e9}")),
        (
            "https host with a full-width full stop",
            a("https://d\u{ff0e}example/"),
        ),
        (
            "https link with a non-breaking space",
            a("https://d.example/a\u{a0}b"),
        ),
        (
            "link of more than the bound",
            a(&format!("https://d.example/{}", "a".repeat(MAX_URL_BYTES))),
        ),
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
        (
            "image path with a tab in it",
            "<img src=\"/\t/evil.example/p.png\" alt=\"\">".into(),
        ),
        (
            "image path with a line feed in it",
            "<img src=\"/\n/evil.example/p.png\" alt=\"\">".into(),
        ),
        (
            "image path with a backslash",
            "<img src=\"/\\evil.example/p.png\" alt=\"\">".into(),
        ),
        (
            "image path with a non-ascii letter",
            "<img src=\"/caf\u{e9}.png\" alt=\"\">".into(),
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

/// `n` nested `tag` elements around `leaf`.
fn tower(n: usize, tag: &str, leaf: &str) -> String {
    format!(
        "{}{leaf}{}",
        format!("<{tag}>").repeat(n),
        format!("</{tag}>").repeat(n)
    )
}

#[test]
fn the_checker_bounds_how_deep_elements_nest() {
    // The bound itself is a page, one more is not.
    assert_eq!(
        html_violations(&tower(MAX_HTML_NESTING_DEPTH, "blockquote", "x")),
        Vec::<String>::new(),
        "a page nested exactly {MAX_HTML_NESTING_DEPTH} deep is inside the allowlist"
    );
    let over = html_violations(&tower(MAX_HTML_NESTING_DEPTH + 1, "blockquote", "x"));
    assert!(
        over.iter().any(|v| v.contains("nested deeper than")),
        "a page nested {} deep is outside it: {over:?}",
        MAX_HTML_NESTING_DEPTH + 1
    );

    // A void element is an element too: a line break inside the innermost of the bound is one level too deep.
    assert!(
        !html_violations(&tower(MAX_HTML_NESTING_DEPTH, "blockquote", "<br>")).is_empty(),
        "a void element counts as a level"
    );
    assert_eq!(
        html_violations(&tower(MAX_HTML_NESTING_DEPTH - 1, "blockquote", "<br>")),
        Vec::<String>::new()
    );

    // Siblings are not nesting: many shallow elements are fine, and so is a long page of them.
    let flat = "<blockquote><p>x</p></blockquote>".repeat(10_000);
    assert_eq!(html_violations(&flat), Vec::<String>::new());

    // Every container counts, not only the quote: lists (an `ul` and an `li` per level) and tables.
    let lists = format!(
        "{}x{}",
        "<ul><li>".repeat(MAX_HTML_NESTING_DEPTH / 2 + 1),
        "</li></ul>".repeat(MAX_HTML_NESTING_DEPTH / 2 + 1)
    );
    assert!(!html_violations(&lists).is_empty(), "lists nest too: {lists:.80}");
    let table = format!(
        "{}<table><tbody><tr><td>x</td></tr></tbody></table>{}",
        "<blockquote>".repeat(MAX_HTML_NESTING_DEPTH - 3),
        "</blockquote>".repeat(MAX_HTML_NESTING_DEPTH - 3)
    );
    assert!(
        !html_violations(&table).is_empty(),
        "a table inside a quote tower counts every level of the table"
    );

    // The probe of the review: a page 3000 deep is refused, and the answer stays short.
    let tall = html_violations(&tower(3000, "blockquote", "x"));
    assert!(
        tall.iter().any(|v| v.contains("nested deeper than")),
        "a page 3000 deep is outside the allowlist"
    );
    assert!(
        tall.len() <= 20,
        "the answer is bounded: {} violations",
        tall.len()
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
        // The ways a browser reads a path as another origin: a backslash, a tab and a line break inside the value.
        "<a href=\"//evil.example/\"",
        "<a href=\"/\\evil.example/\"",
        "<a href=\"/\t/evil.example/\"",
        "<a href=\"/\n/evil.example/\"",
        "<img src=\"//evil.example/",
        "<img src=\"/\\evil.example/",
        "<img src=\"/\t/evil.example/",
        "<img src=\"/\n/evil.example/",
        // Relative links of Markdown, which the page rewrites to the reader route or drops.
        "](guide.md)",
        "](../x.md)",
    ] {
        assert!(all.contains(needle), "the corpus has no entry with `{needle}`");
    }
}

/// The corpus entries that the review of this contract found missing. A name here is a deliberate act to remove.
const URL_ENTRIES: [&str; 8] = [
    "raw-a-protocol-relative",
    "raw-a-backslash",
    "raw-a-tab-path",
    "raw-a-line-feed-path",
    "raw-img-protocol-relative",
    "raw-img-backslash",
    "raw-img-tab-path",
    "raw-img-line-feed-path",
];

const OTHER_REQUIRED_ENTRIES: [&str; 8] = [
    "relative-md-link",
    "relative-md-link-parent",
    "relative-md-link-fragment",
    "md-link-tab-path",
    "md-image-tab-path",
    "deep-nesting",
    "deep-nesting-quotes",
    "deep-nesting-lists",
];

#[test]
fn the_corpus_has_the_url_nesting_and_relative_link_entries() {
    let names: BTreeSet<&str> = XSS_CORPUS.iter().map(|(n, _)| *n).collect();
    for name in URL_ENTRIES.iter().chain(OTHER_REQUIRED_ENTRIES.iter()) {
        assert!(names.contains(name), "the corpus has no entry `{name}`");
    }
}

/// The `raw-` entries are the anchors and images that a renderer which trusts a path-shaped URL would let through. Each is a
/// page that would be inside the allowlist but for its URL value, so the checker is the only thing between it and the reader:
/// emitted as it stands, the checker must refuse it, and for its URL. Delete the tab, line feed, backslash, `//` or
/// non-ASCII check of the checker and one of these turns red.
#[test]
fn every_raw_url_entry_is_refused_by_the_checker_for_its_url_and_for_nothing_else() {
    let raw: Vec<&(&str, &str)> = XSS_CORPUS.iter().filter(|(n, _)| n.starts_with("raw-")).collect();
    assert_eq!(
        raw.iter().map(|(n, _)| *n).collect::<BTreeSet<_>>(),
        URL_ENTRIES.iter().copied().collect(),
        "the `raw-` entries are the URL entries"
    );
    for (name, source) in raw {
        let (attr, tag) = if source.starts_with("<a ") {
            ("href", "a")
        } else {
            ("src", "img")
        };
        assert!(
            source.starts_with(&format!("<{tag} {attr}=\"")),
            "{name}: a raw entry is one {tag} with its {attr} first"
        );
        let violations = html_violations(source);
        assert!(
            violations
                .first()
                .is_some_and(|v| v.contains(&format!("of {attr} on <{tag}> is outside the allowlist"))),
            "{name}: the checker must refuse the page for the value of {attr}, and says {violations:?}"
        );
        // The twin with a safe value is a page inside the allowlist: the URL was the only defect.
        let start = format!("<{tag} {attr}=\"").len();
        let end = start + source[start..].find('"').expect("a closing quote");
        let twin = format!("{}/ok{}", &source[..start], &source[end..]);
        assert_eq!(
            html_violations(&twin),
            Vec::<String>::new(),
            "{name}: with a safe value the page is clean, so the value is the one defect: {twin}"
        );
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
    /// Forwards everything, and breaks nothing: the fake plants a secret and masks it before it renders.
    Nothing,
    /// Keeps the default of `render`.
    KeepsTheRenderDefault,
    /// Appends the raw text of the doc to the page.
    EmitsTheTextRaw,
    /// Answers a doc over the bound with a page instead of a refusal.
    ForgetsTheBound,
    /// Plants a value that its own scan does not flag, so the value reaches the page.
    LeaksTheSecret,
    /// Says it planted a value, and seeds a doc that does not hold it: the check would pass for nothing.
    PlantsNothing,
    /// Says it cannot plant a secret, with a reason.
    CannotPlantAndSaysWhy,
    /// Says it cannot plant a secret, and gives no reason.
    CannotPlantAndSaysNothing,
    /// Names a heading id that the page does not carry.
    HeadingIdNotInThePage,
    /// Derives the content hash from the path, not from the text.
    HashFollowsThePath,
    /// Gives a search hit an anchor that names no heading of its page.
    AnchorNamesNoHeading,
    /// Forgets to cut the outline at the bound.
    OutlineWithoutBound,
    /// A sanitiser that trusts a path-shaped URL: the `raw-` entries of the corpus reach the page as they are written.
    PassesRawUrlsThrough,
    /// A renderer that turns every `>` of a quote marker into a `blockquote`, however many there are.
    NestsQuotesWithoutBound,
}

/// A value that the secret scan of the fake does not flag, so that only a wrapper can leak or mask it.
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
            // The raw text of a corpus doc, which is where a renderer that forgets to sanitise is caught.
            Break::EmitsTheTextRaw if path.as_str().contains("/xss/") => {
                let text = self.inner.read(u, path, None).await?.text;
                page.html.push_str(&text);
            }
            Break::HeadingIdNotInThePage => {
                if let Some(h) = page.headings.first_mut() {
                    h.id = ShortText::parse("doc-not-in-the-page").unwrap();
                }
            }
            Break::PassesRawUrlsThrough if path.as_str().contains("/xss/raw-") => {
                page.html = self.inner.read(u, path, None).await?.text;
            }
            Break::NestsQuotesWithoutBound if path.as_str().contains("/xss/deep-nesting") => {
                let text = self.inner.read(u, path, None).await?.text;
                let depth = text
                    .bytes()
                    .take_while(|b| matches!(*b, b'>' | b' '))
                    .filter(|b| *b == b'>')
                    .count();
                page.html = format!(
                    "{}<p>x</p>{}",
                    "<blockquote>".repeat(depth),
                    "</blockquote>".repeat(depth)
                );
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

    async fn plant_secret(&self, path: DocPath) -> SecretPlant {
        match self.how {
            Break::CannotPlantAndSaysWhy => SecretPlant::Cannot {
                reason: "this wrapper has no secret scan of its own to give a value to",
            },
            Break::CannotPlantAndSaysNothing => SecretPlant::Cannot { reason: "  " },
            Break::PlantsNothing => {
                FakeDocSearch::seed(
                    &self.inner,
                    path,
                    "Planted",
                    "# Planted\n\nnothing to hide here\n",
                );
                SecretPlant::Planted(PLANTED.to_owned())
            }
            Break::LeaksTheSecret => {
                // A value that the scan of this implementation does not flag, so that it reaches the page as it stands.
                FakeDocSearch::seed(
                    &self.inner,
                    path.clone(),
                    "Planted",
                    &format!("# Planted\n\nthe value is {PLANTED} and nobody may see it\n"),
                );
                SecretPlant::Planted(PLANTED.to_owned())
            }
            _ => {
                let planted = DocScenario::plant_secret(&self.inner, path.clone()).await;
                if let SecretPlant::Planted(secret) = &planted {
                    self.planted.lock().unwrap().insert(path, secret.clone());
                }
                planted
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_wrapper_that_breaks_nothing_passes_the_suite() {
    conformance::doc_search(&Wrapped::new(Break::Nothing)).await;
}

#[tokio::test(start_paused = true)]
async fn an_implementation_that_masks_the_planted_secret_passes() {
    let w = Wrapped::new(Break::Nothing);
    conformance::render_output_stays_inside_the_allowlist(&w).await;
    assert_eq!(w.planted.lock().unwrap().len(), 1, "the suite did plant a secret");
}

/// The check of the order "scan, then render" cannot be skipped by saying nothing: a harness plants, or says why it cannot.
#[tokio::test(start_paused = true)]
async fn a_scenario_that_cannot_plant_a_secret_and_says_why_passes() {
    conformance::doc_search(&Wrapped::new(Break::CannotPlantAndSaysWhy)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "a scenario that cannot plant a secret says why")]
async fn a_scenario_that_cannot_plant_a_secret_and_gives_no_reason_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::CannotPlantAndSaysNothing))
        .await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "the planted value is masked and not just dropped")]
async fn a_scenario_that_plants_nothing_the_page_could_leak_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::PlantsNothing)).await;
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
#[should_panic(expected = "raw-a-protocol-relative: the rendered page leaves the allowlist")]
async fn a_sanitiser_that_trusts_a_path_shaped_url_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::PassesRawUrlsThrough)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "nested deeper than")]
async fn a_renderer_that_nests_without_a_bound_fails() {
    conformance::render_output_stays_inside_the_allowlist(&Wrapped::new(Break::NestsQuotesWithoutBound))
        .await;
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

/// The scan runs before the render, in a heading as in a paragraph: the page and the outline hold the marker, never the value,
/// the lines are the stored text's, and the hash is of the stored text (it is what the ETag follows).
#[tokio::test(start_paused = true)]
async fn the_fake_masks_what_its_scan_flags_before_it_renders() {
    let fake = FakeDocSearch::new();
    let viewer = ports::conformance::sample::active(1, domain::Role::Viewer);
    let p = DocPath::parse("/git/main/scan.md").unwrap();
    let text = "# Keys fake-secret-heading-1\n\nthe key is fake-secret-abc, and fake-secret-def\nplain fake-secrets are fine\n";
    fake.seed(p.clone(), "Scan", text);
    let page = fake.render(&viewer, &p).await.unwrap();
    assert!(
        !page.html.contains("fake-secret-"),
        "no flagged value in the page: {}",
        page.html
    );
    // The scan takes a whole word, a trailing comma included: 16 characters, then 15.
    assert!(
        page.html
            .contains("[REDACTED: 16 chars, fake_rule] and [REDACTED: 15 chars, fake_rule]"),
        "{}",
        page.html
    );
    assert!(
        page.html.contains("[REDACTED: 21 chars, fake_rule]"),
        "the heading's value: {}",
        page.html
    );
    assert!(
        page.html.contains("plain fake-secrets are fine"),
        "a word that merely holds the prefix is not flagged"
    );
    assert_eq!(page.headings.len(), 1);
    assert_eq!(
        page.headings[0].text.as_str(),
        "Keys [REDACTED: 21 chars, fake_rule]"
    );
    assert_eq!(page.headings[0].line, 1);
    assert_eq!(
        page.content_hash,
        content_hash(text.as_bytes()),
        "the hash is of the stored text"
    );
    assert_eq!(rendered_doc_violations(&page), Vec::<String>::new());
}
