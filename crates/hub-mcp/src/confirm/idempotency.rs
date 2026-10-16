//! The idempotency keys of the write tools (D66, D68).
//!
//! `apply_change` uses a key derived from the draft id alone, so that a retry of applying the same draft never applies it twice.
//! Every other write tool derives its key from the user id, the MCP session id, the JSON-RPC request id and the tool name, so that
//! a retry of the same request is the same key and no other request is. Each part is length-prefixed before it is hashed, so
//! `("ab", "c")` and `("a", "bc")` are two keys. A key is `mcp-` or `mcp-apply-` and the 64 hex digits of a SHA-256: 68 or 74
//! characters of `[a-z0-9-]`, a valid [`IdempotencyKey`].

use aws_lc_rs::digest::{SHA256, digest};
use domain::{DraftId, IdempotencyKey, UserId};

use crate::output::hex;

/// The key for applying `draft`.
#[must_use]
pub fn apply_key(draft: &DraftId) -> Option<IdempotencyKey> {
    let hash = digest(&SHA256, draft.as_str().as_bytes());
    IdempotencyKey::parse(&format!("mcp-apply-{}", hex(hash.as_ref()))).ok()
}

/// The key for a request of a tool other than `apply_change`.
#[must_use]
pub fn tool_key(user: &UserId, session: &str, rpc_id: &str, tool: &str) -> Option<IdempotencyKey> {
    let user = user.to_string();
    let mut data = Vec::with_capacity(32 + user.len() + session.len() + rpc_id.len() + tool.len());
    for part in [user.as_str(), session, rpc_id, tool] {
        // A length in front of each part, so that where one ends and the next begins is part of what is hashed.
        data.extend_from_slice(&u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        data.extend_from_slice(part.as_bytes());
    }
    let hash = digest(&SHA256, &data);
    IdempotencyKey::parse(&format!("mcp-{}", hex(hash.as_ref()))).ok()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use sha2::{Digest, Sha256};

    use super::*;

    fn user(oid: &str) -> UserId {
        format!("11111111-2222-4333-8444-555555555555:{oid}")
            .parse()
            .unwrap()
    }

    const OID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

    #[test]
    fn the_apply_key_is_the_hash_of_the_draft_id() {
        let draft = DraftId::parse("draft-1").unwrap();
        let expected = format!("mcp-apply-{}", hex::encode(Sha256::digest(b"draft-1")));
        assert_eq!(apply_key(&draft).unwrap().as_str(), expected);
        assert_eq!(expected.len(), 74);
    }

    #[test]
    fn the_apply_key_does_not_depend_on_who_or_where() {
        let draft = DraftId::parse("draft-1").unwrap();
        assert_eq!(apply_key(&draft), apply_key(&DraftId::parse("draft-1").unwrap()));
        assert_ne!(apply_key(&draft), apply_key(&DraftId::parse("draft-2").unwrap()));
    }

    #[test]
    fn the_tool_key_is_a_valid_key_of_68_characters() {
        let key = tool_key(&user(OID), "session-a", "7", "restart_service").unwrap();
        assert!(key.as_str().starts_with("mcp-"));
        assert!(!key.as_str().starts_with("mcp-apply-"));
        assert_eq!(key.as_str().len(), 68);
        assert!(IdempotencyKey::parse(key.as_str()).is_ok());
    }

    #[test]
    fn parts_cannot_slide_into_each_other() {
        let u = user(OID);
        assert_ne!(tool_key(&u, "ab", "c", "t"), tool_key(&u, "a", "bc", "t"));
        assert_ne!(tool_key(&u, "a", "bc", "t"), tool_key(&u, "a", "b", "ct"));
        assert_ne!(tool_key(&u, "", "ab", "t"), tool_key(&u, "ab", "", "t"));
    }

    proptest! {
        #[test]
        fn other_write_tools_key_depends_on_user_session_request_and_tool_prop(
            a in (any::<u8>(), "[ -~]{0,40}", "[ -~]{0,20}", "[a-z_]{1,20}"),
            b in (any::<u8>(), "[ -~]{0,40}", "[ -~]{0,20}", "[a-z_]{1,20}"),
        ) {
            let make = |(n, session, rpc, tool): &(u8, String, String, String)| {
                tool_key(&user(&format!("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee{n:02x}")), session, rpc, tool)
            };
            let (ka, kb) = (make(&a), make(&b));
            prop_assert!(ka.is_some() && kb.is_some());
            // Every key is valid, and two requests that differ in any of the four parts have two keys.
            prop_assert_eq!(ka == kb, a == b);
            for key in [ka, kb].into_iter().flatten() {
                prop_assert!(IdempotencyKey::parse(key.as_str()).is_ok());
                prop_assert!(key.as_str().len() <= 128);
            }
        }
    }
}
