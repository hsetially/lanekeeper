//! The size bounds of an audit batch: the largest event that can exist, the byte budget, and the helper that splits events into
//! batches that respect it and the count limit.
//!
//! `canonical_json` writes a control character in a text as six bytes (`\u0001`), and the `diff` of an event is untrusted text of
//! up to 64 KiB, so one event is up to about 399 KB of canonical JSON, not 64 KiB: a bound on the *count* of events says little
//! about the bytes under the chain lock.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::items_after_statements,
    clippy::too_many_lines
)]

use domain::{
    Actor, Attribution, AttributionSource, Confidence, Evidence, Guid, JobRef, NfsPath, ProposalId,
    RequestId, ShortText, SwimlaneId, Timestamp, UserId,
};
use ports::conformance::sample::{audit_events_totalling, worst_case_audit_event};
use ports::{
    AuditAction, AuditActor, AuditError, AuditEvent, AuditVia, MAX_AUDIT_BATCH, MAX_AUDIT_BATCH_BYTES,
    MAX_AUDIT_EVENT_BYTES, canonical_json, canonical_size, split_batches,
};
use proptest::prelude::*;

fn json_len(e: &AuditEvent) -> usize {
    canonical_json(e).unwrap().len()
}

fn bare(at: i64) -> AuditEvent {
    AuditEvent::new(
        Timestamp::from_unix_millis(at),
        AuditActor::System {
            component: ShortText::parse("test").unwrap(),
        },
        AuditAction::SettingsChanged,
        AuditVia::System,
    )
}

// ------------------------------------------------------------------------------------------------ the largest event

/// A drift guard for `MAX_AUDIT_EVENT_BYTES`. The constant is derived, by hand, from the fields of `AuditEvent` and of the types
/// inside it. This destructures every struct and matches every enum that the derivation covers, with no `..` and no wildcard, so
/// adding a field to `AuditEvent`, `Attribution` or `Evidence`, or a variant to `AuditActor` or `Actor`, stops this file from
/// compiling. When it does: add the new field's largest form to `max_event_json_bytes` in `ports/src/audit.rs` and to
/// `worst_case_audit_event`, and let the test below check the sum to the byte. (The enums `AuditAction`, `AuditVia`,
/// `AttributionSource` and `Confidence` have the same guard in `the_enum_names_the_constant_assumes_are_the_longest`.)
fn the_shape_the_constant_was_derived_from(e: &AuditEvent) {
    let AuditEvent {
        at: _,
        actor,
        action: _,
        via: _,
        swimlane: _,
        path: _,
        hash_before: _,
        hash_after: _,
        diff: _,
        github_login: _,
        approval: _,
        request_id: _,
        attribution,
    } = e;
    match actor {
        AuditActor::User { user: _ }
        | AuditActor::Agent { swimlane: _ }
        | AuditActor::Sentinel { name: _ }
        | AuditActor::System { component: _ } => {}
    }
    if let Some(Attribution {
        source: _,
        confidence: _,
        actor,
        evidence,
    }) = attribution
    {
        match actor {
            None | Some(Actor::User { user: _ } | Actor::OsLogin { login: _ } | Actor::Job { job: _ }) => {}
        }
        let Evidence {
            executable: _,
            login_identity: _,
            observed_at: _,
        } = evidence;
    }
}

#[test]
fn the_largest_event_is_what_the_constant_says() {
    let e = worst_case_audit_event(0);
    the_shape_the_constant_was_derived_from(&e);
    assert_eq!(json_len(&e), MAX_AUDIT_EVENT_BYTES, "to the byte");
    assert_eq!(canonical_size(&e), Ok(MAX_AUDIT_EVENT_BYTES));
    assert_eq!(MAX_AUDIT_EVENT_BYTES, 398_588, "the number the docs quote");
    // Every worst-case event has the same size, whatever control character its diff is made of.
    for n in 0..14 {
        assert_eq!(json_len(&worst_case_audit_event(n)), MAX_AUDIT_EVENT_BYTES);
    }
    assert_ne!(
        worst_case_audit_event(0),
        worst_case_audit_event(1),
        "they differ, so their hashes do"
    );
}

// The relations between the constants, checked when this test is built.
const _: () = {
    assert!(
        MAX_AUDIT_BATCH_BYTES >= MAX_AUDIT_EVENT_BYTES,
        "one event alone fits"
    );
    // Almost all of the largest event is its diff: 64 KiB of control characters at six bytes each.
    assert!(MAX_AUDIT_EVENT_BYTES - 6 * AuditEvent::MAX_DIFF_BYTES < 6_000);
    // Two of the largest fit, three do not: the point of a budget of bytes.
    assert!(2 * MAX_AUDIT_EVENT_BYTES <= MAX_AUDIT_BATCH_BYTES);
    assert!(3 * MAX_AUDIT_EVENT_BYTES > MAX_AUDIT_BATCH_BYTES);
    // 200 of the largest would have been 67 MiB.
    assert!(MAX_AUDIT_BATCH * MAX_AUDIT_EVENT_BYTES > 60 * 1024 * 1024);
};

#[test]
fn the_budget_is_one_mib() {
    assert_eq!(MAX_AUDIT_BATCH_BYTES, 1024 * 1024);
    assert_eq!(MAX_AUDIT_BATCH, 200);
}

/// The sizes the constant is derived from are the limits of the types. If one of them changes, this fails first and says which.
#[test]
fn the_limits_the_constant_assumes_are_the_limits_of_the_types() {
    let ok_and_over = |limit: usize, parse: &dyn Fn(&str) -> bool| {
        assert!(parse(&"a".repeat(limit)), "{limit} bytes is accepted");
        assert!(!parse(&"a".repeat(limit + 1)), "{} bytes is refused", limit + 1);
    };
    ok_and_over(256, &|s| ShortText::parse(s).is_ok());
    ok_and_over(63, &|s| SwimlaneId::parse(s).is_ok());
    ok_and_over(128, &|s| RequestId::parse(s).is_ok());
    // A path of 1,024 bytes in components of at most 255.
    let path = |sizes: &[usize]| sizes.iter().map(|n| "a".repeat(*n)).collect::<Vec<_>>().join("/");
    assert!(
        NfsPath::parse(&path(&[255, 255, 255, 254, 1])).is_ok(),
        "1,024 bytes"
    );
    assert!(
        NfsPath::parse(&path(&[255, 255, 255, 254, 2])).is_err(),
        "1,025 bytes"
    );
    assert!(NfsPath::parse(&path(&[255])).is_ok());
    assert!(NfsPath::parse(&path(&[256])).is_err(), "a component of 256 bytes");
    // A job: a name of 253 and a uid of 64.
    assert!(JobRef::new(&"a".repeat(253), &"a".repeat(64)).is_ok());
    assert!(JobRef::new(&"a".repeat(254), "u").is_err());
    assert!(JobRef::new("a", &"a".repeat(65)).is_err());
    // A user is `tid:oid`: 36 + 1 + 36.
    let guid = "00000000-0000-4000-8000-000000000001";
    assert_eq!(Guid::parse(guid).unwrap().as_str().len(), 36);
    assert_eq!(
        UserId::new(Guid::parse(guid).unwrap(), Guid::parse(guid).unwrap())
            .to_string()
            .len(),
        73
    );
    assert_eq!(AuditEvent::MAX_DIFF_BYTES, 64 * 1024);
}

/// The cost of one character in canonical JSON: the bound the constant uses is six bytes in the diff (any text) and two in a
/// text that excludes control characters, whatever the character.
#[test]
fn no_character_costs_more_than_the_constant_assumes() {
    let empty = json_len(&{
        let mut e = bare(0);
        e.diff = Some(String::new());
        e
    });
    let mut short_empty = bare(0);
    short_empty.github_login = Some(ShortText::parse("").unwrap());
    let short_empty_len = json_len(&short_empty);
    let mut worst_diff = 0;
    let mut worst_short = 0;
    // Every character below U+3000, and a sample of the rest, including the last.
    let chars = (0..0x3000_u32)
        .chain((0x3000..=0x10_FFFF).step_by(37))
        .chain([0x10_FFFF])
        .filter_map(char::from_u32);
    for c in chars {
        let text = c.to_string();
        let mut e = bare(0);
        e.diff = Some(text.clone());
        let cost = json_len(&e) - empty;
        assert!(cost <= 6 * c.len_utf8(), "{c:?} costs {cost} bytes in a diff");
        worst_diff = worst_diff.max(cost / c.len_utf8());
        if let Ok(st) = ShortText::parse(&text) {
            let mut e = bare(0);
            e.github_login = Some(st);
            let cost = json_len(&e) - short_empty_len;
            assert!(
                cost <= 2 * c.len_utf8(),
                "{c:?} costs {cost} bytes in a short text"
            );
            worst_short = worst_short.max(cost / c.len_utf8());
        }
    }
    assert_eq!(worst_diff, 6, "a control character is the worst case in a diff");
    assert_eq!(
        worst_short, 2,
        "a quote or a backslash is the worst case in a short text"
    );
}

/// The longest wire names of the enums that the constant assumes (25, 14, 14 and 7 bytes). The `match` has no wildcard, so a new
/// variant is a compile error here and has to be looked at.
#[test]
fn the_enum_names_the_constant_assumes_are_the_longest() {
    fn name<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_value(v).unwrap().as_str().unwrap().to_owned()
    }
    // A function with an exhaustive match: adding a variant stops the build until it is listed in `ALL` too.
    fn listed(a: AuditAction) -> bool {
        match a {
            AuditAction::FileEdited
            | AuditAction::FileUploaded
            | AuditAction::FileDeleted
            | AuditAction::FileReverted
            | AuditAction::FileChangeDetected
            | AuditAction::AttributionUpgraded
            | AuditAction::ServiceRestarted
            | AuditAction::ConfigServerNotified
            | AuditAction::PrRaised
            | AuditAction::ProposalCreated
            | AuditAction::ProposalApproved
            | AuditAction::ProposalRejected
            | AuditAction::ProposalExpired
            | AuditAction::DraftApplied
            | AuditAction::BaselineAdopted
            | AuditAction::DriftMarkedIntentional
            | AuditAction::DriftMarkCleared
            | AuditAction::FlaggedValueRevealed
            | AuditAction::RoleChanged
            | AuditAction::UserStatusChanged
            | AuditAction::AccessRequested
            | AuditAction::AccessApproved
            | AuditAction::AccessDenied
            | AuditAction::GithubTokenStored
            | AuditAction::GithubTokenDeleted
            | AuditAction::DocCreated
            | AuditAction::DocUpdated
            | AuditAction::DocDeleted
            | AuditAction::AgentJoined
            | AuditAction::AgentCertificateRenewed
            | AuditAction::SentinelEnrolled
            | AuditAction::SettingsChanged
            | AuditAction::AutoNotifyChanged
            | AuditAction::SessionSignedIn
            | AuditAction::SessionSignedOut
            | AuditAction::PrStateChanged => true,
        }
    }
    const ALL: [AuditAction; 36] = [
        AuditAction::FileEdited,
        AuditAction::FileUploaded,
        AuditAction::FileDeleted,
        AuditAction::FileReverted,
        AuditAction::FileChangeDetected,
        AuditAction::AttributionUpgraded,
        AuditAction::ServiceRestarted,
        AuditAction::ConfigServerNotified,
        AuditAction::PrRaised,
        AuditAction::ProposalCreated,
        AuditAction::ProposalApproved,
        AuditAction::ProposalRejected,
        AuditAction::ProposalExpired,
        AuditAction::DraftApplied,
        AuditAction::BaselineAdopted,
        AuditAction::DriftMarkedIntentional,
        AuditAction::DriftMarkCleared,
        AuditAction::FlaggedValueRevealed,
        AuditAction::RoleChanged,
        AuditAction::UserStatusChanged,
        AuditAction::AccessRequested,
        AuditAction::AccessApproved,
        AuditAction::AccessDenied,
        AuditAction::GithubTokenStored,
        AuditAction::GithubTokenDeleted,
        AuditAction::DocCreated,
        AuditAction::DocUpdated,
        AuditAction::DocDeleted,
        AuditAction::AgentJoined,
        AuditAction::AgentCertificateRenewed,
        AuditAction::SentinelEnrolled,
        AuditAction::SettingsChanged,
        AuditAction::AutoNotifyChanged,
        AuditAction::SessionSignedIn,
        AuditAction::SessionSignedOut,
        AuditAction::PrStateChanged,
    ];
    assert!(ALL.iter().copied().all(listed));
    assert_eq!(ALL.iter().map(name).map(|n| n.len()).max(), Some(25));

    fn via(v: AuditVia) -> bool {
        match v {
            AuditVia::Ui | AuditVia::Mcp | AuditVia::Sync | AuditVia::AgentDetected | AuditVia::System => {
                true
            }
        }
    }
    let vias = [
        AuditVia::Ui,
        AuditVia::Mcp,
        AuditVia::Sync,
        AuditVia::AgentDetected,
        AuditVia::System,
    ];
    assert!(vias.iter().copied().all(via));
    assert_eq!(vias.iter().map(name).map(|n| n.len()).max(), Some(14));

    fn source(s: AttributionSource) -> bool {
        match s {
            AttributionSource::ToolWrite
            | AttributionSource::SentinelLogin
            | AttributionSource::SyncJob
            | AttributionSource::NfsClient
            | AttributionSource::FsOwnerHint
            | AttributionSource::Unknown => true,
        }
    }
    let sources = [
        AttributionSource::ToolWrite,
        AttributionSource::SentinelLogin,
        AttributionSource::SyncJob,
        AttributionSource::NfsClient,
        AttributionSource::FsOwnerHint,
        AttributionSource::Unknown,
    ];
    assert!(sources.iter().copied().all(source));
    assert_eq!(sources.iter().map(name).map(|n| n.len()).max(), Some(14));

    fn confidence(c: Confidence) -> bool {
        match c {
            Confidence::Low | Confidence::Medium | Confidence::High | Confidence::Certain => true,
        }
    }
    let confidences = [
        Confidence::Low,
        Confidence::Medium,
        Confidence::High,
        Confidence::Certain,
    ];
    assert!(confidences.iter().copied().all(confidence));
    assert_eq!(confidences.iter().map(name).map(|n| n.len()).max(), Some(7));
}

/// Every variant of the two actor enums, at its largest, is no larger than the largest the constant takes.
#[test]
fn every_actor_at_its_largest_fits_under_the_constant() {
    let quotes = ShortText::parse(&"\"".repeat(256)).unwrap();
    let user = || {
        let g = Guid::parse("ffffffff-ffff-4fff-8fff-ffffffffffff").unwrap();
        UserId::new(g.clone(), g)
    };
    let swimlane = SwimlaneId::parse(&"a".repeat(63)).unwrap();
    let actors = [
        AuditActor::User { user: user() },
        AuditActor::Agent { swimlane },
        AuditActor::Sentinel { name: quotes.clone() },
        AuditActor::System {
            component: quotes.clone(),
        },
    ];
    let attributed = [
        Actor::User { user: user() },
        Actor::OsLogin { login: quotes },
        Actor::Job {
            job: JobRef::new(&"a".repeat(253), &"a".repeat(64)).unwrap(),
        },
    ];
    let largest = worst_case_audit_event(0);
    for actor in actors {
        let mut e = largest.clone();
        e.actor = actor;
        assert!(json_len(&e) <= MAX_AUDIT_EVENT_BYTES);
    }
    for actor in attributed {
        let mut e = largest.clone();
        e.attribution = Some(Attribution {
            actor: Some(actor),
            ..e.attribution.unwrap()
        });
        assert!(json_len(&e) <= MAX_AUDIT_EVENT_BYTES);
    }
    // And the same event with every optional field absent is far below it.
    assert!(json_len(&bare(0)) < 300);
}

// ---------------------------------------------------------------------------------------------- any event, by property

fn text_of(pool: Vec<char>, max_len: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(pool), 0..=max_len).prop_map(String::from_iter)
}

/// A short text of at most 256 bytes without control characters, from the characters that cost the most and some that cost less.
fn short_text() -> impl Strategy<Value = ShortText> {
    text_of(vec!['"', '\\', 'a', 'z', ' ', '\u{e9}'], 128)
        .prop_filter_map("within 256 bytes", |s| ShortText::parse(&s).ok())
}

fn arbitrary_event() -> impl Strategy<Value = AuditEvent> {
    let diff = proptest::option::of(text_of(
        vec![
            '\u{1}',
            '\u{1f}',
            '\n',
            '\t',
            '\r',
            '"',
            '\\',
            'x',
            '\u{e9}',
            '\u{1F600}',
            '\u{7f}',
        ],
        4096,
    ));
    let actor = prop_oneof![
        short_text().prop_map(|name| AuditActor::Sentinel { name }),
        short_text().prop_map(|component| AuditActor::System { component }),
    ];
    let attribution = proptest::option::of(
        (
            short_text(),
            proptest::option::of(short_text()),
            proptest::option::of(short_text()),
            any::<i64>(),
        )
            .prop_map(|(login, executable, login_identity, observed)| Attribution {
                source: AttributionSource::SentinelLogin,
                confidence: Confidence::Certain,
                actor: Some(Actor::OsLogin { login }),
                evidence: Evidence {
                    executable,
                    login_identity,
                    observed_at: Some(Timestamp::from_unix_millis(observed)),
                },
            }),
    );
    (
        any::<i64>(),
        actor,
        diff,
        proptest::option::of(short_text()),
        any::<i64>(),
        attribution,
    )
        .prop_map(|(at, actor, diff, github_login, approval, attribution)| {
            let mut e = AuditEvent::new(
                Timestamp::from_unix_millis(at),
                actor,
                AuditAction::AgentCertificateRenewed,
                AuditVia::AgentDetected,
            );
            e.diff = diff;
            e.github_login = github_login;
            e.approval = Some(ProposalId::from(approval));
            e.attribution = attribution;
            e
        })
}

proptest! {
    /// Whatever the fields hold, the canonical JSON is within what the constant says for the diff it has: the parts other than the
    /// diff never exceed their limits, and each byte of the diff costs at most six.
    #[test]
    fn no_event_exceeds_the_constant(e in arbitrary_event()) {
        let diff_bytes = e.diff.as_ref().map_or(0, String::len);
        let without_diff = MAX_AUDIT_EVENT_BYTES - (2 + 6 * AuditEvent::MAX_DIFF_BYTES);
        let bound = without_diff + if e.diff.is_some() { 2 + 6 * diff_bytes } else { 4 };
        prop_assert!(json_len(&e) <= bound, "{} > {}", json_len(&e), bound);
        prop_assert!(json_len(&e) <= MAX_AUDIT_EVENT_BYTES);
        prop_assert_eq!(canonical_size(&e), Ok(json_len(&e)));
    }
}

// ----------------------------------------------------------------------------------------------------- canonical_size

#[test]
fn canonical_size_is_the_length_of_the_canonical_json_and_refuses_what_record_refuses() {
    let mut e = bare(1);
    assert_eq!(canonical_size(&e), Ok(canonical_json(&e).unwrap().len()));
    e.diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES));
    assert!(canonical_size(&e).is_ok());
    e.diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
    assert_eq!(
        canonical_size(&e),
        Err(AuditError::TooLarge),
        "the same limit as `record`, before the event is encoded"
    );
}

// ---------------------------------------------------------------------------------------------------- split_batches

fn sizes(batches: &[Vec<AuditEvent>]) -> Vec<usize> {
    batches.iter().map(Vec::len).collect()
}

fn bytes(batch: &[AuditEvent]) -> usize {
    batch.iter().map(json_len).sum()
}

#[test]
fn nothing_gives_no_batches() {
    assert_eq!(split_batches(Vec::new()), Ok(Vec::new()));
}

#[test]
fn small_events_are_split_by_count() {
    let events: Vec<AuditEvent> = (0..450).map(bare).collect();
    let batches = split_batches(events.clone()).unwrap();
    assert_eq!(sizes(&batches), [200, 200, 50]);
    assert_eq!(batches.concat(), events, "in order, none lost or repeated");
}

#[test]
fn large_events_are_split_by_bytes() {
    // Two of the largest fit in the budget and a third does not: 797,176 and then 1,195,764 against 1,048,576.
    let events: Vec<AuditEvent> = (0..5).map(worst_case_audit_event).collect();
    let batches = split_batches(events.clone()).unwrap();
    assert_eq!(sizes(&batches), [2, 2, 1]);
    assert_eq!(batches.concat(), events);
    assert!(batches.iter().all(|b| bytes(b) <= MAX_AUDIT_BATCH_BYTES));
}

#[test]
fn a_batch_may_fill_the_budget_to_the_byte_and_not_one_byte_more() {
    let exact = audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES);
    assert_eq!(exact.iter().map(json_len).sum::<usize>(), MAX_AUDIT_BATCH_BYTES);
    assert_eq!(
        sizes(&split_batches(exact).unwrap()),
        [3],
        "exactly the budget is one batch"
    );

    let over = audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES + 1);
    assert_eq!(
        over.iter().map(json_len).sum::<usize>(),
        MAX_AUDIT_BATCH_BYTES + 1
    );
    assert_eq!(
        sizes(&split_batches(over).unwrap()),
        [2, 1],
        "one byte more is two"
    );

    // Both limits at their edge together.
    let both = audit_events_totalling(MAX_AUDIT_BATCH, MAX_AUDIT_BATCH_BYTES);
    assert_eq!(sizes(&split_batches(both).unwrap()), [MAX_AUDIT_BATCH]);
}

#[test]
fn an_event_that_cannot_be_recorded_fails_the_split_whatever_its_place() {
    for place in [0, 3, 6] {
        let mut events: Vec<AuditEvent> = (0..7).map(bare).collect();
        events[place].diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
        assert_eq!(split_batches(events), Err(AuditError::TooLarge), "place {place}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Whatever the sizes: each batch respects both limits, nothing is lost, reordered or repeated, and a batch is closed only
    /// when the next event would not fit (so there are no more batches than the limits need).
    #[test]
    fn batches_respect_both_limits_and_are_filled_in_order(
        diffs in prop::collection::vec(prop_oneof![Just(0_usize), 0..2_000_usize, 0..65_536_usize], 0..60),
    ) {
        let events: Vec<AuditEvent> = diffs
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let mut e = bare(i as i64);
                // Control characters cost six bytes each, so these events run from 300 bytes to 393 KB.
                e.diff = Some("\u{1}".repeat(*n));
                e
            })
            .collect();
        let batches = split_batches(events.clone()).unwrap();
        prop_assert_eq!(batches.concat(), events);
        for (i, b) in batches.iter().enumerate() {
            prop_assert!(!b.is_empty());
            prop_assert!(b.len() <= MAX_AUDIT_BATCH);
            prop_assert!(bytes(b) <= MAX_AUDIT_BATCH_BYTES);
            if let Some(next) = batches.get(i + 1).and_then(|n| n.first()) {
                prop_assert!(
                    b.len() == MAX_AUDIT_BATCH || bytes(b) + json_len(next) > MAX_AUDIT_BATCH_BYTES,
                    "batch {i} was closed with room for the next event"
                );
            }
        }
    }
}
