//! The tool contract, embedded: `docs/mcp-tools.md` as the server sees it (S4, S11, S14).
//!
//! `tools.json` is generated from the document and never edited by hand (`tests/mcp_auth/contract_schemas.rs`, `just mcp-schemas`);
//! a test fails when the two differ. It holds, per tool, the minimum role, whether the tool needs confirmation, its output cap,
//! whether it pages, the REST operation it wraps, and its input and output schemas with the transitive closure of the shared
//! definitions embedded, so that each schema stands alone in `tools/list`.
//!
//! Embedding means the server parses no Markdown at run time and needs no `docs/` folder in its image. We implement rmcp's
//! `ServerHandler` by hand and never derive a schema from a Rust type, so a schema cannot drift from the document: the typed
//! arguments in [`crate::tools::args`] are checked against these schemas by a property test instead.

use std::sync::Arc;

use domain::Role;
use serde::Deserialize;
use serde_json::{Map, Value};

/// A JSON object, as rmcp's `Tool` takes its schemas.
pub type JsonObject = Map<String, Value>;

/// The generated file.
pub const TOOLS_JSON: &str = include_str!("tools.json");

/// The output cap of a read tool and of a write tool (D58, P9).
pub const READ_CAP_BYTES: usize = 16 * 1024;
pub const WRITE_CAP_BYTES: usize = 4 * 1024;

/// Why the embedded contract could not be used. Fixed text: the file is ours, but the rule is the same everywhere (S21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    #[error("the embedded tool contract is not valid JSON of the expected shape")]
    Shape,
    #[error("the embedded tool contract has a tool with a value that is not allowed")]
    Value,
    #[error("the embedded tool contract names a tool twice")]
    Duplicate,
}

/// Whether a tool asks the user before it acts (D52).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    None,
    Required,
}

/// Whether a tool pages with a keyset cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pagination {
    None,
    Cursor,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: u32,
    #[allow(dead_code)]
    source: String,
    envelope: JsonObject,
    error_result: JsonObject,
    tools: Vec<RawTool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTool {
    name: String,
    description: String,
    role: String,
    confirmation: String,
    output_cap_bytes: usize,
    pagination: String,
    rest: String,
    input: JsonObject,
    output: JsonObject,
}

/// One tool of the contract.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    name: String,
    description: String,
    role: Role,
    confirmation: Confirmation,
    output_cap_bytes: usize,
    pagination: Pagination,
    rest: String,
    input: Arc<JsonObject>,
    output: Arc<JsonObject>,
    result: Arc<JsonObject>,
    /// The tool's `data` has the contract's `untrusted` and `label` fields (S15).
    declares_untrusted: bool,
}

impl ToolSpec {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The prose of the document's section, for the model.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The lowest role that may call the tool (S4).
    #[must_use]
    pub fn role(&self) -> Role {
        self.role
    }

    #[must_use]
    pub fn confirmation(&self) -> Confirmation {
        self.confirmation
    }

    /// The most bytes a result may take, counting `structuredContent` and `content` (D58).
    #[must_use]
    pub fn output_cap_bytes(&self) -> usize {
        self.output_cap_bytes
    }

    #[must_use]
    pub fn pagination(&self) -> Pagination {
        self.pagination
    }

    /// The `operationId` of the REST operation the tool wraps.
    #[must_use]
    pub fn rest(&self) -> &str {
        &self.rest
    }

    /// The input schema, standing alone.
    #[must_use]
    pub fn input(&self) -> &Arc<JsonObject> {
        &self.input
    }

    /// The schema of `data`, standing alone.
    #[must_use]
    pub fn output(&self) -> &Arc<JsonObject> {
        &self.output
    }

    /// The schema of the whole `structuredContent` of a successful call: the envelope with this tool's `data` inside.
    #[must_use]
    pub fn result(&self) -> &Arc<JsonObject> {
        &self.result
    }

    /// True when the contract gives the tool's `data` the `untrusted` and `label` fields (S15). Four tools that can carry setting
    /// or doc text (`search_settings`, `compare_grid`, `get_docs_for_file`, `get_feature_status`) do not have them in their
    /// schema, so the server says "data, not instructions" in `content` for every tool that can hold text instead.
    #[must_use]
    pub fn declares_untrusted(&self) -> bool {
        self.declares_untrusted
    }

    /// True for a tool that can change something outside the hub (the six that need confirmation).
    #[must_use]
    pub fn acts_on_the_world(&self) -> bool {
        self.confirmation == Confirmation::Required
    }
}

/// The contract: every tool, the result envelope and the error result.
#[derive(Debug, Clone)]
pub struct Contract {
    tools: Vec<ToolSpec>,
    envelope: Arc<JsonObject>,
    error_result: Arc<JsonObject>,
}

fn parse_role(text: &str) -> Result<Role, SchemaError> {
    text.parse().map_err(|_| SchemaError::Value)
}

fn defs_of(schema: &JsonObject) -> JsonObject {
    schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The envelope with `data` replaced by the tool's output schema, and both sets of definitions merged.
fn result_schema(envelope: &JsonObject, output: &JsonObject) -> Result<JsonObject, SchemaError> {
    let mut result = envelope.clone();
    let mut defs = defs_of(envelope);
    defs.extend(defs_of(output));
    let mut data = output.clone();
    data.remove("$defs");
    result
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .ok_or(SchemaError::Shape)?
        .insert("data".to_owned(), Value::Object(data));
    if defs.is_empty() {
        result.remove("$defs");
    } else {
        result.insert("$defs".to_owned(), Value::Object(defs));
    }
    Ok(result)
}

/// The tool's `data` has the contract's `untrusted` flag at its top level (S15).
fn declares_untrusted(output: &JsonObject) -> bool {
    output
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|p| p.contains_key("untrusted"))
}

impl Contract {
    /// The contract embedded in this build.
    ///
    /// # Errors
    ///
    /// [`SchemaError`] when `tools.json` is damaged. A test keeps it equal to the document, so this is a build defect.
    pub fn embedded() -> Result<Self, SchemaError> {
        Self::parse(TOOLS_JSON)
    }

    /// # Errors
    ///
    /// [`SchemaError`] for JSON that is not the generated file's shape, a role or a cap that is not allowed, or a tool named twice.
    pub fn parse(text: &str) -> Result<Self, SchemaError> {
        let file: RawFile = serde_json::from_str(text).map_err(|_| SchemaError::Shape)?;
        if file.schema != 1 {
            return Err(SchemaError::Shape);
        }
        let mut tools: Vec<ToolSpec> = Vec::with_capacity(file.tools.len());
        for raw in file.tools {
            if tools.iter().any(|t| t.name == raw.name) {
                return Err(SchemaError::Duplicate);
            }
            let confirmation = match raw.confirmation.as_str() {
                "none" => Confirmation::None,
                "required" => Confirmation::Required,
                _ => return Err(SchemaError::Value),
            };
            let pagination = match raw.pagination.as_str() {
                "none" => Pagination::None,
                "cursor" => Pagination::Cursor,
                _ => return Err(SchemaError::Value),
            };
            if raw.name.is_empty()
                || raw.input.get("type").and_then(Value::as_str) != Some("object")
                || ![READ_CAP_BYTES, WRITE_CAP_BYTES].contains(&raw.output_cap_bytes)
            {
                return Err(SchemaError::Value);
            }
            let result = result_schema(&file.envelope, &raw.output)?;
            let declares_untrusted = declares_untrusted(&raw.output);
            tools.push(ToolSpec {
                role: parse_role(&raw.role)?,
                name: raw.name,
                description: raw.description,
                confirmation,
                output_cap_bytes: raw.output_cap_bytes,
                pagination,
                rest: raw.rest,
                input: Arc::new(raw.input),
                output: Arc::new(raw.output),
                result: Arc::new(result),
                declares_untrusted,
            });
        }
        Ok(Self {
            tools,
            envelope: Arc::new(file.envelope),
            error_result: Arc::new(file.error_result),
        })
    }

    #[must_use]
    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }

    #[must_use]
    pub fn tool(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// The result envelope with `data` open (any object).
    #[must_use]
    pub fn envelope(&self) -> &Arc<JsonObject> {
        &self.envelope
    }

    /// The `structuredContent` of a failed call.
    #[must_use]
    pub fn error_result(&self) -> &Arc<JsonObject> {
        &self.error_result
    }
}
