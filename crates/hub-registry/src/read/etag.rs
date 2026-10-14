//! Entity tags of the content reads (T6).
//!
//! The four operations the spec tags `content` (`compareFile`, `getEffectiveConfig`, `getFileContent`, `getServedFile`) carry an
//! `ETag` and honour `If-None-Match`; no other operation declares either, so the lists do not (the plan's design note had list tags
//! from `swimlane_registry.version`; the `OpenAPI` contract is narrower, and a 304 on an operation that does not declare it would be a
//! response the contract does not promise).
//!
//! A tag is a strong validator made from the content hashes of what the answer is made of and the parameters that shape it (the two
//! sides and the file of a compare, the tenant and channel of an effective config), so it changes exactly when the answer does and
//! never goes stale. It is the first 16 bytes of a SHA-256, in hex, in quotes.

use sha2::{Digest, Sha256};

/// The longest `If-None-Match` looked at: the contract's `maxLength`. A longer one is ignored, not an error (as the blob endpoint does).
pub const MAX_IF_NONE_MATCH: usize = 200;

/// A strong entity tag over `parts` (each is length-prefixed, so a byte moved between parts is another tag).
pub fn tag(kind: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(b"lanekeeper/etag/v1\n");
    h.update((kind.len() as u64).to_be_bytes());
    h.update(kind.as_bytes());
    for part in parts {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part.as_bytes());
    }
    let digest = h.finalize();
    format!("\"{}\"", hex::encode(&digest[..16]))
}

/// Whether `if_none_match` (a header value: `*`, or a comma separated list of tags, each possibly `W/`-prefixed) has `etag`. The
/// comparison is the weak one of RFC 9110 for `If-None-Match`: the `W/` prefix does not matter.
pub fn matches(if_none_match: Option<&str>, etag: &str) -> bool {
    let Some(header) = if_none_match else {
        return false;
    };
    if header.len() > MAX_IF_NONE_MATCH {
        return false;
    }
    let header = header.trim();
    if header == "*" {
        return true;
    }
    let bare = |t: &str| t.trim().trim_start_matches("W/").to_owned();
    let wanted = bare(etag);
    header.split(',').any(|candidate| bare(candidate) == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_depends_on_every_part_and_on_the_kind() {
        let a = tag("compare", &["h1", "h2", "file"]);
        assert_eq!(a, tag("compare", &["h1", "h2", "file"]));
        assert_ne!(a, tag("compare", &["h1", "h2", "other"]));
        assert_ne!(
            a,
            tag("compare", &["h2", "h1", "file"]),
            "the sides are not interchangeable"
        );
        assert_ne!(a, tag("effective", &["h1", "h2", "file"]));
        assert_ne!(
            tag("k", &["ab", "c"]),
            tag("k", &["a", "bc"]),
            "a byte moved between parts is another tag"
        );
        assert!(a.starts_with('"') && a.ends_with('"') && a.len() == 34, "{a}");
    }

    #[test]
    fn if_none_match_is_a_list_a_star_or_a_weak_tag() {
        let t = tag("x", &["a"]);
        assert!(matches(Some(&t), &t));
        assert!(matches(Some(&format!("W/{t}")), &t), "weak comparison");
        assert!(matches(Some(&format!("\"other\", {t}")), &t), "a list");
        assert!(matches(Some("*"), &t));
        assert!(!matches(Some("\"other\""), &t));
        assert!(!matches(None, &t));
        assert!(!matches(Some(""), &t));
        let long = format!("{t}{}", " ".repeat(MAX_IF_NONE_MATCH));
        assert!(
            !matches(Some(&long), &t),
            "over the contract's 200 bytes it is ignored"
        );
    }
}
