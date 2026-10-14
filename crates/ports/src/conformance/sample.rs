//! Small builders for the values the conformance suites (and tests of other crates) need.

use bytes::Bytes;
use domain::{
    Actor, Attribution, AttributionSource, Confidence, ContentHash, Evidence, Guid, IdempotencyKey, NfsPath,
    ProposalId, RequestId, Role, SwimlaneId, Timestamp, User, UserId, UserStatus, Via, WriteCtx,
};

use super::auth::TokenFixtures;
use crate::{
    AuditAction, AuditActor, AuditEvent, AuditVia, GoogleIdentity, VerifiedUser, canonical_size, content_hash,
};
use domain::ShortText;

pub fn swimlane(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

pub fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

pub fn request_id(s: &str) -> RequestId {
    RequestId::parse(s).unwrap()
}

pub fn hash(bytes: &[u8]) -> ContentHash {
    content_hash(bytes)
}

pub fn user_id(n: u32) -> UserId {
    let tid = Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").unwrap();
    let oid = Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap();
    UserId::new(tid, oid)
}

/// User number `n`, with distinct id and email.
pub fn user(n: u32, role: Option<Role>, status: UserStatus) -> User {
    User {
        id: user_id(n),
        email: ShortText::parse(&format!("user{n}@example.com")).unwrap(),
        display_name: ShortText::parse(&format!("User {n}")).unwrap(),
        role,
        status,
        requires_approval: false,
        github_login: None,
        last_seen_at: None,
    }
}

pub fn active(n: u32, role: Role) -> User {
    user(n, Some(role), UserStatus::Active)
}

pub fn write_ctx(user: &User, key: &str) -> WriteCtx {
    WriteCtx {
        user: user.clone(),
        via: Via::Ui,
        idempotency_key: IdempotencyKey::parse(key).unwrap(),
        request_id: RequestId::parse(&format!("req-{key}")).unwrap(),
    }
}

pub fn files(items: &[(&str, &str)]) -> Vec<(NfsPath, Bytes)> {
    items
        .iter()
        .map(|(p, c)| (nfs(p), Bytes::copy_from_slice(c.as_bytes())))
        .collect()
}

/// One token of each kind for `conformance::token_verifier`.
pub fn token_fixtures() -> TokenFixtures {
    TokenFixtures {
        entra_ok: (
            "entra-good-token".to_owned(),
            VerifiedUser {
                id: user_id(900),
                email: Some(ShortText::parse("ada@example.com").unwrap()),
                display_name: Some(ShortText::parse("Ada").unwrap()),
            },
        ),
        entra_expired: "entra-expired-token".to_owned(),
        google_ok: (
            "google-good-token".to_owned(),
            "https://hub.example.com".to_owned(),
            GoogleIdentity {
                subject: ShortText::parse("1234567890").unwrap(),
                email: ShortText::parse("agent@proj.iam.gserviceaccount.com").unwrap(),
                audience: ShortText::parse("https://hub.example.com").unwrap(),
            },
        ),
    }
}

/// The largest audit event that can exist: every optional field present, every text at its limit and made of the characters
/// that cost the most in canonical JSON. Its canonical JSON is exactly [`crate::MAX_AUDIT_EVENT_BYTES`] bytes.
///
/// `n` picks which control character the diff is made of (all of them cost six bytes), so that events with different `n` differ,
/// and have different hashes, at the same size.
pub fn worst_case_audit_event(n: u32) -> AuditEvent {
    // Control characters that canonical JSON writes as `\u00XX`: all but backspace, tab, line feed, form feed and carriage return.
    const SIX_BYTE: [char; 7] = ['\u{1}', '\u{2}', '\u{3}', '\u{4}', '\u{5}', '\u{6}', '\u{7}'];
    let control = SIX_BYTE[(n as usize) % SIX_BYTE.len()];
    let quotes = |len: usize| ShortText::parse(&"\"".repeat(len)).unwrap();
    let backslashes = |len: usize| ShortText::parse(&"\\".repeat(len)).unwrap();
    // 1,024 bytes in five components of at most 255 bytes: four separators and 1,020 quotes.
    let path = [255, 255, 255, 254, 1].map(|len| "\"".repeat(len)).join("/");
    assert_eq!(path.len(), 1024);
    let mut e = AuditEvent::new(
        Timestamp::from_unix_millis(i64::MIN),
        AuditActor::System {
            component: quotes(256),
        },
        // The longest wire names of `AuditAction` and `AuditVia`.
        AuditAction::AgentCertificateRenewed,
        AuditVia::AgentDetected,
    );
    e.swimlane = Some(swimlane(&"a".repeat(63)));
    e.path = Some(NfsPath::parse(&path).unwrap());
    e.hash_before = Some(hash(b"before"));
    e.hash_after = Some(hash(b"after"));
    e.diff = Some(control.to_string().repeat(AuditEvent::MAX_DIFF_BYTES));
    e.github_login = Some(backslashes(256));
    e.approval = Some(ProposalId::from(i64::MIN));
    e.request_id = Some(RequestId::parse(&"r".repeat(128)).unwrap());
    e.attribution = Some(Attribution {
        source: AttributionSource::SentinelLogin,
        confidence: Confidence::Certain,
        actor: Some(Actor::OsLogin { login: quotes(256) }),
        evidence: Evidence {
            executable: Some(quotes(256)),
            login_identity: Some(backslashes(256)),
            observed_at: Some(Timestamp::from_unix_millis(i64::MIN)),
        },
    });
    e
}

/// [`worst_case_audit_event`] with a time that a store can hold: the real log's `timestamptz` column holds the years 4713 BC to
/// 294276 AD, and refuses `i64::MIN` milliseconds. The earliest time it holds is 4 characters shorter than `i64::MIN`, so
/// this event is `MAX_AUDIT_EVENT_BYTES - 4` bytes: the largest event that `record` accepts anywhere.
pub fn storable_worst_case_audit_event(n: u32) -> AuditEvent {
    let mut e = worst_case_audit_event(n);
    e.at = Timestamp::from_unix_millis(-210_866_803_200_000);
    e
}

/// `n` audit events whose canonical JSON adds up to exactly `total` bytes, each of a size within one byte of the others, with
/// diffs of control characters (six bytes each) and a few plain characters to make up the last bytes. For tests of the byte
/// budget at its edge. Panics if the events cannot be made: an event is at least about 300 bytes and at most
/// [`crate::MAX_AUDIT_EVENT_BYTES`].
pub fn audit_events_totalling(n: usize, total: usize) -> Vec<AuditEvent> {
    let make = |i: usize, size: usize| {
        let mut e = AuditEvent::new(
            Timestamp::from_unix_millis(1_700_000_000_000 + i64::try_from(i).unwrap()),
            AuditActor::User { user: user_id(1) },
            AuditAction::FileChangeDetected,
            AuditVia::AgentDetected,
        );
        e.swimlane = Some(swimlane("sit1"));
        e.path = Some(nfs("a/b.yml"));
        e.diff = Some(String::new());
        let base = canonical_size(&e).unwrap();
        let extra = size
            .checked_sub(base)
            .expect("the size is below the smallest event");
        let (control, plain) = (extra / 6, extra % 6);
        assert!(
            control + plain <= AuditEvent::MAX_DIFF_BYTES,
            "the size is above what one event holds"
        );
        // Another control character per event, so that the events differ.
        let c = ['\u{1}', '\u{2}', '\u{3}', '\u{4}', '\u{5}', '\u{6}', '\u{7}'][i % 7];
        e.diff = Some(format!("{}{}", c.to_string().repeat(control), "x".repeat(plain)));
        assert_eq!(canonical_size(&e).unwrap(), size);
        e
    };
    (0..n)
        .map(|i| make(i, total / n + usize::from(i < total % n)))
        .collect()
}
