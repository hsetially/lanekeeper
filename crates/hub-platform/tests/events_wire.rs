//! T5: the outbox envelope and the event id (S11): versioned, size-capped, strict, and ids and hashes only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use domain::{
    CommitId, ContentHash, DomainEvent, GitRef, NfsPath, ObservationSource, PrLinkId, PrState, ProposalId,
    ProposalStatus, RepoKind, SwimlaneId,
};
use hub_platform::events::wire::{ENVELOPE_VERSION, EnvelopeError, EventId, MAX_EVENT_BYTES, decode, encode};
use proptest::prelude::*;

fn swimlane(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

fn sample_events() -> Vec<DomainEvent> {
    vec![
        DomainEvent::FindingsChanged {
            swimlane: swimlane("sit1"),
        },
        DomainEvent::ProposalChanged {
            id: ProposalId::from(7),
            status: ProposalStatus::Pending,
        },
        DomainEvent::DriftChanged {
            swimlane: swimlane("sit2"),
            paths: vec![NfsPath::parse("svc/app.yml").unwrap()],
        },
        DomainEvent::FileObserved {
            swimlane: swimlane("sit1"),
            path: NfsPath::parse("svc/app.yml").unwrap(),
            hash: ContentHash::from_bytes([7; 32]),
            source: ObservationSource::Scan,
        },
        DomainEvent::GitHeadMoved {
            repo: RepoKind::Base,
            branch: GitRef::parse("refs/heads/master").unwrap(),
            from: None,
            to: CommitId::parse(&"a".repeat(40)).unwrap(),
        },
        DomainEvent::PrStateChanged {
            link_id: PrLinkId::from(3),
            state: PrState::Merged,
        },
        DomainEvent::Resync,
    ]
}

#[test]
fn every_event_round_trips_through_the_envelope() {
    for event in sample_events() {
        let text = encode(&event, MAX_EVENT_BYTES).unwrap();
        assert_eq!(decode(&text, MAX_EVENT_BYTES).unwrap(), event, "{text}");
    }
}

#[test]
fn the_envelope_is_versioned_and_golden() {
    let snapshot: Vec<String> = sample_events()
        .iter()
        .map(|e| encode(e, MAX_EVENT_BYTES).unwrap())
        .collect();
    insta::assert_snapshot!("outbox_envelopes", snapshot.join("\n"));
    assert_eq!(ENVELOPE_VERSION, 1);
}

#[test]
fn an_event_over_the_cap_is_refused_on_encode_and_on_decode() {
    let paths: Vec<NfsPath> = (0..400)
        .map(|i| NfsPath::parse(&format!("service-{i}/config-file-{i}.yml")).unwrap())
        .collect();
    let big = DomainEvent::DriftChanged {
        swimlane: swimlane("sit1"),
        paths,
    };
    assert_eq!(encode(&big, MAX_EVENT_BYTES), Err(EnvelopeError::TooLarge));
    // The same event under a bigger cap encodes, and a reader with the small cap still refuses it.
    let text = encode(&big, 1 << 20).unwrap();
    assert!(text.len() > MAX_EVENT_BYTES);
    assert_eq!(decode(&text, MAX_EVENT_BYTES), Err(EnvelopeError::TooLarge));
}

#[test]
fn a_row_of_another_version_or_shape_is_not_decoded() {
    assert_eq!(
        decode(r#"{"v":2,"event":{"type":"resync"}}"#, MAX_EVENT_BYTES),
        Err(EnvelopeError::Version(2))
    );
    for bad in [
        "",
        "null",
        "[]",
        r#"{"v":1}"#,
        r#"{"event":{"type":"resync"}}"#,
        r#"{"v":1,"event":{"type":"no_such_event"}}"#,
        r#"{"v":1,"event":{"type":"resync"},"extra":true}"#,
        r#"{"v":"1","event":{"type":"resync"}}"#,
        r#"{"v":1,"event":{"type":"findings_changed","swimlane":"NOT A SWIMLANE"}}"#,
    ] {
        assert_eq!(
            decode(bad, MAX_EVENT_BYTES),
            Err(EnvelopeError::Malformed),
            "{bad:?}"
        );
    }
}

#[test]
fn event_ids_are_positive_decimal_numbers_of_bounded_length() {
    assert_eq!(EventId::parse("42").map(EventId::get), Some(42));
    assert_eq!(
        EventId::parse("9223372036854775807").map(EventId::get),
        Some(i64::MAX)
    );
    for bad in [
        "",
        "0",
        "-1",
        "+1",
        " 1",
        "1 ",
        "1.0",
        "0x10",
        "9223372036854775808",
        "１２",
        &"1".repeat(65),
    ] {
        assert_eq!(EventId::parse(bad), None, "{bad:?}");
    }
    assert_eq!(EventId::new(0), None);
    assert_eq!(EventId::new(-5), None);
    assert_eq!(EventId::new(5).unwrap().to_string(), "5");
}

proptest! {
    #[test]
    fn decode_never_panics_and_never_accepts_more_than_the_cap(text in ".{0,6000}") {
        if let Ok(event) = decode(&text, MAX_EVENT_BYTES) {
            prop_assert!(text.len() <= MAX_EVENT_BYTES);
            // What was accepted encodes again to something that decodes to the same event.
            let again = encode(&event, MAX_EVENT_BYTES).unwrap();
            prop_assert_eq!(decode(&again, MAX_EVENT_BYTES).unwrap(), event);
        }
    }

    #[test]
    fn event_id_parse_agrees_with_integer_parse(n in any::<i64>()) {
        let parsed = EventId::parse(&n.to_string());
        prop_assert_eq!(parsed.map(EventId::get), (n > 0).then_some(n));
    }
}
