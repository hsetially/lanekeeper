//! T5 (S9): whatever single byte of a stored chain changes, the verification finds it, at the right row. No database: the
//! per-row check is a pure function over what the table stores.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{ContentHash, ShortText, Timestamp};
use hub_identity::audit::verify::{FailureKind, check_row};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, canonical_json, chain_hash, genesis_hash};
use proptest::prelude::*;

struct Row {
    prev: ContentHash,
    hash: ContentHash,
    json: Vec<u8>,
}

fn chain(texts: &[String]) -> Vec<Row> {
    let mut prev = genesis_hash();
    texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let mut e = AuditEvent::new(
                Timestamp::from_unix_millis(i64::try_from(i).unwrap()),
                AuditActor::System {
                    component: ShortText::parse("job").unwrap(),
                },
                AuditAction::SettingsChanged,
                AuditVia::System,
            );
            e.diff = Some(text.clone());
            let json = canonical_json(&e).unwrap();
            let hash = chain_hash(&prev, &json);
            let row = Row { prev, hash, json };
            prev = hash;
            row
        })
        .collect()
}

/// The first failing row, walking as the verifier does.
fn first_failure(rows: &[Row]) -> Option<(usize, FailureKind)> {
    let mut expected = genesis_hash();
    for (i, r) in rows.iter().enumerate() {
        match check_row(&expected, &r.prev, &r.hash, &r.json) {
            Ok(()) => expected = r.hash,
            Err(kind) => return Some((i, kind)),
        }
    }
    None
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn chain_verifier_property_any_single_byte_change_is_detected(
        texts in proptest::collection::vec("[ -~]{0,40}", 1..12),
        row in any::<prop::sample::Index>(),
        part in 0..3_u8,
        byte in any::<prop::sample::Index>(),
        bit in 0..8_u32,
    ) {
        let mut rows = chain(&texts);
        prop_assert_eq!(first_failure(&rows), None, "an intact chain verifies");
        let i = row.index(rows.len());
        let r = &mut rows[i];
        match part {
            0 => {
                let at = byte.index(r.json.len());
                r.json[at] ^= 1 << bit;
            }
            1 => {
                let mut b = *r.prev.as_bytes();
                b[byte.index(32)] ^= 1 << bit;
                r.prev = ContentHash::from_bytes(b);
            }
            _ => {
                let mut b = *r.hash.as_bytes();
                b[byte.index(32)] ^= 1 << bit;
                r.hash = ContentHash::from_bytes(b);
            }
        }
        // A changed event or hash fails at its own row; a changed predecessor at its own row too (the link). A changed hash can
        // also surface one row later as a broken link, but the row's own check comes first.
        let found = first_failure(&rows);
        prop_assert!(found.is_some(), "a change went unnoticed");
        prop_assert_eq!(found.unwrap().0, i);
    }

    #[test]
    fn removing_or_swapping_rows_is_detected(
        texts in proptest::collection::vec("[ -~]{0,40}", 3..12),
        at in any::<prop::sample::Index>(),
    ) {
        let rows = chain(&texts);
        let i = at.index(rows.len() - 1);
        // Remove row i: the row that followed it now follows the wrong predecessor.
        let mut cut = chain(&texts);
        cut.remove(i);
        let found = first_failure(&cut);
        prop_assert_eq!(found, Some((i, FailureKind::ChainLinkBroken)));
        // Swap two neighbours.
        let mut swapped = chain(&texts);
        swapped.swap(i, i + 1);
        prop_assert!(first_failure(&swapped).is_some());
    }
}
