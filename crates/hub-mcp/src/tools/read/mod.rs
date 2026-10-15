//! The twenty read tools.
//!
//! Each tool declares the shape of its `data` next to the code that fills it, so those structs follow statements.
#![allow(clippy::items_after_statements)]

pub mod compare;
pub mod docs;
pub mod effective;
pub mod files;
pub mod findings;
pub mod search;
pub mod swimlanes;

use serde::Serialize;
use serde_json::Value;

/// `value` as JSON. The output types are plain data with string keys, so this does not fail; if it ever did, the result would be
/// `null`, which the schema validation in the tests would catch.
pub(crate) fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
