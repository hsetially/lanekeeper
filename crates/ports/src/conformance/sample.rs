//! Small builders for the values the conformance suites (and tests of other crates) need.

use bytes::Bytes;
use domain::{
    ContentHash, Guid, IdempotencyKey, NfsPath, RequestId, Role, SwimlaneId, User, UserId, UserStatus, Via,
    WriteCtx,
};

use super::auth::TokenFixtures;
use crate::{GoogleIdentity, VerifiedUser, content_hash};
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
