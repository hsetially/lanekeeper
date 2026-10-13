//! What a scalar is: its type tag and its flag (`docs/domain-model.md`: "Scalars keep their type tag and their source
//! text", "Don't treat `yes` or `on` as equal to `true`. Flag these values instead").
//!
//! The types are those of the YAML 1.2 core schema, which JSON is a subset of. Only a plain scalar is read as a number, a
//! boolean or null; a quoted one is text. Two rules go beyond the schema, because the services read YAML with `SnakeYAML`
//! (YAML 1.1) and the engine must not claim more than both readings agree on:
//!
//! - `yes`, `no`, `on` and `off` (in the three spellings `SnakeYAML` knows) stay strings and are flagged `YamlBooleanLike`;
//! - a number written with a leading zero (`0755`) is a string: 1.2 reads it as 755 and 1.1 as octal 493.

use domain::{ValueFlag, ValueType};

use super::node::{Scalar, ScalarStyle, ScalarTag};

/// The type and the flag of a scalar.
pub(super) fn classify(scalar: &Scalar) -> (ValueType, Option<ValueFlag>) {
    match scalar.tag {
        ScalarTag::Str => return (ValueType::String, None),
        ScalarTag::Int => return (ValueType::Int, None),
        ScalarTag::Float => return (ValueType::Float, None),
        ScalarTag::Bool => return (ValueType::Bool, None),
        ScalarTag::Null => return (ValueType::Null, None),
        ScalarTag::Other => return (ValueType::Other, None),
        ScalarTag::None => {}
    }
    if scalar.style != ScalarStyle::Plain {
        return (ValueType::String, None);
    }
    let text = scalar.text();
    if is_boolean_like(text) {
        return (ValueType::String, Some(ValueFlag::YamlBooleanLike));
    }
    (plain_type(text), None)
}

fn is_boolean_like(text: &str) -> bool {
    matches!(
        text,
        "yes" | "Yes" | "YES" | "no" | "No" | "NO" | "on" | "On" | "ON" | "off" | "Off" | "OFF"
    )
}

fn plain_type(text: &str) -> ValueType {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => ValueType::Null,
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => ValueType::Bool,
        _ if is_int(text) => ValueType::Int,
        _ if is_float(text) => ValueType::Float,
        _ => ValueType::String,
    }
}

fn is_int(text: &str) -> bool {
    if let Some(hex) = text.strip_prefix("0x") {
        return !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    if let Some(octal) = text.strip_prefix("0o") {
        return !octal.is_empty() && octal.bytes().all(|b| (b'0'..=b'7').contains(&b));
    }
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    // `0755` is neither 1.2's 755 nor 1.1's octal: it is left as text.
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits.len() == 1 || !digits.starts_with('0'))
}

fn is_float(text: &str) -> bool {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    if matches!(body, ".inf" | ".Inf" | ".INF") {
        return true;
    }
    // The core schema gives a sign to infinity and not to NaN (`+.NaN` is text), so this is `text` and not `body`.
    if matches!(text, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    // ( \. [0-9]+ | [0-9]+ ( \. [0-9]* )? ) ( [eE] [-+]? [0-9]+ )?
    let bytes = body.as_bytes();
    let digits = |from: usize| bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    // A float has a fraction or an exponent: digits alone are an integer (or, with a leading zero, text).
    let mut at;
    let mut fraction = false;
    if bytes.first() == Some(&b'.') {
        let n = digits(1);
        if n == 0 {
            return false;
        }
        at = 1 + n;
        fraction = true;
    } else {
        let n = digits(0);
        if n == 0 {
            return false;
        }
        at = n;
        if bytes.get(at) == Some(&b'.') {
            at += 1 + digits(at + 1);
            fraction = true;
        }
    }
    let mut exponent = false;
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let n = digits(at);
        if n == 0 {
            return false;
        }
        at += n;
        exponent = true;
    }
    (fraction || exponent) && at == bytes.len()
}
