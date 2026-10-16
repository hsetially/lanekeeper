//! What the user's answer to the question means (D52, S14).
//!
//! **Only an accept that carries exactly `{"confirm": true}` proceeds.** An accept with `confirm: false` is a refusal ("declined"),
//! a decline is a refusal, and everything else (a cancel, an accept without the boolean, with another key beside it, with a string
//! or a number where the boolean should be, an action nobody knows) is "cancelled": nothing is done and nothing is said to have
//! been agreed. This file depends on `serde_json` only, so that the fuzz target `mcp_elicitation_result` includes it as it is.

use serde_json::Value;

/// What the server does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The user said yes. The only decision that leads to a write.
    Proceed,
    /// The user said no (decline, or accept with `confirm: false`).
    Declined,
    /// No usable answer: a cancel, or an answer that is not a confirmation.
    Cancelled,
}

/// The `action` of an elicitation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Accept,
    Decline,
    Cancel,
}

/// Decide from the action and the `content` the client sent with it.
#[must_use]
pub fn decide(reply: Reply, content: Option<&Value>) -> Decision {
    match reply {
        Reply::Cancel => Decision::Cancelled,
        Reply::Decline => Decision::Declined,
        Reply::Accept => {
            // Exactly one key, `confirm`, and a real boolean: not a string, not a number, not a second key beside it.
            let Some(map) = content.and_then(Value::as_object).filter(|m| m.len() == 1) else {
                return Decision::Cancelled;
            };
            match map.get("confirm") {
                Some(Value::Bool(true)) => Decision::Proceed,
                Some(Value::Bool(false)) => Decision::Declined,
                _ => Decision::Cancelled,
            }
        }
    }
}

/// Decide from an elicitation result as it is on the wire: `{"action": "accept" | "decline" | "cancel", "content": {...}}`.
/// Anything that is not that is [`Decision::Cancelled`].
#[must_use]
pub fn decide_json(result: &Value) -> Decision {
    let Some(map) = result.as_object() else {
        return Decision::Cancelled;
    };
    // The names are the protocol's, lower case and exact. A later revision's new action is not a yes.
    let reply = match map.get("action").and_then(Value::as_str) {
        Some("accept") => Reply::Accept,
        Some("decline") => Reply::Decline,
        Some("cancel") => Reply::Cancel,
        _ => return Decision::Cancelled,
    };
    decide(reply, map.get("content"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_an_exact_accept_with_confirm_true_proceeds() {
        assert_eq!(
            decide(Reply::Accept, Some(&json!({"confirm": true}))),
            Decision::Proceed
        );
        assert_eq!(
            decide_json(&json!({"action": "accept", "content": {"confirm": true}})),
            Decision::Proceed
        );
    }

    #[test]
    fn accept_with_confirm_false_and_decline_are_declined() {
        assert_eq!(
            decide(Reply::Accept, Some(&json!({"confirm": false}))),
            Decision::Declined
        );
        assert_eq!(decide(Reply::Decline, None), Decision::Declined);
        // A decline that carries a yes is still a decline.
        assert_eq!(
            decide(Reply::Decline, Some(&json!({"confirm": true}))),
            Decision::Declined
        );
        assert_eq!(decide_json(&json!({"action": "decline"})), Decision::Declined);
    }

    #[test]
    fn a_cancel_is_cancelled_whatever_it_carries() {
        assert_eq!(decide(Reply::Cancel, None), Decision::Cancelled);
        assert_eq!(
            decide(Reply::Cancel, Some(&json!({"confirm": true}))),
            Decision::Cancelled
        );
        assert_eq!(
            decide_json(&json!({"action": "cancel", "content": {"confirm": true}})),
            Decision::Cancelled
        );
    }

    #[test]
    fn an_accept_that_is_not_exactly_the_boolean_is_cancelled() {
        for content in [
            None,
            Some(json!(null)),
            Some(json!({})),
            Some(json!({"confirm": "true"})),
            Some(json!({"confirm": 1})),
            Some(json!({"confirm": null})),
            Some(json!({"confirm": [true]})),
            Some(json!({"confirm": {"value": true}})),
            Some(json!({"confirm": true, "extra": 1})),
            Some(json!({"confirm": true, "confirm2": true})),
            Some(json!({"Confirm": true})),
            Some(json!({" confirm": true})),
            Some(json!([true])),
            Some(json!("confirm")),
            Some(json!(true)),
            Some(json!(1)),
        ] {
            assert_eq!(
                decide(Reply::Accept, content.as_ref()),
                Decision::Cancelled,
                "{content:?}"
            );
        }
    }

    #[test]
    fn a_wire_result_that_is_not_an_elicitation_result_is_cancelled() {
        for result in [
            json!(null),
            json!(true),
            json!("accept"),
            json!([]),
            json!({}),
            json!({"content": {"confirm": true}}),
            json!({"action": null, "content": {"confirm": true}}),
            json!({"action": "Accept", "content": {"confirm": true}}),
            json!({"action": "ACCEPT", "content": {"confirm": true}}),
            json!({"action": " accept", "content": {"confirm": true}}),
            json!({"action": "accepted", "content": {"confirm": true}}),
            json!({"action": ["accept"], "content": {"confirm": true}}),
            json!({"action": 1, "content": {"confirm": true}}),
            json!({"action": true, "content": {"confirm": true}}),
        ] {
            assert_eq!(decide_json(&result), Decision::Cancelled, "{result}");
        }
    }

    #[test]
    fn other_keys_beside_the_action_do_not_matter() {
        // `_meta` is part of the protocol's result type.
        let result = json!({"action": "accept", "content": {"confirm": true}, "_meta": {"a": 1}});
        assert_eq!(decide_json(&result), Decision::Proceed);
    }
}
