//! The property the fuzz target `mcp_tool_args` asserts (S11, S22): arbitrary JSON as the arguments of any of the 27 tools.
//!
//! Included, with `#[path]`, by `fuzz_targets/mcp_tool_args.rs` and by `tests/mcp_auth/corpus_replay.rs`, so the nightly fuzzer and
//! the stable seed replay check the same things. The includer provides one module at its root, `args`, which is
//! `src/tools/args.rs` (the fuzz target includes the file; the replay re-exports the library's).
//!
//! # The input
//!
//! The first byte picks the tool (modulo 27); the rest is JSON. An object is the `arguments`, `null` is a call with none, and
//! anything else (or text that is not JSON) is not an input of the protocol (rmcp only hands over an object or nothing) and is
//! passed over.
//!
//! # What must hold
//!
//! | Check | Why |
//! |---|---|
//! | parsing never panics and gives the same answer twice | a panic in the handler is a failed call for every caller of the replica |
//! | a key that the tool does not declare is `Invalid`, and nothing is accepted with one | `deny_unknown_fields` (S11) |
//! | an accepted call has every required argument, and its tool is the one asked for | a missing argument is `Missing` with the tool's own name for it |
//! | `Missing` names a required argument that is absent; `TooLarge` only follows `content` over 2 MiB | the error says only what the contract says |
//! | the text of an error is one of three fixed sentences | no input in an error (S21) |
//! | a present `null` is refused, except the PR `body` | the schemas have no `null` for an optional argument |
//! | the `Debug` of accepted arguments shows no `content` and no `body` | log lines and panic messages cannot hold file text (S10) |
//! | every accepted path is relative, normalised, without `..`, backslash or control character; every accepted docs path starts with `/` | paths are untrusted (S11) |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use serde_json::Value;

use super::args::{ArgError, ToolArgs, ToolName, declared, parse_args};

/// A seed corpus needs at least this many files per target (`MIN_SEEDS` in the replay test).
pub const MIN_SEEDS: usize = 8;

/// A check of one input.
pub type Check = fn(&[u8]) -> Result<(), String>;

const FIXED: [&str; 3] = [
    "an argument is not valid; see the input schema of the tool",
    "a required argument is missing",
    "the content is larger than 2 MiB",
];

fn relative_ok(what: &str, text: &str) -> Result<(), String> {
    if text.starts_with('/')
        || text.contains('\\')
        || text.contains("//")
        || text.chars().any(char::is_control)
    {
        return Err(format!(
            "accepted a {what} that is not relative and plain: {text:?}"
        ));
    }
    if text.split('/').any(|part| part == ".." || part == ".") {
        return Err(format!("accepted a {what} with a dot segment: {text:?}"));
    }
    Ok(())
}

fn docs_ok(text: &str) -> Result<(), String> {
    if !text.starts_with('/')
        || text.contains('\\')
        || text.contains("//")
        || text.chars().any(char::is_control)
    {
        return Err(format!(
            "accepted a docs path that is not absolute and plain: {text:?}"
        ));
    }
    if text.split('/').any(|part| part == ".." || part == ".") {
        return Err(format!("accepted a docs path with a dot segment: {text:?}"));
    }
    Ok(())
}

/// Every path-like argument of an accepted call.
fn check_paths(parsed: &ToolArgs) -> Result<(), String> {
    use ToolArgs as A;
    let nfs = |p: &domain::NfsPath| relative_ok("path", p.as_str());
    let logical = |p: &domain::LogicalFile| relative_ok("logical file", p.as_str());
    match parsed {
        A::ListFiles(a) => a.prefix.as_ref().map_or(Ok(()), |p| nfs(&p.0)),
        A::ReadFile(a) => nfs(&a.path.0),
        A::GetEffectiveConfig(a) => logical(&a.file.0),
        A::GetServedConfig(a) => nfs(&a.file.0),
        A::Compare(a) => logical(&a.file.0),
        A::CompareTree(a) => a.prefix.as_ref().map_or(Ok(()), |p| nfs(&p.0)),
        A::CompareGrid(a) => logical(&a.file.0),
        A::SearchSettings(a) => a.file.as_ref().map_or(Ok(()), |f| logical(&f.0)),
        A::GetDrift(a) => a.prefix.as_ref().map_or(Ok(()), |p| nfs(&p.0)),
        A::GetHistory(a) => nfs(&a.path.0),
        A::ReadDoc(a) => docs_ok(a.path.0.as_str()),
        A::GrepDocs(a) => a.path_prefix.as_ref().map_or(Ok(()), |p| docs_ok(p.0.as_str())),
        A::GetDocsForFile(a) => logical(&a.file.0),
        A::GetFeatureStatus(a) => a.file.as_ref().map_or(Ok(()), |f| logical(&f.0)),
        A::ProposeChange(a) => nfs(&a.path.0),
        A::UploadFile(a) => nfs(&a.path.0),
        A::DeleteFile(a) => nfs(&a.path.0),
        A::RaisePr(a) => a.paths.iter().try_for_each(|p| nfs(&p.0)),
        A::NotifyServices(a) => a.paths.iter().try_for_each(|p| nfs(&p.0)),
        _ => Ok(()),
    }
}

/// `mcp_tool_args`.
pub fn mcp_tool_args(data: &[u8]) -> Result<(), String> {
    let Some((&selector, json)) = data.split_first() else {
        return Ok(());
    };
    let tool = ToolName::ALL[usize::from(selector) % ToolName::ALL.len()];
    let Ok(value) = serde_json::from_slice::<Value>(json) else {
        return Ok(());
    };
    let arguments = match value {
        Value::Object(map) => Some(map),
        Value::Null => None,
        _ => return Ok(()),
    };
    let map = arguments.clone().unwrap_or_default();
    let (known, required) = declared(tool);

    let first = parse_args(tool, arguments.clone());
    let second = parse_args(tool, arguments);
    if first != second {
        return Err("parsing answered differently for the same arguments".to_owned());
    }
    let has_unknown = map.keys().any(|k| !known.contains(&k.as_str()));
    if has_unknown && first != Err(ArgError::Invalid) {
        return Err(format!(
            "an unknown argument was not refused as invalid: {first:?}"
        ));
    }
    match &first {
        Ok(parsed) => {
            if parsed.tool() != tool {
                return Err("the arguments are for another tool".to_owned());
            }
            if let Some(missing) = required.iter().find(|r| !map.contains_key(**r)) {
                return Err(format!("accepted without the required argument {missing}"));
            }
            for (key, value) in &map {
                if value.is_null() && !(tool == ToolName::RaisePr && key == "body") {
                    return Err(format!("accepted a null for {key}"));
                }
            }
            let shown = format!("{parsed:?}");
            for field in ["content", "body"] {
                if let Some(Value::String(text)) = map.get(field)
                    && text.len() >= 8
                    && shown.contains(text.as_str())
                {
                    return Err(format!(
                        "the Debug of accepted arguments shows the text of {field}"
                    ));
                }
            }
            check_paths(parsed)?;
        }
        Err(error) => {
            let said = error.to_string();
            if !FIXED.contains(&said.as_str()) {
                return Err(format!(
                    "an error message that is not one of the fixed sentences: {said}"
                ));
            }
            match error {
                ArgError::Missing(name) => {
                    if !required.contains(name) || map.contains_key(*name) {
                        return Err(format!(
                            "Missing({name}) for an argument that is not required or is present"
                        ));
                    }
                }
                ArgError::TooLarge => {
                    if !matches!(map.get("content"), Some(Value::String(c)) if c.len() > 2 * 1024 * 1024) {
                        return Err("TooLarge without content over 2 MiB".to_owned());
                    }
                }
                ArgError::Invalid => {}
            }
        }
    }
    Ok(())
}
