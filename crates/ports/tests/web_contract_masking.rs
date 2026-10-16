//! contract/08-web, `CG7` (the human chose M1): masking of read responses (S14b, D79).
//!
//! The server masks at the REST edge and never in a port: a response that carries file or doc text never holds a value
//! the secret scan flags, only its marker `[REDACTED: N chars, rule]`. `FileContent`, `ServedFile` and `DocText` also say
//! where each marker sits (`masks`), so that the web app can show a reveal button and turn Edit mode off. A write that
//! holds a marker is refused, with the issue kind `redaction_marker`, so that a masked text can never be saved over the
//! value it hides.
//!
//! Each check below is a function that returns what is wrong with a document, so that the teeth tests can damage a copy of
//! the document and see that the check finds the damage.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use domain::IssueKind;
use serde_json::{Value, json};

const SPEC: &str = include_str!("../../../api/openapi.yaml");
const SCHEMA_REF: &str = "#/components/schemas/";
const MASK_REF: &str = "#/components/schemas/Mask";

fn doc() -> Value {
    serde_saphyr::from_str(SPEC).expect("api/openapi.yaml is valid YAML")
}

// The marker, as every description writes it.
const MARKER_SHAPE: &str = "[REDACTED: N chars, rule]";

// The three schemas that carry `masks`, and the property the masks index.
const CARRIERS: [(&str, &str); 3] = [
    ("FileContent", "text"),
    ("ServedFile", "text"),
    ("DocText", "text"),
];

// What the schemas this change touches required before it, and still require: `masks` is optional, so a body that was valid
// stays valid (additive only).
const REQUIRED: [(&str, &[&str]); 5] = [
    (
        "FileContent",
        &[
            "path",
            "hash",
            "class",
            "format",
            "size",
            "content_withheld",
            "text",
        ],
    ),
    (
        "ServedFile",
        &[
            "swimlane",
            "application",
            "tenant",
            "channel",
            "file",
            "role",
            "hash",
            "content_withheld",
            "text",
            "sources",
            "unresolved_placeholders",
        ],
    ),
    (
        "DocText",
        &[
            "path",
            "lines",
            "total_lines",
            "text",
            "truncated",
            "content_hash",
        ],
    ),
    ("ValidationIssue", &["kind"]),
    ("Mask", &["line", "start", "end"]),
];

// How a property that can hold file or doc text is protected.
#[derive(Clone, Copy)]
enum Stance {
    // The server replaces a flagged value by its marker, and the description says so.
    Masked,
    // Raw on purpose: the word the description must carry to say why.
    Raw(&'static str),
}

// Every property of the document that can hold file or doc text, each masked or raw by design. A schema with such a property
// that is not in the table fails `every_text_bearing_schema_states_how_it_is_masked`: whoever adds one decides here.
const TEXT_BEARING: [(&str, &str, Stance); 18] = [
    ("FileContent", "text", Stance::Masked),
    ("ServedFile", "text", Stance::Masked),
    ("DocText", "text", Stance::Masked),
    ("DiffLine", "text", Stance::Masked),
    ("SettingValue", "text", Stance::Masked),
    ("TextHit", "text", Stance::Masked),
    ("EffectiveSetting", "value", Stance::Masked),
    ("RenderedDoc", "html", Stance::Masked),
    ("DocHeading", "text", Stance::Masked),
    ("DocGrepHit", "text", Stance::Masked),
    ("DocGrepHit", "before", Stance::Masked),
    ("DocGrepHit", "after", Stance::Masked),
    ("DocHit", "snippet", Stance::Masked),
    ("Proposal", "hunks", Stance::Masked),
    ("Draft", "hunks", Stance::Masked),
    ("Comparison", "hunks", Stance::Masked),
    ("AuditEvent", "diff", Stance::Masked),
    // The audited reveal of one value: an Editor, 30 a minute, never cached. It is the one way to see a value.
    ("RevealedValue", "value", Stance::Raw("unmasked")),
];

// The names under which a property can hold text. A property of one of these names whose own type is text, or that is the
// list of diff hunks, has to be in the table.
const TEXT_NAMES: [&str; 8] = [
    "text", "diff", "snippet", "hunks", "value", "html", "before", "after",
];

// The operations that take file text from the client and so can be handed a masked copy of a file.
const TEXT_WRITES: [&str; 4] = ["editFile", "createDraft", "uploadFile", "previewFileEffect"];

// ---------------------------------------------------------------------------------------------------------------------
// Reading the document
// ---------------------------------------------------------------------------------------------------------------------

fn operation<'a>(d: &'a Value, id: &str) -> Option<&'a Value> {
    for item in d["paths"].as_object()?.values() {
        for m in ["get", "put", "post", "delete", "patch"] {
            if let Some(op) = item.get(m)
                && op["operationId"] == id
            {
                return Some(op);
            }
        }
    }
    None
}

// Every description of a subtree, joined, in lower case.
fn descriptions(v: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::Object(o) => {
                if let Some(text) = o.get("description").and_then(Value::as_str) {
                    out.push_str(text);
                    out.push('\n');
                }
                for (k, x) in o {
                    if k != "description" {
                        walk(x, out);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    walk(v, &mut out);
    out.to_lowercase()
}

fn own_description(v: &Value) -> String {
    v["description"].as_str().unwrap_or("").to_lowercase()
}

// Records every needle that `text` (already in lower case) does not hold.
fn say(gaps: &mut Vec<String>, at: &str, text: &str, needles: &[&str]) {
    for needle in needles {
        if !text.contains(&needle.to_lowercase()) {
            gaps.push(format!("{at} must say `{needle}`"));
        }
    }
}

fn names(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

// Every `$ref` string of the document, with the pointer of the node that holds it.
fn refs(d: &Value) -> Vec<(String, String)> {
    fn walk(v: &Value, at: &str, out: &mut Vec<(String, String)>) {
        match v {
            Value::Object(o) => {
                if let Some(r) = o.get("$ref").and_then(Value::as_str) {
                    out.push((at.to_owned(), r.to_owned()));
                }
                for (k, x) in o {
                    walk(x, &format!("{at}/{k}"), out);
                }
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(x, &format!("{at}/{i}"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(d, "", &mut out);
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// The mask: where one marker sits
// ---------------------------------------------------------------------------------------------------------------------

// What is wrong with the `Mask` schema: its fields, their bounds and the units a client has to know.
fn mask_gaps(d: &Value) -> Vec<String> {
    let mut gaps = Vec::new();
    let Some(mask) = d["components"]["schemas"].get("Mask") else {
        return vec!["schema Mask is not in the document".to_owned()];
    };
    if mask["type"] != "object" || mask["additionalProperties"] != false {
        gaps.push("Mask is not a closed object".to_owned());
    }
    let declared: BTreeSet<String> = mask["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    if declared != set(&["line", "start", "end", "setting"]) {
        gaps.push(format!(
            "Mask declares {declared:?}, not line, start, end and setting"
        ));
    }
    if names(&mask["required"]) != set(&["line", "start", "end"]) {
        gaps.push("Mask requires something other than line, start and end".to_owned());
    }
    let u32_max = u64::from(u32::MAX);
    for (field, minimum) in [("line", 1), ("start", 0), ("end", 0)] {
        let p = &mask["properties"][field];
        if p["type"] != "integer" || p["minimum"] != minimum || p["maximum"] != u32_max {
            gaps.push(format!(
                "Mask.{field} is not an integer from {minimum} to {u32_max}"
            ));
        }
    }
    // `setting` is the one that is optional: a plain text has none.
    let setting = mask["properties"]["setting"]["oneOf"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let is_ref = |a: &Value| a["$ref"] == format!("{SCHEMA_REF}SettingPath");
    let is_null = |a: &Value| a["type"] == "null";
    if setting.len() != 2 || !setting.iter().any(is_ref) || !setting.iter().any(is_null) {
        gaps.push("Mask.setting is not a SettingPath or null".to_owned());
    }
    // The units and the meaning of every number, as the client reads them.
    say(
        &mut gaps,
        "Mask",
        &own_description(mask),
        &[
            "utf-8 byte",
            "0-based",
            "half-open",
            "character boundar",
            "`text`",
            "text[start..end]",
            MARKER_SHAPE,
            "holds no value",
        ],
    );
    say(
        &mut gaps,
        "Mask.line",
        &descriptions(&mask["properties"]["line"]),
        &["1-based", "`text`"],
    );
    for field in ["start", "end"] {
        say(
            &mut gaps,
            &format!("Mask.{field}"),
            &descriptions(&mask["properties"][field]),
            &["utf-8 byte", "`text`"],
        );
    }
    say(
        &mut gaps,
        "Mask.setting",
        &descriptions(&mask["properties"]["setting"]),
        &["revealValue", "FileContent.path"],
    );
    gaps
}

// No field of a mask can hold a value or any part of one, whatever it is called and however deep it is.
fn mask_value_gaps(d: &Value) -> Vec<String> {
    let mut gaps = Vec::new();
    let Some(mask) = d["components"]["schemas"].get("Mask") else {
        return vec!["schema Mask is not in the document".to_owned()];
    };
    // Every field is a number, or the name of a setting. A string that is not a `SettingPath` could hold text.
    for (field, p) in mask["properties"].as_object().into_iter().flatten() {
        let numeric = p["type"] == "integer";
        let setting = field == "setting";
        if !numeric && !setting {
            gaps.push(format!("Mask.{field} is neither a number nor the setting"));
        }
        for banned in ["value", "text", "secret", "plain", "raw", "original", "content"] {
            if field.contains(banned) {
                gaps.push(format!("Mask.{field} is named like a value (`{banned}`)"));
            }
        }
    }
    gaps
}

// ---------------------------------------------------------------------------------------------------------------------
// The three schemas that carry masks
// ---------------------------------------------------------------------------------------------------------------------

// What is wrong with where `masks` sits: exactly on the three schemas, optional, bounded, and nowhere else.
fn carrier_gaps(d: &Value) -> Vec<String> {
    let mut gaps = Vec::new();
    let schemas = &d["components"]["schemas"];

    for (name, _) in CARRIERS {
        let Some(schema) = schemas.get(name) else {
            gaps.push(format!("schema {name} is not in the document"));
            continue;
        };
        let masks = &schema["properties"]["masks"];
        if masks.is_null() {
            gaps.push(format!("{name}.masks is not in the document"));
            continue;
        }
        if masks["type"] != "array" || masks["items"]["$ref"] != MASK_REF {
            gaps.push(format!("{name}.masks is not an array of Mask"));
        }
        if masks["maxItems"] != 1000 {
            gaps.push(format!("{name}.masks has no bound of 1,000"));
        }
        if schema["additionalProperties"] != false {
            gaps.push(format!("{name} is not closed, so `masks` would be undeclared"));
        }
        say(
            &mut gaps,
            &format!("{name}.masks"),
            &descriptions(masks),
            &[MARKER_SHAPE, "`text`", "1,000"],
        );
    }

    // `masks` is on these three and nowhere else, and `Mask` is used by these three and nowhere else.
    let carriers: BTreeSet<&str> = CARRIERS.iter().map(|(n, _)| *n).collect();
    for (name, schema) in schemas.as_object().into_iter().flatten() {
        if schema["properties"].get("masks").is_some() && !carriers.contains(name.as_str()) {
            gaps.push(format!("{name} has a `masks` property, only three schemas may"));
        }
    }
    let users: Vec<String> = refs(d)
        .into_iter()
        .filter(|(_, r)| r == MASK_REF)
        .map(|(at, _)| at)
        .collect();
    let expected: BTreeSet<String> = CARRIERS
        .iter()
        .map(|(n, _)| format!("/components/schemas/{n}/properties/masks/items"))
        .collect();
    let found: BTreeSet<String> = users.iter().cloned().collect();
    if found != expected || users.len() != expected.len() {
        gaps.push(format!(
            "Mask is used at {found:?}, not by the items of the three `masks`"
        ));
    }

    // Additive only: `masks` is never required, and what the schemas required before they still require.
    for (name, required) in REQUIRED {
        let Some(schema) = schemas.get(name) else {
            continue;
        };
        let now = names(&schema["required"]);
        if now != set(required) {
            gaps.push(format!(
                "{name} requires {now:?}, not {:?}: a response must not gain a required property",
                set(required)
            ));
        }
    }
    gaps
}

// ---------------------------------------------------------------------------------------------------------------------
// Every place text can be
// ---------------------------------------------------------------------------------------------------------------------

// Whether a property is text itself or the list of diff hunks, and not a reference to another schema (which says it for itself).
fn holds_text(name: &str, p: &Value) -> bool {
    if name == "hunks" {
        return true;
    }
    if p.get("$ref").is_some() {
        return false;
    }
    if let Some(arms) = p["oneOf"].as_array() {
        return arms.iter().any(|a| holds_text(name, a));
    }
    match p["type"].as_str() {
        Some("string") => true,
        Some("array") => holds_text(name, &p["items"]),
        _ => false,
    }
}

// What is wrong with the way the schemas that hold text say how it is masked.
fn text_stance_gaps(d: &Value) -> Vec<String> {
    let mut gaps = Vec::new();
    let schemas = &d["components"]["schemas"];

    for (name, schema) in schemas.as_object().into_iter().flatten() {
        for (prop, p) in schema["properties"].as_object().into_iter().flatten() {
            if TEXT_NAMES.contains(&prop.as_str())
                && holds_text(prop, p)
                && !TEXT_BEARING.iter().any(|(n, q, _)| n == name && q == prop)
            {
                gaps.push(format!(
                    "{name}.{prop} can hold file or doc text and is not in the table: say whether it is masked"
                ));
            }
        }
    }

    for (name, prop, stance) in TEXT_BEARING {
        let Some(p) = schemas.get(name).and_then(|s| s["properties"].get(prop)) else {
            gaps.push(format!("{name}.{prop} is not in the document"));
            continue;
        };
        let said = format!("{}\n{}", own_description(&schemas[name]), descriptions(p));
        match stance {
            Stance::Masked => say(
                &mut gaps,
                &format!("{name}.{prop}"),
                &said,
                &[MARKER_SHAPE, "never sent"],
            ),
            Stance::Raw(word) => say(&mut gaps, &format!("{name}.{prop}"), &said, &[word]),
        }
    }
    gaps
}

// ---------------------------------------------------------------------------------------------------------------------
// The rule for a write, the rule for the editor, and what the document says first
// ---------------------------------------------------------------------------------------------------------------------

// What is missing from the prose a reader of the document meets: the front page, the read that hands out text, and each write
// that could be handed a masked copy.
fn rule_gaps(d: &Value) -> Vec<String> {
    let mut gaps = Vec::new();

    say(
        &mut gaps,
        "info.description",
        &own_description(&d["info"]),
        &[
            "S14b",
            MARKER_SHAPE,
            "never sends",
            "`masks`",
            "content_withheld",
            "revealValue",
            "redaction_marker",
        ],
    );
    say(
        &mut gaps,
        "the ETag header",
        &own_description(&d["components"]["headers"]["ETag"]),
        &["secret-scan configuration", "stale 304"],
    );

    // The editor's rule: Edit mode is off while a text is masked.
    let carrier = &d["components"]["schemas"]["FileContent"]["properties"]["masks"];
    say(
        &mut gaps,
        "FileContent.masks",
        &descriptions(carrier),
        &["edit mode is off while", "editFile"],
    );
    match operation(d, "getFileContent") {
        Some(op) => say(
            &mut gaps,
            "getFileContent",
            &own_description(op),
            &["`masks`", "editFile", "read-only", "never sent"],
        ),
        None => gaps.push("getFileContent is not in the document".to_owned()),
    }

    // The server's rule: a write that holds a marker is refused before anything is claimed or written.
    for id in TEXT_WRITES {
        let Some(op) = operation(d, id) else {
            gaps.push(format!("{id} is not in the document"));
            continue;
        };
        say(
            &mut gaps,
            id,
            &own_description(op),
            &[MARKER_SHAPE, "422", "invalid_content", "redaction_marker"],
        );
        if op["responses"].get("422").is_none() {
            gaps.push(format!("{id} does not document the 422 it refuses with"));
        }
    }

    // A blob is the file's own bytes under an immutable hash, so it cannot be masked: it is not served for a flagged text.
    match operation(d, "getBlob") {
        Some(op) => say(
            &mut gaps,
            "getBlob",
            &own_description(op),
            &["secret scan", "404", "getFileContent"],
        ),
        None => gaps.push("getBlob is not in the document".to_owned()),
    }
    gaps
}

// ---------------------------------------------------------------------------------------------------------------------
// The issue kind, in the domain and in the document
// ---------------------------------------------------------------------------------------------------------------------

// What is wrong with `redaction_marker` as an issue kind: it must be the same in the domain enum and in the OpenAPI enum.
fn issue_kind_gaps(d: &Value) -> Vec<String> {
    let mut gaps = Vec::new();
    let kind = &d["components"]["schemas"]["ValidationIssue"]["properties"]["kind"];
    let documented = names(&kind["enum"]);
    let in_domain: BTreeSet<String> = IssueKind::ALL.iter().map(|k| k.as_str().to_owned()).collect();
    if documented != in_domain {
        gaps.push(format!(
            "ValidationIssue.kind lists {documented:?} and the domain's IssueKind has {in_domain:?}"
        ));
    }
    if !documented.contains("redaction_marker") {
        gaps.push("ValidationIssue.kind does not list redaction_marker".to_owned());
    }
    say(
        &mut gaps,
        "ValidationIssue.kind",
        &descriptions(kind),
        &["redaction_marker", "S14b", "line"],
    );
    gaps
}

// ---------------------------------------------------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn masks_sit_on_exactly_the_three_schemas_and_are_optional() {
    let gaps = carrier_gaps(&doc());
    assert!(gaps.is_empty(), "{gaps:#?}");
}

#[test]
fn mask_fields_and_bounds_are_the_ones_the_client_reads() {
    let gaps = mask_gaps(&doc());
    assert!(gaps.is_empty(), "{gaps:#?}");
}

#[test]
fn mask_has_no_value_field() {
    let gaps = mask_value_gaps(&doc());
    assert!(gaps.is_empty(), "{gaps:#?}");
}

#[test]
fn every_text_bearing_schema_states_how_it_is_masked() {
    let gaps = text_stance_gaps(&doc());
    assert!(gaps.is_empty(), "{gaps:#?}");
}

#[test]
fn edit_mode_and_the_marker_refusal_are_stated_where_they_apply() {
    let gaps = rule_gaps(&doc());
    assert!(gaps.is_empty(), "{gaps:#?}");
}

#[test]
fn redaction_marker_is_an_issue_kind_of_the_domain_and_the_document() {
    let gaps = issue_kind_gaps(&doc());
    assert!(gaps.is_empty(), "{gaps:#?}");
    // The wire name, from the domain's side.
    assert_eq!(IssueKind::RedactionMarker.as_str(), "redaction_marker");
    assert_eq!(
        serde_json::to_value(IssueKind::RedactionMarker).unwrap(),
        json!("redaction_marker")
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Teeth: each check finds the damage it is there for
// ---------------------------------------------------------------------------------------------------------------------

fn damaged(change: impl FnOnce(&mut Value)) -> Value {
    let mut d = doc();
    change(&mut d);
    d
}

fn found(gaps: &[String], word: &str) -> bool {
    gaps.iter().any(|g| g.contains(word))
}

#[test]
fn the_carrier_check_finds_a_missing_masks_a_fourth_carrier_and_a_required_one() {
    let d = damaged(|d| {
        d["components"]["schemas"]["FileContent"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove("masks");
    });
    assert!(found(
        &carrier_gaps(&d),
        "FileContent.masks is not in the document"
    ));

    // A fourth schema that carries masks.
    let d = damaged(|d| {
        d["components"]["schemas"]["TextHit"]["properties"]["masks"] =
            d["components"]["schemas"]["DocText"]["properties"]["masks"].clone();
    });
    let gaps = carrier_gaps(&d);
    assert!(found(&gaps, "TextHit has a `masks` property"), "{gaps:#?}");
    assert!(found(&gaps, "Mask is used at"), "{gaps:#?}");

    // `masks` becomes required: an old body would be invalid.
    let d = damaged(|d| {
        d["components"]["schemas"]["DocText"]["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("masks"));
    });
    assert!(found(&carrier_gaps(&d), "DocText requires"));

    // The bound goes.
    let d = damaged(|d| {
        d["components"]["schemas"]["ServedFile"]["properties"]["masks"]
            .as_object_mut()
            .unwrap()
            .remove("maxItems");
    });
    assert!(found(&carrier_gaps(&d), "ServedFile.masks has no bound"));

    // An old response property turns optional: a client that relied on it would break, and the pin says so.
    let d = damaged(|d| {
        d["components"]["schemas"]["FileContent"]["required"]
            .as_array_mut()
            .unwrap()
            .retain(|r| r != "content_withheld");
    });
    assert!(found(&carrier_gaps(&d), "FileContent requires"));
}

#[test]
fn the_mask_check_finds_a_wrong_field_a_wrong_bound_and_a_missing_unit() {
    // A field that is not one of the four.
    let d = damaged(|d| {
        d["components"]["schemas"]["Mask"]["properties"]["rule"] = json!({"type": "string", "maxLength": 64});
    });
    assert!(found(&mask_gaps(&d), "Mask declares"));

    // The characters-and-columns shape of the first draft.
    let d = damaged(|d| {
        let p = d["components"]["schemas"]["Mask"]["properties"]
            .as_object_mut()
            .unwrap();
        let start = p.remove("start").unwrap();
        p.insert("column".into(), start);
    });
    assert!(found(&mask_gaps(&d), "Mask declares"));

    // A line can be 0.
    let d = damaged(|d| {
        d["components"]["schemas"]["Mask"]["properties"]["line"]["minimum"] = json!(0);
    });
    assert!(found(&mask_gaps(&d), "Mask.line is not an integer"));

    // An offset without a ceiling.
    let d = damaged(|d| {
        d["components"]["schemas"]["Mask"]["properties"]["end"]
            .as_object_mut()
            .unwrap()
            .remove("maximum");
    });
    assert!(found(&mask_gaps(&d), "Mask.end is not an integer"));

    // The unit is no longer said.
    let d = damaged(|d| {
        d["components"]["schemas"]["Mask"]["description"] = json!("Where a marker sits.");
    });
    let gaps = mask_gaps(&d);
    assert!(found(&gaps, "utf-8 byte"), "{gaps:#?}");
    assert!(found(&gaps, "half-open"), "{gaps:#?}");

    // `setting` becomes a free string.
    let d = damaged(|d| {
        d["components"]["schemas"]["Mask"]["properties"]["setting"] =
            json!({"type": "string", "maxLength": 10});
    });
    assert!(found(&mask_gaps(&d), "Mask.setting is not a SettingPath or null"));

    // A mask must not become required of its setting, nor open up.
    let d = damaged(|d| {
        d["components"]["schemas"]["Mask"]["additionalProperties"] = json!(true);
    });
    assert!(found(&mask_gaps(&d), "not a closed object"));
}

#[test]
fn the_no_value_check_finds_a_field_that_could_hold_the_value() {
    for field in ["value", "text", "original", "secret_text"] {
        let d = damaged(|d| {
            d["components"]["schemas"]["Mask"]["properties"][field] =
                json!({"type": "string", "maxLength": 100});
        });
        assert!(
            found(&mask_value_gaps(&d), &format!("Mask.{field}")),
            "a `{field}` field on Mask was not found"
        );
    }
}

#[test]
fn the_stance_check_finds_an_unlisted_text_a_lost_statement_and_a_missing_reason() {
    // A schema added later with a `text` property that nobody classified.
    let d = damaged(|d| {
        d["components"]["schemas"]["LogLine"] = json!({
            "type": "object",
            "properties": {"text": {"type": "string", "maxLength": 100}},
            "additionalProperties": false,
        });
    });
    assert!(found(
        &text_stance_gaps(&d),
        "LogLine.text can hold file or doc text"
    ));

    // A list of strings, and the list of hunks, count as text too.
    let d = damaged(|d| {
        d["components"]["schemas"]["Context"] = json!({
            "type": "object",
            "properties": {
                "lines": {"type": "array", "items": {"type": "string", "maxLength": 10}, "maxItems": 5},
                "after": {"type": "array", "items": {"type": "string", "maxLength": 10}, "maxItems": 5},
                "hunks": {"type": "array", "items": {"$ref": "#/components/schemas/DiffHunk"}, "maxItems": 5},
            },
            "additionalProperties": false,
        });
    });
    let gaps = text_stance_gaps(&d);
    assert!(found(&gaps, "Context.after can hold"), "{gaps:#?}");
    assert!(found(&gaps, "Context.hunks can hold"), "{gaps:#?}");

    // The marker sentence goes from the most used text, from a hunk list and from an audit diff.
    for (schema, prop) in [("DiffLine", "text"), ("Proposal", "hunks"), ("TextHit", "text")] {
        let d = damaged(|d| {
            d["components"]["schemas"][schema]["properties"][prop]["description"] =
                json!("The text, exactly as written.");
        });
        assert!(
            found(&text_stance_gaps(&d), &format!("{schema}.{prop} must say")),
            "{schema}.{prop}: a lost statement was not found"
        );
    }
    let d = damaged(|d| {
        d["components"]["schemas"]["AuditEvent"]["properties"]["diff"]["oneOf"][0]["description"] =
            json!("The change's own diff text.");
    });
    assert!(found(&text_stance_gaps(&d), "AuditEvent.diff must say"));

    // The one raw value must say that it is raw.
    let d = damaged(|d| {
        d["components"]["schemas"]["RevealedValue"]["properties"]["value"]["description"] =
            json!("The value.");
    });
    assert!(found(
        &text_stance_gaps(&d),
        "RevealedValue.value must say `unmasked`"
    ));
}

#[test]
fn the_rule_check_finds_a_lost_edit_rule_a_lost_refusal_and_a_lost_blob_rule() {
    let d = damaged(|d| {
        d["paths"]["/api/v1/swimlanes/{id}/files"]["put"]["description"] =
            json!("Carries the expected hash.");
    });
    assert!(found(&rule_gaps(&d), "editFile must say"));

    let d = damaged(|d| {
        d["paths"]["/api/v1/drafts"]["post"]["description"] = json!("Computes the diff.");
    });
    assert!(found(&rule_gaps(&d), "createDraft must say"));

    let d = damaged(|d| {
        d["paths"]["/api/v1/swimlanes/{id}/files/content"]["get"]["description"] =
            json!("Denied files return `content_withheld: true`.");
    });
    assert!(found(&rule_gaps(&d), "getFileContent must say"));

    let d = damaged(|d| {
        d["components"]["schemas"]["FileContent"]["properties"]["masks"]["description"] =
            json!("Where the markers sit in `text`, at most 1,000: [REDACTED: N chars, rule].");
    });
    assert!(found(
        &rule_gaps(&d),
        "FileContent.masks must say `edit mode is off while`"
    ));

    let d = damaged(|d| {
        d["paths"]["/api/v1/blobs/{hash}"]["get"]["description"] =
            json!("Never serves denied or withheld files (404).");
    });
    assert!(found(&rule_gaps(&d), "getBlob must say"));

    let d = damaged(|d| {
        d["info"]["description"] = json!("The REST contract.");
    });
    assert!(found(&rule_gaps(&d), "info.description must say"));

    let d = damaged(|d| {
        d["components"]["headers"]["ETag"]["description"] = json!("Strong validator.");
    });
    assert!(found(&rule_gaps(&d), "the ETag header must say"));

    // A write that stops documenting its 422 cannot say what it refuses with.
    let d = damaged(|d| {
        d["paths"]["/api/v1/swimlanes/{id}/files"]["put"]["responses"]
            .as_object_mut()
            .unwrap()
            .remove("422");
    });
    assert!(found(&rule_gaps(&d), "editFile does not document the 422"));
}

#[test]
fn the_issue_kind_check_finds_a_kind_that_the_two_sides_do_not_share() {
    // The document loses the value.
    let d = damaged(|d| {
        d["components"]["schemas"]["ValidationIssue"]["properties"]["kind"]["enum"]
            .as_array_mut()
            .unwrap()
            .retain(|k| k != "redaction_marker");
    });
    let gaps = issue_kind_gaps(&d);
    assert!(found(&gaps, "does not list redaction_marker"), "{gaps:#?}");
    assert!(found(&gaps, "the domain's IssueKind has"), "{gaps:#?}");

    // The document gains a value the domain does not have.
    let d = damaged(|d| {
        d["components"]["schemas"]["ValidationIssue"]["properties"]["kind"]["enum"]
            .as_array_mut()
            .unwrap()
            .push(json!("cosmic_ray"));
    });
    assert!(found(&issue_kind_gaps(&d), "the domain's IssueKind has"));

    // The kind stops saying what it is for.
    let d = damaged(|d| {
        d["components"]["schemas"]["ValidationIssue"]["properties"]["kind"]
            .as_object_mut()
            .unwrap()
            .remove("description");
    });
    assert!(found(&issue_kind_gaps(&d), "ValidationIssue.kind must say"));
}
