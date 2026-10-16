//! The final sweep (S14b): the last look at everything that is about to leave, whatever built it.
//!
//! A tool shapes file and doc text with [`RedactedText`](super::RedactedText), whose only constructor runs the secret scan. That is the
//! control, and a source scan holds that nothing else builds one. The sweep is the net under it, and it exists because a control that
//! is one habit is not enough for a secret: a new tool, a field of a type that is not `RedactedText` (a title, a message, a value the
//! port gave), a port that put text where only a name belongs, or a change that builds a result by hand would otherwise send a secret
//! to a model provider and nobody would know.
//!
//! After a result is built and cut to its cap, [`sweep`] scans, with the same scan and the same configuration, every string of
//! `data` and the `summary` of the envelope. Anything the scan finds is replaced by the marker the scan makes, and the sweep says how
//! many values it replaced. The caller counts that, because a value found here is a defect: the typed path should have taken it out
//! already (`mcp_redaction_late_total`).
//!
//! **The `content` text.** A client may show the model `content` in place of `structuredContent` (A6), and in data-block mode it holds
//! the data a second time, as JSON. When the sweep replaced something in the data, `content` is built again from the corrected data
//! with the same code that built it (`render`), and not patched: JSON escapes put letters next to a secret (`\n` before a key is an `n`
//! beside it), so a scan of the JSON text can miss what a scan of the string finds. The `content` that comes out is then scanned as
//! well, as a net under the net: anything it finds is replaced in place.
//!
//! **What is not scanned.** The cursor (`next`) and the link (`web_link`) are made by the server from typed parts and look like
//! random strings by design, so they would be flagged, and they hold no file or doc text. Keys of objects are the names of the contract.
//!
//! **Fail closed.** If the scan cannot finish on a string (it is larger than the scan takes, or holds more matches than it keeps) there
//! is no result: the caller gets [`RedactError`] and sends an error, never the text.
//!
//! **Cost.** One pass over the bytes of a result, which is at most 16 KB (the cap), so about a tenth of a millisecond; a string is
//! only copied when something in it is replaced, which is the case that should not happen.

use serde_json::Value;

use super::envelope::{DATA_NOTICE, Parts, Rendered, Shape, measure, render};
use super::redact::{RedactError, Redactor};
use crate::config::ContentMode;

/// A result after the sweep, and how many values the sweep replaced in it.
#[derive(Debug, Clone)]
pub struct Swept {
    pub rendered: Rendered,
    /// Values replaced in `structuredContent`, once each, and any the net under it found in `content`. Zero for every result that
    /// the typed path built right.
    pub late: u32,
}

/// Replace what the scan finds in `text`. Returns how many values it replaced.
fn fix(redactor: &Redactor, text: &mut String) -> Result<u32, RedactError> {
    // The common case allocates nothing: a scan that finds nothing leaves the string as it is.
    if redactor.scan(text)?.is_empty() {
        return Ok(0);
    }
    let redacted = redactor.text(text)?;
    *text = redacted.text.into_string();
    Ok(redacted.redacted)
}

/// The envelope's parts, read back from its JSON.
fn parts_of(structured: &Value) -> Option<Parts> {
    let envelope = structured.as_object()?;
    let text = |key: &str| envelope.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Parts {
        summary: text("summary")?,
        data: envelope.get("data")?.clone(),
        truncated: envelope.get("truncated")?.as_bool()?,
        next: text("next"),
        web_link: text("web_link"),
    })
}

/// Sweep a result. `mode` is how the server delivers text (`ContentMode`), which the result was built for.
///
/// # Errors
///
/// [`RedactError`] when the scan cannot finish on one of the strings. Nothing of the result is then sent.
pub fn sweep(redactor: &Redactor, mode: ContentMode, mut rendered: Rendered) -> Result<Swept, RedactError> {
    let mut late = 0_u32;
    if let Value::Object(envelope) = &mut rendered.structured {
        if let Some(Value::String(summary)) = envelope.get_mut("summary") {
            late = late.saturating_add(fix(redactor, summary)?);
        }
        if let Some(data) = envelope.get_mut("data") {
            // Every string of `data`, at any depth, without recursion: the depth is not ours to trust.
            let mut stack: Vec<&mut Value> = vec![data];
            while let Some(value) = stack.pop() {
                match value {
                    Value::String(text) => late = late.saturating_add(fix(redactor, text)?),
                    Value::Array(items) => stack.extend(items.iter_mut()),
                    Value::Object(map) => stack.extend(map.values_mut()),
                    Value::Null | Value::Bool(_) | Value::Number(_) => {}
                }
            }
        }
    }
    if late > 0 {
        // What the client shows in place of the data is made again from the corrected data, by the code that made it first. The
        // shape is the one it had: it held the data notice if and only if the result was text-bearing.
        let shape = Shape {
            mode,
            cap: usize::MAX,
            text_bearing: rendered.content.contains(DATA_NOTICE),
        };
        match parts_of(&rendered.structured) {
            Some(parts) => rendered = render(&shape, parts),
            // An envelope that is not one (a defect): nothing of the old `content` can be trusted, so it goes.
            None => rendered.content = String::new(),
        }
    }
    // The net under the net: whatever the scan still finds in `content` is replaced where it stands.
    let in_content = fix(redactor, &mut rendered.content)?;
    late = late.saturating_add(in_content);
    if in_content > 0 {
        rendered.bytes = measure(&rendered.structured, &rendered.content);
    }
    Ok(Swept { rendered, late })
}
