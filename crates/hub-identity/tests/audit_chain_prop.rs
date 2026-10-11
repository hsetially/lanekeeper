//! T5 (S9): whatever single byte of a stored chain changes, the verification finds it, at the right row. No database: the
//! per-row check is a pure function over what the table stores.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{Attribution, AttributionSource, Confidence, ContentHash, ProposalId, ShortText, Timestamp};
use hub_identity::audit::columns::{Column, ColumnSet, StoredColumns, differing_columns};
use hub_identity::audit::log::prepare;
use hub_identity::audit::verify::{FailureKind, check_row};
use ports::conformance::sample::{hash, nfs, request_id, swimlane, user_id};
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

// ---------------------------------------------------------------------------------------------------------------------------
// The columns that copy the event. Still no database: `prepare` gives what the log writes for an event, and `differing_columns`
// is what the verifier does with a stored row.

/// An event with a random subset of its optional fields, on a random kind of actor, at a random time the table can hold.
fn event_strategy() -> impl Strategy<Value = AuditEvent> {
    (
        -210_866_803_200_000_i64..8_900_000_000_000_000,
        0..4_u8,
        any::<u16>(),
        "[a-z][a-z0-9]{0,10}",
        0..6_u8,
        0..4_u8,
        any::<i64>(),
    )
        .prop_map(|(at, actor, present, word, source, confidence, approval)| {
            let mut e = AuditEvent::new(
                Timestamp::from_unix_millis(at),
                match actor {
                    0 => AuditActor::User { user: user_id(1) },
                    1 => AuditActor::Agent {
                        swimlane: swimlane("sit2"),
                    },
                    2 => AuditActor::Sentinel {
                        name: ShortText::parse(&word).unwrap(),
                    },
                    _ => AuditActor::System {
                        component: ShortText::parse(&word).unwrap(),
                    },
                },
                AuditAction::SettingsChanged,
                AuditVia::Sync,
            );
            let has = |bit: u16| present & (1 << bit) != 0;
            e.swimlane = has(0).then(|| swimlane("sit1"));
            e.path = has(1).then(|| nfs(&format!("{word}/file.yml")));
            e.hash_before = has(2).then(|| hash(word.as_bytes()));
            e.hash_after = has(3).then(|| hash(b"after"));
            e.diff = has(4).then(|| format!("-{word}\n"));
            e.github_login = has(5).then(|| ShortText::parse(&word).unwrap());
            e.approval = has(6).then(|| ProposalId::from(approval));
            e.request_id = has(7).then(|| request_id(&word));
            e.attribution = has(8).then(|| Attribution {
                source: [
                    AttributionSource::ToolWrite,
                    AttributionSource::SentinelLogin,
                    AttributionSource::SyncJob,
                    AttributionSource::NfsClient,
                    AttributionSource::FsOwnerHint,
                    AttributionSource::Unknown,
                ][usize::from(source)],
                confidence: [
                    Confidence::Low,
                    Confidence::Medium,
                    Confidence::High,
                    Confidence::Certain,
                ][usize::from(confidence)],
                actor: has(9).then(|| domain::Actor::OsLogin {
                    login: ShortText::parse(&word).unwrap(),
                }),
                evidence: domain::Evidence {
                    executable: has(10).then(|| ShortText::parse(&word).unwrap()),
                    login_identity: has(11).then(|| ShortText::parse(&word).unwrap()),
                    observed_at: has(12).then(|| Timestamp::from_unix_millis(at)),
                },
            });
            e
        })
}

/// Change one column to something the event cannot have produced.
fn change(c: Column, s: &mut StoredColumns) {
    fn text(v: &mut Option<String>) {
        *v = Some(v.take().map_or_else(|| "x".to_owned(), |t| format!("{t}x")));
    }
    fn bytes(v: &mut Option<Vec<u8>>) {
        *v = Some(
            v.take()
                .map_or_else(|| vec![9; 32], |b| b.iter().map(|x| x ^ 1).collect()),
        );
    }
    match c {
        Column::At => s.at_micros += 1,
        Column::ActorKind => s.actor_kind.push('x'),
        Column::ActorRef => text(&mut s.actor_ref),
        Column::Action => s.action.push('x'),
        Column::Via => s.via.push('x'),
        Column::SwimlaneId => text(&mut s.swimlane_id),
        Column::Path => text(&mut s.path),
        Column::HashBefore => bytes(&mut s.hash_before),
        Column::HashAfter => bytes(&mut s.hash_after),
        Column::GithubLogin => text(&mut s.github_login),
        Column::ApprovalId => s.approval_id = Some(s.approval_id.map_or(1, |n| n.wrapping_add(1))),
        Column::RequestId => text(&mut s.request_id),
        Column::AttributionSource => text(&mut s.attribution_source),
        Column::AttributionConfidence => text(&mut s.attribution_confidence),
        Column::AttributionActor => {
            s.attribution_actor = Some(serde_json::json!({"kind": "os_login", "login": "someone-else"}));
        }
        Column::AttributionEvidence => s.attribution_evidence = Some(serde_json::json!({"other": 1})),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn the_columns_of_any_written_event_never_differ_from_it(e in event_strategy()) {
        let p = prepare(&e).unwrap();
        prop_assert_eq!(differing_columns(p.json(), &p.columns().unwrap()), ColumnSet::empty());
    }

    #[test]
    fn column_verifier_property_any_single_column_change_is_detected(
        e in event_strategy(),
        column in prop::sample::select(Column::ALL.to_vec()),
    ) {
        let p = prepare(&e).unwrap();
        let mut stored = p.columns().unwrap();
        change(column, &mut stored);
        prop_assert_eq!(differing_columns(p.json(), &stored), ColumnSet::of(&[column]));
    }
}
