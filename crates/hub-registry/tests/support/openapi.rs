//! A small OpenAPI 3.1 schema walker (plan A43): enough of JSON Schema to say whether a response matches `api/openapi.yaml`, with no
//! JSON-schema crate. The spec is read with `serde-saphyr` (already in the workspace).
//!
//! What it checks: `$ref` (local), `type` (including `"null"`), `enum`, `const`, `required`, `properties`, `additionalProperties`
//! (boolean or schema), `minProperties`, `oneOf` (exactly one branch), `anyOf`, `allOf`, `items`, `prefixItems`, `minItems`,
//! `maxItems`, `uniqueItems`, `minLength`, `maxLength`, `pattern`, `minimum` and `maximum`, and `format: int64`.
//!
//! **`allOf` is the merge the spec means.** The spec writes an extension as `allOf: [Base, {properties: {..}, additionalProperties:
//! false}]`; read literally, the second member would forbid everything the first one defines. So the members of an `allOf` are
//! flattened into one object schema: the properties and required names of all of them, and extra properties are allowed only if some
//! member says `additionalProperties: true` (RFC 9457's `Problem` does) or if no member says anything.
//!
//! What it does not check: `format` other than `int64` (uri, uuid, email), `discriminator` (the `oneOf` it sits on is checked, and every
//! branch fixes its discriminating property with `const`), `readOnly` and `writeOnly`, and `not`.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unused_self,
    clippy::doc_markdown
)]

use std::collections::BTreeSet;
use std::path::Path;

use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use serde_json::Value;

/// The deepest chain of references and nested schemas followed: the spec's own deepest is under ten, so more is a cycle.
const MAX_DEPTH: usize = 48;

pub struct Spec {
    root: Value,
}

/// One declared parameter of an operation.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub location: String,
    pub required: bool,
    pub schema: Value,
}

impl Spec {
    /// `api/openapi.yaml` of this repository.
    pub fn load() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml");
        let text = std::fs::read_to_string(path).unwrap();
        Self::from_yaml(&text)
    }

    pub fn from_yaml(text: &str) -> Self {
        Self {
            root: serde_saphyr::from_str(text).unwrap(),
        }
    }

    pub fn from_value(root: Value) -> Self {
        Self { root }
    }

    // ------------------------------------------------------------------------------------------------ the document

    /// Follow `$ref`s (local ones only) until a schema that is not one.
    pub fn deref<'a>(&'a self, mut node: &'a Value) -> &'a Value {
        for _ in 0..MAX_DEPTH {
            let Some(target) = node.get("$ref").and_then(Value::as_str) else {
                return node;
            };
            let pointer = target
                .strip_prefix('#')
                .unwrap_or_else(|| panic!("only local references are followed: {target}"));
            node = self
                .root
                .pointer(pointer)
                .unwrap_or_else(|| panic!("dangling reference {target}"));
        }
        panic!("a reference chain longer than {MAX_DEPTH}: a cycle")
    }

    /// The operation with this method and path template.
    pub fn operation(&self, method: &str, path: &str) -> Option<&Value> {
        self.root.get("paths")?.get(path)?.get(method.to_lowercase())
    }

    /// The `operationId` of every operation, with its method and path.
    pub fn operation_ids(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (path, item) in self.root["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                if let Some(id) = op.get("operationId").and_then(Value::as_str) {
                    out.push((id.to_owned(), method.to_uppercase(), path.clone()));
                }
            }
        }
        out
    }

    /// The response the operation declares for `status`, or its `default`.
    pub fn response<'a>(&'a self, op: &'a Value, status: u16) -> Option<&'a Value> {
        let responses = op.get("responses")?;
        let declared = responses
            .get(status.to_string())
            .or_else(|| responses.get("default"))?;
        Some(self.deref(declared))
    }

    /// Whether the operation names `status` itself (not only through `default`).
    pub fn names_status(&self, op: &Value, status: u16) -> bool {
        op.get("responses")
            .and_then(|r| r.get(status.to_string()))
            .is_some()
    }

    /// The schema of a response body of this media type.
    pub fn body_schema<'a>(&'a self, response: &'a Value, media_type: &str) -> Option<&'a Value> {
        response.get("content")?.get(media_type)?.get("schema")
    }

    /// The media types of a response's body.
    pub fn media_types(&self, response: &Value) -> Vec<String> {
        response
            .get("content")
            .and_then(Value::as_object)
            .map(|c| c.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The headers a response declares, lower-cased.
    pub fn header_names(&self, response: &Value) -> Vec<String> {
        response
            .get("headers")
            .and_then(Value::as_object)
            .map(|h| h.keys().map(|k| k.to_lowercase()).collect())
            .unwrap_or_default()
    }

    /// The parameters of an operation (path, query and header), references resolved.
    pub fn parameters(&self, op: &Value) -> Vec<Param> {
        op.get("parameters")
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .map(|p| {
                        let p = self.deref(p);
                        Param {
                            name: p["name"].as_str().unwrap().to_owned(),
                            location: p["in"].as_str().unwrap().to_owned(),
                            required: p.get("required").and_then(Value::as_bool).unwrap_or(false),
                            schema: p["schema"].clone(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A named schema of `components/schemas`.
    pub fn schema(&self, name: &str) -> &Value {
        self.root
            .pointer(&format!("/components/schemas/{name}"))
            .unwrap_or_else(|| panic!("no schema {name}"))
    }

    // ------------------------------------------------------------------------------------------------ validation

    /// Every way `instance` fails `schema`, each as `pointer: what`. Empty means it matches.
    pub fn check(&self, schema: &Value, instance: &Value) -> Vec<String> {
        let mut out = Vec::new();
        self.walk(schema, instance, "", 0, &mut out);
        out
    }

    fn walk(&self, schema: &Value, instance: &Value, at: &str, depth: usize, out: &mut Vec<String>) {
        if depth > MAX_DEPTH {
            out.push(format!("{at}: schemas nested deeper than {MAX_DEPTH}"));
            return;
        }
        let schema = self.deref(schema);
        match schema {
            Value::Bool(true) => return,
            Value::Bool(false) => {
                out.push(format!("{at}: no value is allowed here"));
                return;
            }
            Value::Object(_) => {}
            _ => {
                out.push(format!("{at}: the schema is not an object"));
                return;
            }
        }
        let mut members = Vec::new();
        self.members(schema, &mut members, 0);
        self.object_rules(&members, instance, at, depth, out);
        for member in &members {
            self.common_rules(member, instance, at, depth, out);
        }
    }

    /// The schema and, recursively, the members of its `allOf`.
    fn members<'a>(&'a self, schema: &'a Value, out: &mut Vec<&'a Value>, depth: usize) {
        let schema = self.deref(schema);
        assert!(depth <= MAX_DEPTH, "allOf nested deeper than {MAX_DEPTH}");
        if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
            for member in all {
                self.members(member, out, depth + 1);
            }
        }
        out.push(schema);
    }

    /// `required`, `properties`, `additionalProperties` and `minProperties`, over the merged members.
    fn object_rules(
        &self,
        members: &[&Value],
        instance: &Value,
        at: &str,
        depth: usize,
        out: &mut Vec<String>,
    ) {
        let Some(object) = instance.as_object() else {
            return;
        };
        let mut properties: Vec<(&String, &Value)> = Vec::new();
        let mut required: BTreeSet<&str> = BTreeSet::new();
        let mut open = false;
        let mut closed = false;
        let mut extra_schema: Option<&Value> = None;
        for member in members {
            if let Some(p) = member.get("properties").and_then(Value::as_object) {
                properties.extend(p.iter());
            }
            if let Some(r) = member.get("required").and_then(Value::as_array) {
                required.extend(r.iter().filter_map(Value::as_str));
            }
            match member.get("additionalProperties") {
                Some(Value::Bool(true)) => open = true,
                Some(Value::Bool(false)) => closed = true,
                Some(schema @ Value::Object(_)) => extra_schema = Some(schema),
                _ => {}
            }
            if let Some(min) = member.get("minProperties").and_then(Value::as_u64) {
                if (object.len() as u64) < min {
                    out.push(format!("{at}: fewer than {min} properties"));
                }
            }
        }
        for name in &required {
            if !object.contains_key(*name) {
                out.push(format!("{at}: required property `{name}` is missing"));
            }
        }
        for (name, value) in object {
            let child = format!("{at}/{name}");
            let declared: Vec<&Value> = properties
                .iter()
                .filter(|(n, _)| *n == name)
                .map(|(_, s)| *s)
                .collect();
            if !declared.is_empty() {
                for schema in declared {
                    self.walk(schema, value, &child, depth + 1, out);
                }
            } else if open {
                // Anything is allowed.
            } else if let Some(schema) = extra_schema {
                self.walk(schema, value, &child, depth + 1, out);
            } else if closed {
                out.push(format!("{at}: property `{name}` is not in the schema"));
            }
        }
    }

    /// Everything that is not about the properties of an object.
    fn common_rules(&self, schema: &Value, instance: &Value, at: &str, depth: usize, out: &mut Vec<String>) {
        if let Some(types) = schema.get("type") {
            let allowed: Vec<&str> = match types {
                Value::String(t) => vec![t.as_str()],
                Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if !allowed.iter().any(|t| is_type(instance, t)) {
                out.push(format!("{at}: expected {allowed:?}, found {}", kind_of(instance)));
                // The rest of the rules assume the type.
                return;
            }
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            if !values.contains(instance) {
                out.push(format!(
                    "{at}: not one of the enum values ({})",
                    shorten(instance)
                ));
            }
        }
        if let Some(value) = schema.get("const") {
            if value != instance {
                out.push(format!("{at}: not the constant {value} ({})", shorten(instance)));
            }
        }
        if let Some(text) = instance.as_str() {
            self.string_rules(schema, text, at, out);
        }
        if let Some(number) = instance.as_f64() {
            Self::number_rules(schema, number, at, out);
        }
        if schema.get("format").and_then(Value::as_str) == Some("int64")
            && instance.is_number()
            && !is_type(instance, "integer")
        {
            out.push(format!("{at}: int64 is a whole number"));
        }
        if let Some(items) = instance.as_array() {
            self.array_rules(schema, items, at, depth, out);
        }
        if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
            let mut matching = 0;
            let mut why: Vec<String> = Vec::new();
            for branch in branches {
                let mut found = Vec::new();
                self.walk(branch, instance, at, depth + 1, &mut found);
                if found.is_empty() {
                    matching += 1;
                } else if why.len() < 3 {
                    why.push(found.remove(0));
                }
            }
            if matching != 1 {
                out.push(format!(
                    "{at}: {matching} of {} oneOf branches match (exactly one must) [{}]",
                    branches.len(),
                    why.join("; ")
                ));
            }
        }
        if let Some(branches) = schema.get("anyOf").and_then(Value::as_array) {
            let any = branches.iter().any(|b| {
                let mut found = Vec::new();
                self.walk(b, instance, at, depth + 1, &mut found);
                found.is_empty()
            });
            if !any {
                out.push(format!("{at}: none of the anyOf branches match"));
            }
        }
    }

    fn string_rules(&self, schema: &Value, text: &str, at: &str, out: &mut Vec<String>) {
        let chars = text.chars().count() as u64;
        if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
            if chars < min {
                out.push(format!("{at}: shorter than {min}"));
            }
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
            if chars > max {
                out.push(format!("{at}: longer than {max}"));
            }
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            let matcher = RegexMatcher::new(pattern).unwrap_or_else(|e| panic!("pattern {pattern}: {e}"));
            if !matcher.is_match(text.as_bytes()).unwrap() {
                out.push(format!(
                    "{at}: does not match {pattern} ({})",
                    shorten(&Value::String(text.to_owned()))
                ));
            }
        }
    }

    fn number_rules(schema: &Value, number: f64, at: &str, out: &mut Vec<String>) {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
            if number < min {
                out.push(format!("{at}: {number} is below {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
            if number > max {
                out.push(format!("{at}: {number} is above {max}"));
            }
        }
    }

    fn array_rules(&self, schema: &Value, items: &[Value], at: &str, depth: usize, out: &mut Vec<String>) {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < min {
                out.push(format!("{at}: fewer than {min} items ({})", items.len()));
            }
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) {
            if (items.len() as u64) > max {
                out.push(format!("{at}: more than {max} items ({})", items.len()));
            }
        }
        if schema.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
            let mut seen = BTreeSet::new();
            for item in items {
                if !seen.insert(item.to_string()) {
                    out.push(format!("{at}: items are not unique"));
                    break;
                }
            }
        }
        let prefix = schema.get("prefixItems").and_then(Value::as_array);
        for (i, item) in items.iter().enumerate() {
            let child = format!("{at}/{i}");
            if let Some(positional) = prefix.and_then(|p| p.get(i)) {
                self.walk(positional, item, &child, depth + 1, out);
            } else if let Some(each) = schema.get("items") {
                self.walk(each, item, &child, depth + 1, out);
            }
        }
        if let Some(positional) = prefix {
            if items.len() > positional.len() && schema.get("items").is_none() {
                // Past the positions, anything goes (JSON Schema 2020-12), unless `maxItems` said otherwise.
            }
        }
    }
}

fn is_type(instance: &Value, name: &str) -> bool {
    match name {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "boolean" => instance.is_boolean(),
        "null" => instance.is_null(),
        "number" => instance.is_number(),
        "integer" => instance.is_i64() || instance.is_u64(),
        _ => false,
    }
}

fn kind_of(instance: &Value) -> &'static str {
    match instance {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn shorten(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 60 {
        format!("{}...", text.chars().take(60).collect::<String>())
    } else {
        text
    }
}
