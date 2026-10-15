//! The preview token (D46): what lets an upload or a delete through.
//!
//! `WriteService::preview_effect` hands out a [`PreviewId`] and the upload or delete presents it. It has to be bound to the user,
//! the swimlane, the path, the action and the hashes of what is previewed, and it has to run out after ten minutes
//! ([`ChangePreview::TTL_SECS`]). It needs no table: the token is the issue time and an HMAC-SHA256 over everything it is bound
//! to, with a key from the secret source. The writer that checks it recomputes the HMAC from the request in front of it, so a token
//! carried to another user, path, action, expected state or content does not verify, and neither does one that was changed by a
//! single bit.
//!
//! The token is not a capability: the expected hash is still checked by the agent when the write runs, so a token that is
//! replayed inside its ten minutes can do nothing the same request could not. A refused attempt does not use the token up
//! (there is nothing to use up).
//!
//! Layout, before base64url: `version (1) | issued at, whole seconds, big endian (8) | HMAC-SHA256 (32)`. That is 41 bytes and 55
//! characters, well inside a [`PreviewId`]'s 128. The MAC is over a fixed tag and length-prefixed fields, so no two different
//! bindings give the same bytes.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aws_lc_rs::hmac;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::{ChangePreview, ContentHash, Expected, NfsPath, PreviewId, SwimlaneId, UserId};
use ports::SecretSource;
use tokio::time::Instant;

const VERSION: u8 = 1;
const TAG: &[u8] = b"lanekeeper/preview/1";
const MAC_LEN: usize = 32;
const TOKEN_LEN: usize = 1 + 8 + MAC_LEN;

/// The shortest key accepted, in bytes. A shorter secret is a configuration mistake, not something to sign with.
pub(crate) const MIN_KEY_BYTES: usize = 32;

/// A token whose issue time is further ahead than this is refused: replicas' clocks agree to well within it, and a larger lead is
/// not one of ours.
const MAX_LEAD_SECS: i64 = 30;

/// How long a fetched key is used before the secret source is asked again, so that a rotated key is picked up.
const KEY_REFRESH: Duration = Duration::from_secs(300);

/// What the token is issued for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewAction {
    Upload,
    Delete,
}

impl PreviewAction {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::Upload => b"upload",
            Self::Delete => b"delete",
        }
    }
}

/// Everything a token is bound to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Binding<'a> {
    pub user: &'a UserId,
    pub swimlane: &'a SwimlaneId,
    pub path: &'a NfsPath,
    pub action: PreviewAction,
    pub expected: &'a Expected,
    /// SHA-256 of the content as the caller sent it: for an upload, the file; for a delete, nothing.
    pub content: Option<&'a ContentHash>,
}

/// Why a token was refused. Fixed words: the token is caller input, and none of it is echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TokenError {
    #[error("the preview id is not one this service issued")]
    Malformed,
    #[error("the preview id has run out")]
    Expired,
    #[error("the preview id was issued in the future")]
    FromTheFuture,
    #[error("the preview id is for another request")]
    Mismatch,
}

fn message(b: &Binding<'_>, issued: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(256);
    let mut field = |bytes: &[u8]| {
        m.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        m.extend_from_slice(bytes);
    };
    field(TAG);
    field(&issued.to_be_bytes());
    field(b.user.tid().as_str().as_bytes());
    field(b.user.oid().as_str().as_bytes());
    field(b.swimlane.as_str().as_bytes());
    field(b.path.as_str().as_bytes());
    field(b.action.tag());
    match b.expected {
        Expected::Absent => field(b"absent"),
        Expected::Hash { hash } => {
            field(b"hash");
            field(hash.as_bytes());
        }
    }
    match b.content {
        Some(h) => {
            field(b"content");
            field(h.as_bytes());
        }
        None => field(b"none"),
    }
    m
}

fn seconds(unix_millis: i64) -> u64 {
    u64::try_from(unix_millis.div_euclid(1000)).unwrap_or(0)
}

/// Issue a token at `now_ms` (unix milliseconds). Returns it with the time it runs out.
pub(crate) fn issue(key: &hmac::Key, b: &Binding<'_>, now_ms: i64) -> Option<(PreviewId, i64)> {
    let issued = seconds(now_ms);
    let mac = hmac::sign(key, &message(b, issued));
    let mut raw = Vec::with_capacity(TOKEN_LEN);
    raw.push(VERSION);
    raw.extend_from_slice(&issued.to_be_bytes());
    raw.extend_from_slice(mac.as_ref());
    let id = PreviewId::parse(&URL_SAFE_NO_PAD.encode(&raw)).ok()?;
    let expires_ms = i64::try_from(issued)
        .unwrap_or(i64::MAX / 1000)
        .saturating_add(i64::try_from(ChangePreview::TTL_SECS).unwrap_or(600))
        .saturating_mul(1000);
    Some((id, expires_ms))
}

/// Check a token presented with a request at `now_ms`.
pub(crate) fn verify(
    key: &hmac::Key,
    b: &Binding<'_>,
    token: &PreviewId,
    now_ms: i64,
) -> Result<(), TokenError> {
    let raw = URL_SAFE_NO_PAD
        .decode(token.as_str())
        .map_err(|_| TokenError::Malformed)?;
    if raw.len() != TOKEN_LEN || raw[0] != VERSION {
        return Err(TokenError::Malformed);
    }
    let mut issued_bytes = [0_u8; 8];
    issued_bytes.copy_from_slice(&raw[1..9]);
    let issued = u64::from_be_bytes(issued_bytes);
    // The MAC first: whatever else is wrong with the token, a forged one learns nothing from the order of the checks.
    hmac::verify(key, &message(b, issued), &raw[9..]).map_err(|_| TokenError::Mismatch)?;
    let now = i64::try_from(seconds(now_ms)).unwrap_or(i64::MAX);
    let issued = i64::try_from(issued).unwrap_or(i64::MAX);
    if issued.saturating_sub(now) > MAX_LEAD_SECS {
        return Err(TokenError::FromTheFuture);
    }
    if now.saturating_sub(issued) > i64::try_from(ChangePreview::TTL_SECS).unwrap_or(600) {
        return Err(TokenError::Expired);
    }
    Ok(())
}

/// The key tokens are signed with: read from the secret source, kept for [`KEY_REFRESH`], and replaced when the source
/// answers with another. A source that fails after a key was fetched does not stop the service; one that fails before any
/// key was fetched does.
pub(crate) struct PreviewKey {
    secrets: Arc<dyn SecretSource>,
    name: &'static str,
    cached: Mutex<Option<(Instant, Arc<hmac::Key>)>>,
}

impl std::fmt::Debug for PreviewKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewKey").finish_non_exhaustive()
    }
}

/// The key could not be had. Carries nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the preview key is not available")]
pub(crate) struct KeyUnavailable;

impl PreviewKey {
    pub fn new(secrets: Arc<dyn SecretSource>, name: &'static str) -> Self {
        Self {
            secrets,
            name,
            cached: Mutex::new(None),
        }
    }

    fn current(&self) -> Option<(Instant, Arc<hmac::Key>)> {
        self.cached.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The key to sign and verify with. `timeout` bounds the call to the secret source (rule 5).
    pub async fn get(&self, timeout: Duration) -> Result<Arc<hmac::Key>, KeyUnavailable> {
        let held = self.current();
        if let Some((fetched, key)) = &held {
            if fetched.elapsed() < KEY_REFRESH {
                return Ok(Arc::clone(key));
            }
        }
        match tokio::time::timeout(timeout, self.secrets.get(self.name)).await {
            Ok(Ok(secret)) if secret.expose().len() >= MIN_KEY_BYTES => {
                let key = Arc::new(hmac::Key::new(hmac::HMAC_SHA256, secret.expose().as_bytes()));
                *self.cached.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some((Instant::now(), Arc::clone(&key)));
                Ok(key)
            }
            // A short secret or a source that does not answer: keep using the key that was fetched before, if there is one.
            _ => held.map(|(_, key)| key).ok_or(KeyUnavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use domain::Secret;
    use ports::SecretError;
    use proptest::prelude::*;

    use super::*;

    fn key() -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, b"0123456789abcdef0123456789abcdef")
    }

    fn user(n: u32) -> UserId {
        ports::conformance::sample::user_id(n)
    }

    fn swimlane() -> SwimlaneId {
        SwimlaneId::parse("sit1").unwrap()
    }

    fn path(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    const NOW: i64 = 1_791_936_000_000;

    struct Case {
        user: UserId,
        swimlane: SwimlaneId,
        path: NfsPath,
        expected: Expected,
        content: Option<ContentHash>,
    }

    impl Case {
        fn upload() -> Self {
            Self {
                user: user(1),
                swimlane: swimlane(),
                path: path("a.yml"),
                expected: Expected::Absent,
                content: Some(ContentHash::from_bytes([1; 32])),
            }
        }

        fn binding(&self, action: PreviewAction) -> Binding<'_> {
            Binding {
                user: &self.user,
                swimlane: &self.swimlane,
                path: &self.path,
                action,
                expected: &self.expected,
                content: self.content.as_ref(),
            }
        }
    }

    #[test]
    fn a_token_verifies_for_the_request_it_was_issued_for_and_fits_a_preview_id() {
        let c = Case::upload();
        let (id, expires) = issue(&key(), &c.binding(PreviewAction::Upload), NOW).unwrap();
        assert_eq!(id.as_str().len(), 55);
        assert_eq!(expires, NOW + 600_000);
        assert_eq!(
            verify(&key(), &c.binding(PreviewAction::Upload), &id, NOW + 1_000),
            Ok(())
        );
    }

    #[test]
    fn a_token_is_bound_to_each_part_of_the_request() {
        let c = Case::upload();
        let (id, _) = issue(&key(), &c.binding(PreviewAction::Upload), NOW).unwrap();
        let check = |other: &Case, action| verify(&key(), &other.binding(action), &id, NOW);

        assert_eq!(check(&c, PreviewAction::Delete), Err(TokenError::Mismatch));
        let mut other = Case::upload();
        other.user = user(2);
        assert_eq!(check(&other, PreviewAction::Upload), Err(TokenError::Mismatch));
        let mut other = Case::upload();
        other.swimlane = SwimlaneId::parse("sit2").unwrap();
        assert_eq!(check(&other, PreviewAction::Upload), Err(TokenError::Mismatch));
        let mut other = Case::upload();
        other.path = path("b.yml");
        assert_eq!(check(&other, PreviewAction::Upload), Err(TokenError::Mismatch));
        let mut other = Case::upload();
        other.expected = Expected::Hash {
            hash: ContentHash::from_bytes([9; 32]),
        };
        assert_eq!(check(&other, PreviewAction::Upload), Err(TokenError::Mismatch));
        let mut other = Case::upload();
        other.content = Some(ContentHash::from_bytes([2; 32]));
        assert_eq!(check(&other, PreviewAction::Upload), Err(TokenError::Mismatch));
        let mut other = Case::upload();
        other.content = None;
        assert_eq!(check(&other, PreviewAction::Upload), Err(TokenError::Mismatch));
    }

    #[test]
    fn a_token_runs_out_after_ten_minutes_and_not_before() {
        let c = Case::upload();
        let b = c.binding(PreviewAction::Upload);
        let (id, _) = issue(&key(), &b, NOW).unwrap();
        assert_eq!(verify(&key(), &b, &id, NOW + 600_000), Ok(()));
        assert_eq!(verify(&key(), &b, &id, NOW + 601_000), Err(TokenError::Expired));
        // Another replica whose clock is a little behind still accepts it; one that is far behind does not.
        assert_eq!(verify(&key(), &b, &id, NOW - 29_000), Ok(()));
        assert_eq!(
            verify(&key(), &b, &id, NOW - 60_000),
            Err(TokenError::FromTheFuture)
        );
    }

    #[test]
    fn a_token_made_with_another_key_is_refused() {
        let c = Case::upload();
        let b = c.binding(PreviewAction::Upload);
        let (id, _) = issue(&key(), &b, NOW).unwrap();
        let other = hmac::Key::new(hmac::HMAC_SHA256, b"another key another key another k");
        assert_eq!(verify(&other, &b, &id, NOW), Err(TokenError::Mismatch));
    }

    #[test]
    fn text_that_is_not_a_token_is_malformed() {
        let c = Case::upload();
        let b = c.binding(PreviewAction::Upload);
        for text in [
            "x",
            "not.base64..",
            "AAAA",
            &"A".repeat(128),
            "dGhpcyBpcyBub3QgYSB0b2tlbg",
        ] {
            let id = PreviewId::parse(text).unwrap();
            assert_eq!(verify(&key(), &b, &id, NOW), Err(TokenError::Malformed), "{text}");
        }
    }

    proptest! {
        /// Any single changed bit of a token makes it fail.
        #[test]
        fn any_bit_flip_fails(byte in 0..TOKEN_LEN, bit in 0u8..8) {
            let c = Case::upload();
            let b = c.binding(PreviewAction::Upload);
            let (id, _) = issue(&key(), &b, NOW).unwrap();
            let mut raw = URL_SAFE_NO_PAD.decode(id.as_str()).unwrap();
            raw[byte] ^= 1 << bit;
            let flipped = PreviewId::parse(&URL_SAFE_NO_PAD.encode(&raw)).unwrap();
            prop_assert!(verify(&key(), &b, &flipped, NOW).is_err());
        }

        /// Fields have boundaries: moving a byte from the path to the swimlane gives another MAC.
        #[test]
        fn bindings_with_the_same_bytes_cut_differently_differ(a in "[a-z]{2,8}", b in "[a-z]{1,8}") {
            let u = user(1);
            let make = |swimlane: &str, p: &str| {
                let s = SwimlaneId::parse(swimlane).unwrap();
                let p = NfsPath::parse(p).unwrap();
                let bind = Binding { user: &u, swimlane: &s, path: &p, action: PreviewAction::Delete, expected: &Expected::Absent, content: None };
                message(&bind, 1)
            };
            let joined = format!("{a}{b}");
            prop_assert_ne!(make(&a, &b), make(&joined[..1], &joined[1..]));
        }
    }

    #[derive(Debug)]
    struct Source {
        calls: AtomicUsize,
        value: Mutex<Option<String>>,
    }

    #[async_trait]
    impl SecretSource for Source {
        async fn get(&self, _name: &str) -> Result<Secret<String>, SecretError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.value
                .lock()
                .unwrap()
                .clone()
                .map(Secret::new)
                .ok_or(SecretError::Unavailable)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_key_is_cached_refreshed_and_survives_an_outage() {
        let source = Arc::new(Source {
            calls: AtomicUsize::new(0),
            value: Mutex::new(Some("k".repeat(40))),
        });
        let cache = PreviewKey::new(source.clone(), "write-preview-key");
        let t = Duration::from_secs(1);
        cache.get(t).await.unwrap();
        cache.get(t).await.unwrap();
        assert_eq!(source.calls.load(Ordering::SeqCst), 1, "cached");
        tokio::time::advance(KEY_REFRESH + Duration::from_secs(1)).await;
        *source.value.lock().unwrap() = None;
        cache.get(t).await.unwrap();
        assert_eq!(
            source.calls.load(Ordering::SeqCst),
            2,
            "asked again after the refresh time"
        );
        // The source is down but a key was fetched earlier: the service goes on with it.
    }

    #[tokio::test]
    async fn a_short_or_missing_key_is_unavailable() {
        let none = Arc::new(Source {
            calls: AtomicUsize::new(0),
            value: Mutex::new(None),
        });
        assert_eq!(
            PreviewKey::new(none, "write-preview-key")
                .get(Duration::from_secs(1))
                .await
                .err(),
            Some(KeyUnavailable)
        );
        let short = Arc::new(Source {
            calls: AtomicUsize::new(0),
            value: Mutex::new(Some("too short".into())),
        });
        assert_eq!(
            PreviewKey::new(short, "write-preview-key")
                .get(Duration::from_secs(1))
                .await
                .err(),
            Some(KeyUnavailable)
        );
    }
}

/// Hooks for the fuzz target of the preview id (S22): the token for one fixed binding, so that the target can ask what the
/// service accepts without a database. Not part of the API.
#[cfg(feature = "fuzz-hooks")]
#[doc(hidden)]
pub mod fuzzing {
    use aws_lc_rs::hmac;
    use domain::{ContentHash, Expected, Guid, NfsPath, PreviewId, SwimlaneId, UserId};

    use super::{Binding, PreviewAction, TOKEN_LEN, VERSION, issue, verify};

    fn with_binding<T>(f: impl FnOnce(&Binding<'_>) -> T) -> Option<T> {
        let user = UserId::new(
            Guid::parse("72f988bf-86f1-41af-91ab-2d7cd011db47").ok()?,
            Guid::parse("00000000-0000-4000-8000-000000000001").ok()?,
        );
        let swimlane = SwimlaneId::parse("sit1").ok()?;
        let path = NfsPath::parse("tx-api/entry.yml").ok()?;
        let content = ContentHash::from_bytes([7; 32]);
        Some(f(&Binding {
            user: &user,
            swimlane: &swimlane,
            path: &path,
            action: PreviewAction::Upload,
            expected: &Expected::Absent,
            content: Some(&content),
        }))
    }

    /// The token issued at `now_ms` for the fixed binding.
    #[must_use]
    pub fn issue_token(key: &[u8], now_ms: i64) -> Option<String> {
        let key = hmac::Key::new(hmac::HMAC_SHA256, key);
        with_binding(|b| issue(&key, b, now_ms))
            .flatten()
            .map(|(id, _)| id.as_str().to_owned())
    }

    /// Whether the service accepts `token` for the fixed binding at `now_ms`.
    #[must_use]
    pub fn accepts(key: &[u8], token: &str, now_ms: i64) -> bool {
        let key = hmac::Key::new(hmac::HMAC_SHA256, key);
        let Ok(id) = PreviewId::parse(token) else {
            return false;
        };
        with_binding(|b| verify(&key, b, &id, now_ms).is_ok()).unwrap_or(false)
    }

    /// The whole seconds a well-formed token says it was issued at, without checking anything else.
    #[must_use]
    pub fn issued_secs(token: &str) -> Option<u64> {
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .ok()?;
        if raw.len() != TOKEN_LEN || raw[0] != VERSION {
            return None;
        }
        let mut secs = [0_u8; 8];
        secs.copy_from_slice(&raw[1..9]);
        Some(u64::from_be_bytes(secs))
    }
}
