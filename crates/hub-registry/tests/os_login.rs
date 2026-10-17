//! T7 (Q32, A18): from the OS Login name a sentinel reports to the application user. The admin's map first; then an unambiguous match on
//! the OS Login form of an **active** user's email; an ambiguous or unknown name is the OS Login user alone, never a guess.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use hub_registry::ManualClock;
use support::rig::Rig;
use support::sentinel::{Agent, batch, by, record, user};

const T0: i64 = ManualClock::T0_MS;
const S: i64 = 1_000;

/// One unattributed change in each of `paths`, seen at `T0 + 20 s`, and a record by each of `logins` for the path at the same place,
/// delivered in one batch. Returns the attribution of each path.
async fn attributions(rig: &Rig, logins: &[&str]) -> Vec<support::sentinel::Attr> {
    rig.sentinel_for_the_swimlane().await;
    let paths: Vec<String> = (0..logins.len()).map(|i| format!("svc/p{i}.yml")).collect();
    let initial: Vec<(&str, &str)> = paths.iter().map(|p| (p.as_str(), "v: 0")).collect();
    let mut agent = Agent::start(rig, &initial).await;
    rig.clock.advance_secs(30);
    let changes: Vec<(&str, &str)> = paths.iter().map(|p| (p.as_str(), "v: 1")).collect();
    agent.change_all(&changes, T0 + 20 * S).await;
    rig.sentinel_hello().await.unwrap();
    let records = paths
        .iter()
        .zip(logins)
        .map(|(p, login)| by(record(p, T0 + 10 * S), login, "root"))
        .collect();
    rig.records(batch(1, records)).await.unwrap();
    let mut out = Vec::new();
    for p in &paths {
        out.push(rig.attribution(p, 1).await);
    }
    out
}

fn actor_login(a: &support::sentinel::Attr) -> Option<String> {
    let actor = a.actor.as_ref()?;
    (actor["kind"] == "os_login").then(|| actor["login"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn an_unambiguous_email_is_the_user_and_the_form_is_the_same_in_sql_and_in_rust() {
    let Some(rig) = Rig::new().await else { return };
    // Emails with every kind of character an OS Login name writes as `_`, and the login each one gives.
    let cases = [
        ("ada@example.com", "ada_example_com"),
        ("Ada.Lovelace+x@My-Corp.COM", "ada_lovelace_x_my_corp_com"),
        ("a-b@x.io", "a_b_x_io"),
        ("ÉLISE@x.io", "_lise_x_io"),
        ("é@y.io", "__y_io"),
    ];
    for (n, (email, _)) in cases.iter().enumerate() {
        rig.add_user(u8::try_from(n).unwrap() + 1, email, "active").await;
    }
    let logins: Vec<&str> = cases.iter().map(|(_, l)| *l).collect();
    let got = attributions(&rig, &logins).await;
    for (n, a) in got.iter().enumerate() {
        assert!(a.is("sentinel_login", "high"), "{n}: {a:?}");
        assert_eq!(
            a.user(),
            Some(user(u8::try_from(n).unwrap() + 1).to_string()),
            "{}: the login {} is the email {}",
            n,
            cases[n].1,
            cases[n].0
        );
    }
}

#[tokio::test]
async fn the_admins_map_wins_over_an_email_match_even_for_a_disabled_user() {
    let Some(rig) = Rig::new().await else { return };
    rig.add_user(1, "ada@example.com", "active").await;
    let carol = rig.add_user(3, "carol@example.com", "disabled").await;
    // The admin says that `ada_example_com` is Carol's login (a shared account, say): the map is the admin's word, and a user who has
    // since been disabled still made the change.
    rig.map_login("ada_example_com", &carol).await;
    let got = attributions(&rig, &["ada_example_com"]).await;
    assert_eq!(got[0].user(), Some(carol.to_string()));
}

#[tokio::test]
async fn a_name_only_the_admin_knows_is_mapped_and_a_name_nobody_knows_stands_alone() {
    let Some(rig) = Rig::new().await else { return };
    let bob = rig.add_user(2, "bob@example.com", "active").await;
    rig.map_login("local_account_bob", &bob).await;
    let got = attributions(&rig, &["local_account_bob", "nobody_example_com", "root"]).await;
    assert_eq!(got[0].user(), Some(bob.to_string()));
    assert_eq!(got[1].user(), None);
    assert_eq!(
        actor_login(&got[1]).as_deref(),
        Some("nobody_example_com"),
        "the OS Login user alone"
    );
    assert!(
        got[1].is("sentinel_login", "high"),
        "still a sentinel's answer: someone on the VM wrote it"
    );
    assert_eq!(actor_login(&got[2]).as_deref(), Some("root"));
}

#[tokio::test]
async fn two_users_with_one_form_are_ambiguous_and_nobody_is_named() {
    let Some(rig) = Rig::new().await else { return };
    // `a.b@x.com` and `a_b@x.com` are both `a_b_x_com`: the name cannot say which, and a guess would blame someone.
    rig.add_user(1, "a.b@x.com", "active").await;
    rig.add_user(2, "a_b@x.com", "active").await;
    rig.add_user(3, "c@x.com", "active").await;
    let got = attributions(&rig, &["a_b_x_com", "c_x_com"]).await;
    assert_eq!(got[0].user(), None, "ambiguous: not guessed at");
    assert_eq!(actor_login(&got[0]).as_deref(), Some("a_b_x_com"));
    assert_eq!(
        got[1].user(),
        Some(user(3).to_string()),
        "an unambiguous one next to it is mapped"
    );
}

#[tokio::test]
async fn a_pending_or_disabled_user_is_not_matched_by_email() {
    let Some(rig) = Rig::new().await else { return };
    rig.add_user(1, "pending@example.com", "pending").await;
    rig.add_user(2, "gone@example.com", "disabled").await;
    rig.add_user(3, "both@example.com", "disabled").await;
    rig.add_user(4, "both@example.com", "active").await; // the same address, another account, and active
    let got = attributions(
        &rig,
        &["pending_example_com", "gone_example_com", "both_example_com"],
    )
    .await;
    assert_eq!(got[0].user(), None);
    assert_eq!(got[1].user(), None);
    assert_eq!(
        got[2].user(),
        Some(user(4).to_string()),
        "only the active one has the form, so it is unambiguous; the disabled one is not a candidate"
    );
}

#[tokio::test]
async fn an_email_that_only_looks_like_the_login_is_not_a_match() {
    let Some(rig) = Rig::new().await else { return };
    rig.add_user(1, "ada@example.com", "active").await;
    // Not the same form: another domain, a prefix, a longer name.
    let got = attributions(
        &rig,
        &[
            "ada_example_org",
            "ada_example",
            "ada_example_com_x",
            "ADA_EXAMPLE_COM",
        ],
    )
    .await;
    assert!(got[..3].iter().all(|a| a.user().is_none()));
    assert_eq!(
        got[3].user(),
        Some(user(1).to_string()),
        "case is not part of the form: the OS Login name is lower case, and a record that says otherwise is the same login"
    );
}
