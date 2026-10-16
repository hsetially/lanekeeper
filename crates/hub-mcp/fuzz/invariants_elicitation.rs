//! The properties the fuzz target `mcp_elicitation_result` asserts (S14, S22): the answer of the client to the question "do you
//! confirm this?", read as a decision. The client's answer is untrusted input, and a mistake here is a write nobody agreed to.
//!
//! Included, with `#[path]`, by `fuzz_targets/mcp_elicitation_result.rs` and by `tests/mcp_auth/corpus_replay.rs`. The includer
//! provides one module at its root, `decision`, which is `src/confirm/decision.rs` (the fuzz target includes the file; the replay
//! re-exports the library's).
//!
//! # The input
//!
//! The bytes are read as JSON, the result of an elicitation as it is on the wire (`{"action": ..., "content": ...}`); bytes that
//! are not JSON are `null`. The same bytes are also read as a selector byte (the action, modulo 3) followed by JSON (the content).
//!
//! # What must hold
//!
//! The oracle below restates the rule without the code under test: a result is a yes only when its action is the string `accept`
//! and its content is, as a JSON value, exactly `{"confirm": true}`; a no when the action is `decline`, or an accept whose content
//! is exactly `{"confirm": false}`; anything else is cancelled.
//!
//! | Check | Why |
//! |---|---|
//! | `decide_json` never panics, gives the same answer twice, and agrees with the oracle | only an exact accept proceeds |
//! | `decide` agrees with the oracle for every action and content, and a cancel is never anything else | the path rmcp's typed result takes |
//! | nothing proceeds unless the text of the input holds `accept`, `confirm` and `true` | no yes out of nothing |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use serde_json::{Value, json};

use super::decision::{Decision, Reply, decide, decide_json};

/// A seed corpus needs at least this many files per target (`MIN_SEEDS` in the replay test).
pub const MIN_SEEDS: usize = 8;

/// A check of one input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// The rule, written as value equality.
fn oracle(action: Option<&str>, content: Option<&Value>) -> Decision {
    match (action, content) {
        (Some("accept"), Some(c)) if *c == json!({"confirm": true}) => Decision::Proceed,
        (Some("accept"), Some(c)) if *c == json!({"confirm": false}) => Decision::Declined,
        (Some("decline"), _) => Decision::Declined,
        _ => Decision::Cancelled,
    }
}

/// `mcp_elicitation_result`.
pub fn mcp_elicitation_result(data: &[u8]) -> Result<(), String> {
    // The result as it is on the wire.
    let value: Value = serde_json::from_slice(data).unwrap_or(Value::Null);
    let got = decide_json(&value);
    if decide_json(&value) != got {
        return Err("the decision differs between two reads of the same answer".to_owned());
    }
    let want = oracle(value.get("action").and_then(Value::as_str), value.get("content"));
    if got != want {
        return Err(format!("decided {got:?} where the rule says {want:?}: {value}"));
    }
    if got == Decision::Proceed {
        let text = String::from_utf8_lossy(data);
        for needle in ["accept", "confirm", "true"] {
            if !text.contains(needle) {
                return Err(format!("proceeded on an input without `{needle}`"));
            }
        }
    }

    // The action and the content apart, as rmcp's typed result hands them over.
    let (reply, rest) = match data.split_first() {
        Some((&b, rest)) => (
            [Reply::Accept, Reply::Decline, Reply::Cancel][usize::from(b) % 3],
            rest,
        ),
        None => (Reply::Cancel, &[][..]),
    };
    let content: Option<Value> = serde_json::from_slice(rest).ok();
    let got = decide(reply, content.as_ref());
    let action = match reply {
        Reply::Accept => "accept",
        Reply::Decline => "decline",
        Reply::Cancel => "cancel",
    };
    let want = oracle(Some(action), content.as_ref());
    if got != want {
        return Err(format!(
            "decided {got:?} for {action} where the rule says {want:?}"
        ));
    }
    if reply == Reply::Cancel && got != Decision::Cancelled {
        return Err("a cancel was read as something else".to_owned());
    }
    Ok(())
}
