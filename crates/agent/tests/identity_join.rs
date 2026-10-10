//! Joining the hub, restoring a stored certificate, and renewing it (T2, S5).
//!
//! The metadata-server tests use a real loopback server and real time. The renewal tests run under `tokio::time::pause`
//! with a clock that follows it, so a 24-hour certificate lives and dies in milliseconds.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent::backoff::ceiling;
use agent::clock::Clock;
use agent::config::{BaseUrl, Settings};
use agent::identity::idtoken::MetadataIdTokens;
use agent::identity::joiner::{
    IdentityHandle, JOIN_BACKOFF_BASE, JOIN_BACKOFF_CAP, Joiner, RENEW_BACKOFF_BASE, RENEW_BACKOFF_CAP,
};
use agent::identity::jointoken::JoinTokenSource;
use agent::identity::store::{CertStore, MemoryCertStore};
use agent::identity::{
    CertProblem, ClientIdentity, IdTokenError, IdentityError, JoinError, JoinTokenError, KeyMaterial,
    RenewalSchedule, StoreError,
};
use bytes::Bytes;
use domain::{SwimlaneId, Timestamp};
use rcgen::{CertificateSigningRequestParams, PublicKeyData};
use support::clock::TestClock;
use support::fake_hub::{CredentialKind, FakeHub, ScriptedIdTokens, ScriptedJoinTokens};
use support::fake_metadata::FakeMetadata;
use support::test_ca::{IssueSpec, T0_MS, TestCa};
use tokio::time::{Instant, sleep};

const WI_TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.eyJhdWQiOiJodHRwczovL2h1YiJ9.d2ktc2lnbmF0dXJl";
const JOIN_TOKEN: &str = "lk_join_0123456789abcdef";
const HOUR: Duration = Duration::from_secs(3600);

fn swimlane() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

fn settings(mode: &str, join_secret: bool) -> Settings {
    let mut env = support::valid_env(Path::new("/nonexistent"));
    env.insert("LK_JOIN_MODE".into(), mode.into());
    if join_secret {
        env.insert("LK_JOIN_TOKEN_SECRET".into(), "lanekeeper-join-token".into());
    }
    Settings::from_env(&env).unwrap()
}

/// Everything a join needs, with the doubles kept so a test can look at them.
struct Rig {
    clock: Arc<TestClock>,
    hub: Arc<FakeHub>,
    store: Arc<MemoryCertStore>,
    wi: Arc<ScriptedIdTokens>,
    jt: Arc<ScriptedJoinTokens>,
    joiner: Arc<Joiner>,
}

impl Rig {
    fn new(mode: &str) -> Self {
        Self::with(mode, Ok(WI_TOKEN), Ok(JOIN_TOKEN), MemoryCertStore::new())
    }

    fn with(
        mode: &str,
        wi: Result<&str, IdTokenError>,
        jt: Result<&str, JoinTokenError>,
        store: MemoryCertStore,
    ) -> Self {
        let clock = Arc::new(TestClock::starting_at(T0_MS));
        let hub = FakeHub::new(clock.clone());
        let store = Arc::new(store);
        let wi = ScriptedIdTokens::new(wi);
        let jt = ScriptedJoinTokens::new(jt);
        let joiner = Joiner::new(
            &settings(mode, true),
            wi.clone(),
            Some(jt.clone() as Arc<dyn JoinTokenSource>),
            hub.clone(),
            store.clone(),
            clock.clone(),
        )
        .with_seed(7);
        Self {
            clock,
            hub,
            store,
            wi,
            jt,
            joiner: Arc::new(joiner),
        }
    }
}

fn now(clock: &TestClock) -> Timestamp {
    clock.now()
}

// ---------------------------------------------------------------- the credential

#[tokio::test]
async fn join_with_workload_identity_token() {
    let metadata = FakeMetadata::serving(WI_TOKEN).await;
    let clock = Arc::new(TestClock::starting_at(T0_MS));
    let hub = FakeHub::new(clock.clone());
    hub.expect_google_token(WI_TOKEN);
    let store = Arc::new(MemoryCertStore::new());
    let jt = ScriptedJoinTokens::new(Ok(JOIN_TOKEN));
    let joiner = Joiner::new(
        &settings("auto", true),
        Arc::new(MetadataIdTokens::new(
            BaseUrl::parse(metadata.url(), "http").unwrap(),
        )),
        Some(jt.clone() as Arc<dyn JoinTokenSource>),
        hub.clone(),
        store.clone(),
        clock.clone(),
    );

    let identity = joiner.join().await.unwrap();

    // The hub saw an agent join for sit1 with the Google token for the configured audience.
    let joins = hub.joins();
    assert_eq!(joins.len(), 1);
    assert_eq!(joins[0].subject, "sit1");
    assert_eq!(joins[0].kind, CredentialKind::GoogleIdToken);
    assert_eq!(joins[0].credential, WI_TOKEN);
    let asked = &metadata.requests()[0].target;
    assert!(
        asked.ends_with("audience=https%3A%2F%2Fhub.example.com"),
        "{asked}"
    );
    // The join token Secret was never read, and the certificate is ours and stored.
    assert_eq!(jt.calls(), 0);
    assert_eq!(identity.swimlane(), &swimlane());
    assert_eq!(store.saves(), 1);
    let stored = store.load().await.unwrap().unwrap();
    assert_eq!(stored.chain_der, identity.chain_der());
    assert_eq!(stored.key.pkcs8_der(), identity.key().pkcs8_der());
}

#[tokio::test]
async fn join_token_fallback_only_without_workload_identity() {
    // Auto, the metadata server is not there: the join token is used, and says so.
    let gone = FakeMetadata::absent().await;
    let clock = Arc::new(TestClock::starting_at(T0_MS));
    let hub = FakeHub::new(clock.clone());
    hub.expect_join_token(JOIN_TOKEN);
    let jt = ScriptedJoinTokens::new(Ok(JOIN_TOKEN));
    let joiner = Joiner::new(
        &settings("auto", true),
        Arc::new(MetadataIdTokens::new(BaseUrl::parse(gone.url(), "http").unwrap())),
        Some(jt.clone() as Arc<dyn JoinTokenSource>),
        hub.clone(),
        Arc::new(MemoryCertStore::new()),
        clock,
    );
    joiner.join().await.unwrap();
    assert_eq!(hub.joins()[0].kind, CredentialKind::JoinToken);
    assert_eq!(hub.joins()[0].credential, JOIN_TOKEN);
    assert_eq!(jt.calls(), 1);

    // Auto, Workload Identity works: the join token Secret is not even read.
    let rig = Rig::new("auto");
    rig.joiner.join().await.unwrap();
    assert_eq!(rig.hub.joins()[0].kind, CredentialKind::GoogleIdToken);
    assert_eq!(rig.jt.calls(), 0);

    // Auto, the metadata server answers with an error: that is a fault to fix, not a reason to use another credential.
    let rig = Rig::with(
        "auto",
        Err(IdTokenError::Refused { status: 500 }),
        Ok(JOIN_TOKEN),
        MemoryCertStore::new(),
    );
    let error = rig.joiner.join().await.unwrap_err();
    assert_eq!(
        error,
        IdentityError::IdToken(IdTokenError::Refused { status: 500 })
    );
    assert_eq!(rig.jt.calls(), 0);
    assert!(rig.hub.joins().is_empty());

    // Auto, no metadata server and no join token Secret configured: nothing to join with.
    let clock = Arc::new(TestClock::starting_at(T0_MS));
    let hub = FakeHub::new(clock.clone());
    let joiner = Joiner::new(
        &settings("auto", false),
        ScriptedIdTokens::new(Err(IdTokenError::Unavailable)),
        None,
        hub.clone(),
        Arc::new(MemoryCertStore::new()),
        clock,
    );
    assert_eq!(
        joiner.join().await.unwrap_err(),
        IdentityError::IdToken(IdTokenError::Unavailable)
    );
    assert!(hub.joins().is_empty());
}

#[tokio::test]
async fn a_hub_that_rejects_the_workload_identity_token_does_not_cause_a_fallback() {
    let rig = Rig::new("auto");
    rig.hub.expect_google_token("a-different-token");
    let error = rig.joiner.join().await.unwrap_err();
    assert_eq!(error, IdentityError::Join(JoinError::Rejected));
    assert_eq!(
        rig.hub.joins().len(),
        1,
        "one attempt, with the Workload Identity token"
    );
    assert_eq!(
        rig.jt.calls(),
        0,
        "the join token Secret is not consulted after a rejection"
    );
    assert_eq!(rig.store.saves(), 0);
}

#[tokio::test]
async fn join_mode_token_never_calls_metadata_server() {
    let metadata = FakeMetadata::serving(WI_TOKEN).await;
    let clock = Arc::new(TestClock::starting_at(T0_MS));
    let hub = FakeHub::new(clock.clone());
    let joiner = Joiner::new(
        &settings("token", true),
        Arc::new(MetadataIdTokens::new(
            BaseUrl::parse(metadata.url(), "http").unwrap(),
        )),
        Some(ScriptedJoinTokens::new(Ok(JOIN_TOKEN)) as Arc<dyn JoinTokenSource>),
        hub.clone(),
        Arc::new(MemoryCertStore::new()),
        clock,
    );
    joiner.join().await.unwrap();
    assert!(
        metadata.requests().is_empty(),
        "the metadata server must not be contacted"
    );
    assert_eq!(hub.joins()[0].kind, CredentialKind::JoinToken);
}

#[tokio::test]
async fn join_mode_workload_identity_never_reads_the_join_token() {
    let rig = Rig::with(
        "workload-identity",
        Err(IdTokenError::Unavailable),
        Ok(JOIN_TOKEN),
        MemoryCertStore::new(),
    );
    assert_eq!(
        rig.joiner.join().await.unwrap_err(),
        IdentityError::IdToken(IdTokenError::Unavailable)
    );
    assert_eq!(rig.jt.calls(), 0);
}

#[tokio::test]
async fn an_unreadable_join_token_is_reported() {
    let rig = Rig::with(
        "token",
        Ok(WI_TOKEN),
        Err(JoinTokenError::Missing),
        MemoryCertStore::new(),
    );
    assert_eq!(
        rig.joiner.join().await.unwrap_err(),
        IdentityError::JoinToken(JoinTokenError::Missing)
    );
    assert!(rig.hub.joins().is_empty());
}

// ---------------------------------------------------------------- what is stored and what is refused

#[tokio::test]
async fn a_certificate_the_agent_refuses_is_never_stored() {
    let rig = Rig::new("auto");
    rig.hub.issue_with(|_, now_ms| {
        IssueSpec::agent("sit1", now_ms).with_uris(&["spiffe://lanekeeper/agent/sit1"])
    });
    let error = rig.joiner.join().await.unwrap_err();
    assert_eq!(error, IdentityError::Certificate(CertProblem::WrongIdentity));
    assert_eq!(rig.store.saves(), 0);
}

/// A store that cannot be written, as when the chart left the Secret out or RBAC lacks `update`.
#[derive(Debug)]
struct BrokenStore(StoreError);

#[async_trait::async_trait]
impl CertStore for BrokenStore {
    async fn load(&self) -> Result<Option<agent::identity::store::StoredIdentity>, StoreError> {
        Err(self.0)
    }
    async fn save(&self, _: &ClientIdentity) -> Result<(), StoreError> {
        Err(self.0)
    }
    async fn probe(&self) -> Result<(), StoreError> {
        Err(self.0)
    }
}

#[tokio::test]
async fn a_store_that_cannot_be_written_stops_the_join_before_a_credential_is_spent() {
    let clock = Arc::new(TestClock::starting_at(T0_MS));
    let hub = FakeHub::new(clock.clone());
    let wi = ScriptedIdTokens::new(Ok(WI_TOKEN));
    let jt = ScriptedJoinTokens::new(Ok(JOIN_TOKEN));
    let joiner = Joiner::new(
        &settings("auto", true),
        wi.clone(),
        Some(jt.clone() as Arc<dyn JoinTokenSource>),
        hub.clone(),
        Arc::new(BrokenStore(StoreError::SecretMissing)),
        clock,
    );
    assert_eq!(
        joiner.join().await.unwrap_err(),
        IdentityError::Store(StoreError::SecretMissing)
    );
    assert!(wi.calls().is_empty() && jt.calls() == 0 && hub.joins().is_empty());
}

// ---------------------------------------------------------------- start-up with a stored certificate

/// A key and a chain for it from a throwaway CA, valid between the given offsets from `T0` (seconds).
fn stored(not_before: i64, not_after: i64) -> MemoryCertStore {
    let key = KeyMaterial::generate().unwrap();
    let chain = TestCa::new().issue_for_csr(
        &key.csr_der().unwrap(),
        &IssueSpec::agent("sit1", T0_MS).valid(T0_MS / 1000 + not_before, T0_MS / 1000 + not_after),
    );
    MemoryCertStore::with(key, chain)
}

#[tokio::test(start_paused = true)]
async fn expired_certificate_recovers_by_rejoin() {
    let rig = Rig::with(
        "auto",
        Ok(WI_TOKEN),
        Ok(JOIN_TOKEN),
        stored(-48 * 3600, -24 * 3600),
    );
    let identity = rig.joiner.try_establish().await.unwrap();
    assert_eq!(
        rig.hub.joins().len(),
        1,
        "an expired certificate is replaced by a join"
    );
    assert!(identity.not_after() > now(&rig.clock));
    // The store now holds the new certificate, not the expired one.
    let in_store = rig.store.load().await.unwrap().unwrap();
    assert_eq!(in_store.chain_der, identity.chain_der());
}

#[tokio::test(start_paused = true)]
async fn a_stored_certificate_that_is_still_good_is_used_without_joining() {
    // 1 hour old: waiting for the renewal point.
    let rig = Rig::with("auto", Ok(WI_TOKEN), Ok(JOIN_TOKEN), stored(-3600, 23 * 3600));
    let identity = rig.joiner.try_establish().await.unwrap();
    assert!(rig.hub.joins().is_empty());
    assert!(rig.wi.calls().is_empty() && rig.jt.calls() == 0);
    assert_eq!(rig.store.saves(), 0);
    assert_eq!(identity.swimlane(), &swimlane());

    // 15 hours old: past the renewal point and not yet in the last tenth. The stream renews it; no join is needed.
    let rig = Rig::with("auto", Ok(WI_TOKEN), Ok(JOIN_TOKEN), stored(-15 * 3600, 9 * 3600));
    rig.joiner.try_establish().await.unwrap();
    assert!(rig.hub.joins().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_stored_certificate_in_its_last_tenth_is_replaced_by_a_join() {
    // 23 hours old: 1 hour left of 24, under 10%.
    let rig = Rig::with("auto", Ok(WI_TOKEN), Ok(JOIN_TOKEN), stored(-23 * 3600, 3600));
    rig.joiner.try_establish().await.unwrap();
    assert_eq!(rig.hub.joins().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_stored_identity_that_does_not_fit_is_replaced() {
    // The certificate is for another swimlane's identity.
    let key = KeyMaterial::generate().unwrap();
    let other = TestCa::new().issue_for_csr(&key.csr_der().unwrap(), &IssueSpec::agent("sit2", T0_MS));
    let rig = Rig::with(
        "auto",
        Ok(WI_TOKEN),
        Ok(JOIN_TOKEN),
        MemoryCertStore::with(key, other),
    );
    rig.joiner.try_establish().await.unwrap();
    assert_eq!(rig.hub.joins().len(), 1);

    // The key is not the one the certificate was issued for.
    let chain = TestCa::new().issue_for_csr(
        &KeyMaterial::generate().unwrap().csr_der().unwrap(),
        &IssueSpec::agent("sit1", T0_MS),
    );
    let rig = Rig::with(
        "auto",
        Ok(WI_TOKEN),
        Ok(JOIN_TOKEN),
        MemoryCertStore::with(KeyMaterial::generate().unwrap(), chain),
    );
    rig.joiner.try_establish().await.unwrap();
    assert_eq!(rig.hub.joins().len(), 1);

    // The stored bytes are not a certificate at all.
    let rig = Rig::with(
        "auto",
        Ok(WI_TOKEN),
        Ok(JOIN_TOKEN),
        MemoryCertStore::with(
            KeyMaterial::generate().unwrap(),
            vec![Bytes::from_static(b"junk")],
        ),
    );
    rig.joiner.try_establish().await.unwrap();
    assert_eq!(rig.hub.joins().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn establish_keeps_trying_with_backoff_until_the_hub_accepts() {
    let rig = Rig::new("auto");
    rig.hub.reject_next_joins(4);
    let started = Instant::now();
    let identity = rig.joiner.establish().await;
    assert_eq!(identity.swimlane(), &swimlane());

    let joins = rig.hub.joins();
    assert_eq!(joins.len(), 5, "four refusals and then the join that worked");
    assert_eq!(joins[0].at, started, "the first attempt is immediate");
    for (k, pair) in joins.windows(2).enumerate() {
        let gap = pair[1].at - pair[0].at;
        let most = ceiling(JOIN_BACKOFF_BASE, JOIN_BACKOFF_CAP, u32::try_from(k).unwrap());
        assert!(gap <= most, "wait {k} was {gap:?}, more than {most:?}");
    }
    // Every attempt used a fresh key.
    let csrs: std::collections::HashSet<_> = joins.iter().map(|j| j.csr.clone()).collect();
    assert_eq!(csrs.len(), 5);
}

// ---------------------------------------------------------------- renewal

/// A joined identity in a running `maintain` loop.
struct Running {
    rig: Rig,
    handle: Arc<IdentityHandle>,
    t0: Instant,
}

async fn running() -> Running {
    let rig = Rig::new("auto");
    let first = rig.joiner.join().await.unwrap();
    let handle = Arc::new(IdentityHandle::new(first));
    let t0 = Instant::now();
    let (joiner, hub, handle2) = (rig.joiner.clone(), rig.hub.clone(), handle.clone());
    tokio::spawn(async move {
        let never = joiner.maintain(&handle2, hub.as_ref()).await;
        match never {}
    });
    Running { rig, handle, t0 }
}

fn spki_of_csr(csr: &Bytes) -> Vec<u8> {
    CertificateSigningRequestParams::from_der(&csr.to_vec().into())
        .unwrap()
        .public_key
        .subject_public_key_info()
}

#[tokio::test(start_paused = true)]
async fn renewal_at_half_lifetime_over_stream() {
    let Running { rig, handle, t0 } = running().await;
    let first = handle.current();
    let schedule = RenewalSchedule::new(first.not_before(), first.not_after());
    let renew_after =
        Duration::from_millis(u64::try_from(schedule.renew_at().unix_millis() - T0_MS).unwrap());
    assert!(renew_after > 11 * HOUR && renew_after < 12 * HOUR + Duration::from_secs(300));

    sleep(renew_after - Duration::from_secs(60)).await;
    assert!(
        rig.hub.renewals().is_empty(),
        "no renewal before half the lifetime has passed"
    );

    sleep(Duration::from_secs(120)).await;
    let renewals = rig.hub.renewals();
    assert_eq!(renewals.len(), 1, "exactly one renewal at half the lifetime");
    assert!(renewals[0].at - t0 >= renew_after);
    assert!(renewals[0].at - t0 < renew_after + Duration::from_secs(1));
    assert_eq!(rig.hub.joins().len(), 1, "renewal does not need a join");

    // The renewed certificate is a new one for a new key, and it is stored.
    let renewed = handle.current();
    assert_ne!(renewed.chain_der(), first.chain_der());
    assert_ne!(renewed.key().spki_der(), first.key().spki_der());
    assert_eq!(spki_of_csr(&renewals[0].csr), renewed.key().spki_der());
    assert!(renewed.not_after() > first.not_after());
    assert_eq!(rig.store.saves(), 2);
    let in_store = rig.store.load().await.unwrap().unwrap();
    assert_eq!(in_store.chain_der, renewed.chain_der());
}

#[tokio::test(start_paused = true)]
async fn renewed_cert_used_on_next_connect() {
    let Running { rig, handle, .. } = running().await;
    let mut changes = handle.subscribe();
    // A connection made before the renewal holds the identity it was made with.
    let before_renewal = handle.current();
    let old_chain = before_renewal.chain_der().to_vec();

    sleep(13 * HOUR).await;

    // The next connect asks the handle and gets the renewed certificate; the old one is untouched.
    assert!(
        changes.has_changed().unwrap(),
        "a connection can wait for a renewal"
    );
    let next_connect = handle.current();
    assert!(!Arc::ptr_eq(&before_renewal, &next_connect));
    assert_eq!(before_renewal.chain_der(), old_chain.as_slice());
    assert_ne!(next_connect.chain_der(), old_chain.as_slice());
    let in_store = rig.store.load().await.unwrap().unwrap();
    assert_eq!(in_store.chain_der, next_connect.chain_der());
    assert_eq!(
        *changes.borrow_and_update().chain_der(),
        *next_connect.chain_der()
    );
}

#[tokio::test(start_paused = true)]
async fn renewal_retry_backoff_then_rejoin_under_ten_percent() {
    let Running { rig, handle, t0 } = running().await;
    rig.hub.stream_down();
    let first = handle.current();
    let schedule = RenewalSchedule::new(first.not_before(), first.not_after());
    let since_t0 = |t: Timestamp| Duration::from_millis(u64::try_from(t.unix_millis() - T0_MS).unwrap());
    let (renew_at, rejoin_at) = (since_t0(schedule.renew_at()), since_t0(schedule.rejoin_at()));

    sleep(rejoin_at + Duration::from_secs(30)).await;

    // The agent kept asking over the stream while renewal was possible, spacing its tries by the backoff.
    let renewals = rig.hub.renewals();
    assert!(renewals.len() >= 10, "only {} attempts", renewals.len());
    assert!(renewals[0].at - t0 >= renew_at);
    for (k, pair) in renewals.windows(2).enumerate() {
        let gap = pair[1].at - pair[0].at;
        let most = ceiling(RENEW_BACKOFF_BASE, RENEW_BACKOFF_CAP, u32::try_from(k).unwrap());
        assert!(gap <= most, "wait {k} was {gap:?}, more than {most:?}");
    }
    // It stopped asking when less than a tenth was left, and joined again, which needs no working certificate.
    assert!(renewals.iter().all(|r| r.at - t0 < rejoin_at));
    let joins = rig.hub.joins();
    assert_eq!(joins.len(), 2, "the first join, then the re-join");
    let rejoined = joins[1].at - t0;
    assert!(
        rejoined >= rejoin_at && rejoined < rejoin_at + Duration::from_secs(1),
        "{rejoined:?}"
    );
    assert_eq!(joins[1].kind, CredentialKind::GoogleIdToken);

    // The re-join produced the identity the next connect uses, and from then on nothing is renewed early.
    let current = handle.current();
    assert!(current.not_after() > first.not_after());
    assert_eq!(rig.store.saves(), 2);
    let renewals_after = rig.hub.renewals().len();
    sleep(HOUR).await;
    assert_eq!(rig.hub.renewals().len(), renewals_after);
}

#[tokio::test(start_paused = true)]
async fn a_failing_rejoin_is_retried_with_backoff_until_it_works() {
    let Running { rig, handle, t0 } = running().await;
    rig.hub.stream_down();
    rig.hub.reject_next_joins(3);
    let first = handle.current();
    let schedule = RenewalSchedule::new(first.not_before(), first.not_after());
    let rejoin_at = Duration::from_millis(u64::try_from(schedule.rejoin_at().unix_millis() - T0_MS).unwrap());

    sleep(rejoin_at + HOUR).await;

    let joins = rig.hub.joins();
    assert_eq!(
        joins.len(),
        5,
        "the first join, three refusals, then the join that worked"
    );
    assert!(joins[1].at - t0 >= rejoin_at);
    for (k, pair) in joins[1..].windows(2).enumerate() {
        let gap = pair[1].at - pair[0].at;
        let most = ceiling(JOIN_BACKOFF_BASE, JOIN_BACKOFF_CAP, u32::try_from(k).unwrap());
        assert!(gap <= most, "wait {k} was {gap:?}, more than {most:?}");
    }
    assert!(
        handle.current().not_after() > first.not_after(),
        "the join that worked replaced the identity"
    );
}
