//! Rule 5, S14: per-user rate limits, concurrency, the in-flight byte budget and the timeouts. Time is the paused clock of tokio, so
//! nothing here sleeps.

use std::sync::Arc;
use std::time::Duration;

use domain::Role;
use hub_mcp::clock::{Clock, ManualClock, SystemClock};
use hub_mcp::limits::{Class, LimitSettings, Limits, Refusal};
use serde_json::json;

use crate::fix::*;
use crate::rig::{Rig, uid};
use crate::tools_rig::{call, signed_in};

fn rig_with(tune: impl FnOnce(&mut LimitSettings)) -> Rig {
    Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            tune(&mut s.limits);
            s
        },
    )
}

#[tokio::test(start_paused = true)]
async fn rate_limit_is_per_user() {
    let rig = rig_with(|l| {
        l.read_burst = 3;
        l.read_per_minute = 60;
    });
    rig.registry
        .inner
        .seed(&sid("sit1"), &[(np("a.yml"), bytes("a: 1\n"))]);
    let mut alice = signed_in(&rig, "alice", Role::Viewer).await;
    let mut bob = signed_in(&rig, "bob", Role::Viewer).await;
    for _ in 0..3 {
        call(&mut alice, "list_swimlanes", &json!({})).await.ok();
    }
    let fourth = call(&mut alice, "list_swimlanes", &json!({})).await;
    assert_eq!(fourth.err(), "rate_limited");
    // Bob has a budget of his own.
    for _ in 0..3 {
        call(&mut bob, "list_swimlanes", &json!({})).await.ok();
    }
    assert_eq!(
        call(&mut bob, "list_swimlanes", &json!({})).await.err(),
        "rate_limited"
    );
    // One token every second: after a second Alice may call once.
    tokio::time::advance(Duration::from_millis(1100)).await;
    call(&mut alice, "list_swimlanes", &json!({})).await.ok();
    assert_eq!(
        call(&mut alice, "list_swimlanes", &json!({})).await.err(),
        "rate_limited"
    );
    assert!(rig.state.metrics().rate_limited_calls("read") >= 3);
}

#[tokio::test(start_paused = true)]
async fn rate_limited_carries_retry_after() {
    // 6 a minute is one token every ten seconds.
    let rig = rig_with(|l| {
        l.read_burst = 1;
        l.read_per_minute = 6;
    });
    let mut alice = signed_in(&rig, "alice", Role::Viewer).await;
    call(&mut alice, "list_swimlanes", &json!({})).await.ok();
    let refused = call(&mut alice, "list_swimlanes", &json!({})).await;
    assert_eq!(refused.err(), "rate_limited");
    let error = refused.error();
    assert_eq!(error["retry_after_secs"], 10);
    assert_eq!(error["retryable"], true);
    // Waiting that long is enough.
    tokio::time::advance(Duration::from_secs(10)).await;
    call(&mut alice, "list_swimlanes", &json!({})).await.ok();
    // The wait shrinks as time passes.
    let refused = call(&mut alice, "list_swimlanes", &json!({})).await;
    assert_eq!(refused.error()["retry_after_secs"], 10);
    tokio::time::advance(Duration::from_secs(4)).await;
    let refused = call(&mut alice, "list_swimlanes", &json!({})).await;
    assert_eq!(refused.error()["retry_after_secs"], 6);
}

#[tokio::test(start_paused = true)]
async fn write_class_has_its_own_budget() {
    let rig = rig_with(|l| {
        l.read_burst = 2;
        l.write_burst = 2;
        l.write_per_minute = 6;
        l.read_per_minute = 6;
    });
    let mut editor = signed_in(&rig, "ed", Role::Editor).await;
    for _ in 0..2 {
        call(&mut editor, "list_swimlanes", &json!({})).await.ok();
    }
    assert_eq!(
        call(&mut editor, "list_swimlanes", &json!({})).await.err(),
        "rate_limited"
    );
    // Reads are used up; writes are not.
    let draft = json!({"draft_id": "d-1"});
    for _ in 0..2 {
        call(&mut editor, "apply_change", &draft).await.ok();
    }
    assert_eq!(
        call(&mut editor, "apply_change", &draft).await.err(),
        "rate_limited"
    );
    assert!(rig.state.metrics().rate_limited_calls("write") >= 1);
    // A call that is refused for its role does not spend the budget.
    let mut viewer = signed_in(&rig, "vi", Role::Viewer).await;
    for _ in 0..5 {
        assert_eq!(call(&mut viewer, "apply_change", &draft).await.err(), "forbidden");
    }
    call(&mut viewer, "list_swimlanes", &json!({})).await.ok();
}

#[tokio::test(start_paused = true)]
async fn concurrency_cap_per_user() {
    let rig = rig_with(|l| l.calls_per_user = 2);
    rig.registry
        .inner
        .seed(&sid("sit1"), &[(np("a.yml"), bytes("a: 1\n"))]);
    rig.registry.delay_every_call(Duration::from_secs(2));
    // One user, three sessions, three calls at once.
    let mut a = signed_in(&rig, "alice", Role::Viewer).await;
    let mut b = rig.client(Some("tok-alice"));
    b.initialize(&json!({})).await.unwrap();
    let mut c = rig.client(Some("tok-alice"));
    c.initialize(&json!({})).await.unwrap();
    let none = json!({});
    let (x, y, z) = tokio::join!(
        call(&mut a, "list_swimlanes", &none),
        call(&mut b, "list_swimlanes", &none),
        call(&mut c, "list_swimlanes", &none),
    );
    let outcomes: Vec<bool> = [&x, &y, &z].iter().map(|r| r.is_error()).collect();
    assert_eq!(
        outcomes.iter().filter(|e| **e).count(),
        1,
        "exactly one is turned away: {outcomes:?}"
    );
    let refused = [x, y, z].into_iter().find(Called::is_error).unwrap();
    assert_eq!(refused.err(), "rate_limited");
    assert_eq!(refused.error()["retry_after_secs"], 1);
    // The slots are free again once the calls are over.
    call(&mut a, "list_swimlanes", &json!({})).await.ok();
}

use crate::tools_rig::Called;

#[tokio::test(start_paused = true)]
async fn port_timeout_is_unavailable() {
    let rig = rig_with(|l| l.read_timeout = Duration::from_secs(5));
    rig.registry.delay_every_call(Duration::from_secs(60));
    let mut alice = signed_in(&rig, "alice", Role::Viewer).await;
    let started = tokio::time::Instant::now();
    let called = call(&mut alice, "list_swimlanes", &json!({})).await;
    assert_eq!(called.err(), "unavailable");
    assert_eq!(called.error()["retryable"], true);
    assert!(called.error()["message"].as_str().unwrap().contains("too long"));
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(5) && waited < Duration::from_secs(10),
        "{waited:?}"
    );
    // The permit was given back: the user can call again.
    rig.registry.delay_every_call(Duration::ZERO);
    call(&mut alice, "list_swimlanes", &json!({})).await.ok();
}

#[tokio::test(start_paused = true)]
async fn in_flight_bytes_are_bounded() {
    let settings = LimitSettings {
        inflight_bytes: 6 * 1024 * 1024,
        ..LimitSettings::default()
    };
    let limits = Limits::new(settings, Arc::new(ManualClock::at(SystemClock.now())));
    let wait = Duration::from_secs(1);
    assert_eq!(limits.free_bytes(), 6 * 1024 * 1024);
    let first = limits.reserve_bytes(4 * 1024 * 1024, wait).await.unwrap();
    assert_eq!(limits.free_bytes(), 2 * 1024 * 1024);
    // A second file of that size does not fit beside it, and does not wait forever.
    let started = tokio::time::Instant::now();
    assert_eq!(
        limits.reserve_bytes(4 * 1024 * 1024, wait).await.unwrap_err(),
        Refusal::Overloaded
    );
    assert!(started.elapsed() >= wait);
    assert_eq!(
        limits.free_bytes(),
        2 * 1024 * 1024,
        "a refused reservation holds nothing"
    );
    // A smaller one does.
    let small = limits.reserve_bytes(1024 * 1024, wait).await.unwrap();
    assert_eq!(limits.free_bytes(), 1024 * 1024);
    drop(first);
    assert_eq!(limits.free_bytes(), 5 * 1024 * 1024);
    // Once the first is gone the big one fits; more than the whole budget never does.
    let big = limits.reserve_bytes(4 * 1024 * 1024, wait).await.unwrap();
    assert!(limits.reserve_bytes(7 * 1024 * 1024, wait).await.is_err());
    drop((small, big));
    assert_eq!(limits.free_bytes(), 6 * 1024 * 1024);
    // The budget is the same however many ask at once: ten waiters, one fits at a time.
    let held = limits.reserve_bytes(6 * 1024 * 1024, wait).await.unwrap();
    let waiting = futures::future::join_all(
        (0..10).map(|_| limits.reserve_bytes(1024 * 1024, Duration::from_millis(10))),
    )
    .await;
    assert!(waiting.iter().all(Result::is_err));
    drop(held);
    assert_eq!(limits.free_bytes(), 6 * 1024 * 1024);
}

#[test]
fn concurrency_and_rate_in_the_limits_themselves() {
    let settings = LimitSettings {
        calls_per_user: 2,
        calls_per_replica: 3,
        read_burst: 100,
        ..LimitSettings::default()
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let limits = Limits::new(settings, Arc::new(SystemClock));
    let (alice, bob) = (uid("alice"), uid("bob"));
    let a1 = limits.begin(&alice, Class::Read).unwrap();
    let a2 = limits.begin(&alice, Class::Read).unwrap();
    assert_eq!(
        limits.begin(&alice, Class::Read).unwrap_err(),
        Refusal::RateLimited { retry_after_secs: 1 }
    );
    let b1 = limits.begin(&bob, Class::Read).unwrap();
    // The replica is full at three.
    assert_eq!(limits.begin(&bob, Class::Read).unwrap_err(), Refusal::Overloaded);
    drop(a1);
    let b2 = limits.begin(&bob, Class::Read).unwrap();
    drop((a2, b1, b2));
    assert!(limits.begin(&alice, Class::Write).is_ok());
}

#[test]
fn the_settings_are_validated() {
    assert!(LimitSettings::default().validate().is_ok());
    for tune in [
        (|l: &mut LimitSettings| l.read_per_minute = 0) as fn(&mut LimitSettings),
        |l| l.read_burst = 0,
        |l| l.calls_per_user = 0,
        |l| l.calls_per_replica = 0,
        |l| l.sessions_per_user = 0,
        |l| l.read_timeout = Duration::ZERO,
        |l| l.inflight_bytes = 1024,
        |l| l.bucket_capacity = 0,
    ] {
        let mut l = LimitSettings::default();
        tune(&mut l);
        assert!(l.validate().is_err(), "{l:?}");
    }
}

#[test]
fn a_tool_that_asks_the_user_has_a_deadline_that_covers_every_step_of_it() {
    // The deadline of the whole call is the sum of the bounds of its steps (prepare, the question, the look at the user, the write),
    // so it is never what cuts one of them short; a tool that does not ask keeps the bound of its class.
    let settings = LimitSettings::default();
    let limits = Limits::new(settings.clone(), Arc::new(SystemClock));
    assert_eq!(limits.call_deadline(Class::Read, false), settings.read_timeout);
    assert_eq!(limits.call_deadline(Class::Write, false), settings.write_timeout);
    let asks = limits.call_deadline(Class::Write, true);
    assert_eq!(
        asks,
        settings.write_timeout * 2 + settings.confirm_timeout + settings.read_timeout
    );
    assert!(asks > settings.confirm_timeout + settings.write_timeout);
    assert_eq!(asks, Duration::from_secs(185));
}
