//! Severity (T9, D76, D103, rule 10): the rules in `severity_rules` rate every pending change, state row and finding of the registry, a change
//! of the rules rates them again, the notifier's priority and the order of the drift list follow them, and what the rules say is data.
//!
//! Everything runs through the real ingestion and the real pipeline over a real Postgres 16, as the hub would: the rules are rows that an
//! admin's write would have made (nobody writes them yet, plan A7, so the tests insert them), and what the tests look at is what the pipeline
//! wrote and what the read service and the notifier then said.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::time::Duration;

use bytes::Bytes;
use domain::{ContentHash, DriftState, FileClass, NfsPath, Page, Severity};
use engine::drift_hash;
use hub_registry::{RegistryApi as _, RegistryConfig};
use ports::NotificationKind;
use support::reads::{Reads, lane};
use support::sync::SyncRig;
use support::world::{World, state_of, wait_until};

/// A file the base repository has and an out-of-band edit changes. Medium by default (D76, "any other out-of-band change").
const CORE: &str = "app/core.yml";
/// Under `security/**`: critical by default.
const SECURITY: &str = "security/keys.yml";
/// An ordinary file in a folder an admin's rule is about.
const PAYMENTS: &str = "payments/rates.yml";
/// Never edited: in sync, so it has no severity.
const CALM: &str = "app/calm.yml";

const GIT: [(&str, &str); 4] = [
    (CORE, "a: 1\n"),
    (SECURITY, "k: 1\n"),
    (PAYMENTS, "r: 1\n"),
    (CALM, "c: 1\n"),
];

fn drift(text: &str) -> ContentHash {
    drift_hash(Some(&Bytes::from(text.to_owned())), FileClass::Structured).unwrap()
}

/// An adopted swimlane `sit1` in sync with Git, then three files edited on NFS out of band that the pipeline has seen: ahead of Git.
async fn drifted(w: &mut World) {
    let base: Vec<(String, Option<&str>)> = GIT
        .iter()
        .map(|(p, c)| (format!("config/{p}"), Some(*c)))
        .collect();
    let base: Vec<(&str, Option<&str>)> = base.iter().map(|(p, c)| (p.as_str(), *c)).collect();
    w.base_commit(&base);
    w.tenant_commit("sit1", &[("data/config/app/x.yml", Some("x: 1\n"))]);
    let mut nfs: Vec<(&str, &str)> = GIT.to_vec();
    nfs.push(("app/x-sit1.yml", "x: 1\n"));
    w.inventory(&nfs).await;
    w.adopt().await;
    for (path, content) in GIT {
        w.baseline(path, drift(content)).await;
    }
    w.baseline("app/x-sit1.yml", drift("x: 1\n")).await;
    w.whole().await;
    w.nfs_change(&[(CORE, "a: 99\n"), (SECURITY, "k: 99\n"), (PAYMENTS, "r: 99\n")])
        .await;
    w.paths(&[CORE, SECURITY, PAYMENTS]).await;
    for p in [CORE, SECURITY, PAYMENTS] {
        assert_eq!(state_of(w, p).await.as_deref(), Some("nfs_ahead"), "{p}");
    }
}

fn nfs_root() -> NfsPath {
    NfsPath::root()
}

/// These paths in the order their rows were made in (`file_state.seq`), the order the drift list keeps among equals.
async fn by_seq(w: &World, paths: &[&str]) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT path FROM file_state WHERE swimlane_id = 'sit1' AND path = ANY($1::text[]) ORDER BY seq LIMIT 100",
    )
    .bind(paths)
    .fetch_all(&w.rig.pool)
    .await
    .unwrap()
}

/// An admin's rule, as a write of the admin API would store it.
async fn rule(w: &World, id: &str, severity: &str, matcher: &str) {
    sqlx::query(
        "INSERT INTO severity_rules (id, description, scope, matcher, severity, position) \
         VALUES ($1, 'a rule of the tests', 'change', $2::jsonb, $3, 100) \
         ON CONFLICT (id) DO UPDATE SET matcher = EXCLUDED.matcher, severity = EXCLUDED.severity, enabled = true",
    )
    .bind(id)
    .bind(matcher)
    .bind(severity)
    .execute(&w.rig.pool)
    .await
    .unwrap();
}

/// What the three tables say about one path: the state row, the open drift finding, and the observation the state row points at.
#[derive(Debug, PartialEq, Eq)]
struct Rated {
    row: Option<String>,
    finding: Option<String>,
    rules: Vec<String>,
    observation: Option<String>,
}

async fn rated(w: &World, path: &str) -> Rated {
    let pool = &w.rig.pool;
    let row: Option<String> =
        sqlx::query_scalar("SELECT severity FROM file_state WHERE swimlane_id = 'sit1' AND path = $1")
            .bind(path)
            .fetch_one(pool)
            .await
            .unwrap();
    let finding: Option<(String, Vec<String>)> = sqlx::query_as(
        "SELECT severity, rule_ids FROM findings \
         WHERE swimlane_id = 'sit1' AND kind = 'drift' AND path = $1 AND resolved_at IS NULL",
    )
    .bind(path)
    .fetch_optional(pool)
    .await
    .unwrap();
    let observation: Option<String> = sqlx::query_scalar(
        "SELECT o.severity FROM file_observations o JOIN file_state f ON f.last_observation_id = o.id \
         WHERE f.swimlane_id = 'sit1' AND f.path = $1",
    )
    .bind(path)
    .fetch_one(pool)
    .await
    .unwrap();
    Rated {
        row,
        finding: finding.as_ref().map(|f| f.0.clone()),
        rules: finding.map(|f| f.1).unwrap_or_default(),
        observation,
    }
}

fn all(severity: &str) -> Rated {
    Rated {
        row: Some(severity.to_owned()),
        finding: Some(severity.to_owned()),
        rules: Vec::new(),
        observation: Some(severity.to_owned()),
    }
}

/// Everything but the ids of the rules that matched, which a test names where it cares.
fn without_rules(mut r: Rated) -> Rated {
    r.rules.clear();
    r
}

// ---------------------------------------------------------------------------------------------------------------- the three places

#[tokio::test]
async fn the_default_rules_rate_the_state_row_the_finding_and_the_observation_of_a_change() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    drifted(&mut r.world).await;
    let w = &r.world;

    assert_eq!(
        without_rules(rated(w, CORE).await),
        all("medium"),
        "an out-of-band change"
    );
    assert_eq!(
        without_rules(rated(w, SECURITY).await),
        all("critical"),
        "any change under security/** (D76)"
    );
    assert_eq!(without_rules(rated(w, PAYMENTS).await), all("medium"));
    assert!(
        rated(w, SECURITY)
            .await
            .rules
            .contains(&"default.security_path".to_owned()),
        "the finding names the rule that rated it"
    );

    // A file nobody changed is not a change: it has no severity, in the row or in what it was observed as.
    assert_eq!(
        rated(w, CALM).await,
        Rated {
            row: None,
            finding: None,
            rules: Vec::new(),
            observation: None
        }
    );

    // The tree says so too: a folder is as serious as the worst thing in it.
    let tree = {
        r.service
            .tree_filtered(
                &Reads::viewer(),
                &lane("sit1"),
                &nfs_root(),
                None,
                Page::new(100, None).unwrap(),
            )
            .await
            .unwrap()
    };
    let by = |n: &str| tree.items.iter().find(|e| e.path.as_str() == n).unwrap().severity;
    assert_eq!(by("security"), Some(Severity::Critical));
    assert_eq!(by("payments"), Some(Severity::Medium));
    assert_eq!(
        by("app"),
        Some(Severity::Medium),
        "app/ holds core.yml, ahead of Git"
    );
}

#[tokio::test]
async fn a_rule_from_the_table_rates_the_row_the_finding_and_the_observation() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    drifted(&mut r.world).await;
    let w = &r.world;
    rule(
        w,
        "payments-are-critical",
        "critical",
        r#"{"path_glob": "payments/**"}"#,
    )
    .await;

    // The pipeline has not been told anything about payments/, only that another path changed: the rules are an input of the whole swimlane.
    w.paths(&[CORE]).await;

    let p = rated(w, PAYMENTS).await;
    assert_eq!(
        (p.row.as_deref(), p.finding.as_deref(), p.observation.as_deref()),
        (Some("critical"), Some("critical"), Some("critical")),
        "{p:?}"
    );
    assert!(p.rules.contains(&"payments-are-critical".to_owned()), "{p:?}");
    assert!(
        p.rules.contains(&"default.out_of_band".to_owned()),
        "the rules that did not decide are named too: {p:?}"
    );
    assert_eq!(
        without_rules(rated(w, CORE).await),
        all("medium"),
        "a path the rule does not match is as it was"
    );
}

#[tokio::test]
async fn a_change_of_the_rules_rates_the_swimlane_again_and_every_way_back_too() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    drifted(&mut r.world).await;
    let w = &r.world;
    let fingerprint = || async {
        sqlx::query_scalar::<_, Option<Vec<u8>>>(
            "SELECT rules_fingerprint FROM swimlane_registry WHERE swimlane_id = 'sit1'",
        )
        .fetch_one(&w.rig.pool)
        .await
        .unwrap()
    };
    let before = fingerprint().await;
    assert!(before.is_some());

    // A new rule.
    rule(
        w,
        "payments-are-critical",
        "critical",
        r#"{"path_glob": "payments/**"}"#,
    )
    .await;
    let report = w.paths(&[CORE]).await;
    assert_eq!(report.lanes.len(), 1, "{report:?}");
    assert_eq!(
        report.lanes[0].result,
        hub_registry::metrics::LaneResult::Whole,
        "an incremental batch whose rules are not the ones the swimlane was computed under is a whole one"
    );
    let with_rule = fingerprint().await;
    assert_ne!(
        with_rule, before,
        "the severity rules are part of the fingerprint"
    );
    assert_eq!(rated(w, PAYMENTS).await.row.as_deref(), Some("critical"));

    // The same rule, switched off.
    sqlx::query("UPDATE severity_rules SET enabled = false WHERE id = 'payments-are-critical'")
        .execute(&w.rig.pool)
        .await
        .unwrap();
    w.paths(&[CORE]).await;
    assert_ne!(
        fingerprint().await,
        with_rule,
        "a rule switched off is another set"
    );
    assert_eq!(
        rated(w, PAYMENTS).await.row.as_deref(),
        Some("medium"),
        "back to the default"
    );
    assert_eq!(rated(w, PAYMENTS).await.finding.as_deref(), Some("medium"));
    assert_eq!(
        rated(w, PAYMENTS).await.observation.as_deref(),
        Some("medium"),
        "the observation of a change that is still pending is rated again"
    );

    // The rule gone altogether is the set it was before the rule: the same fingerprint.
    sqlx::query("DELETE FROM severity_rules")
        .execute(&w.rig.pool)
        .await
        .unwrap();
    w.paths(&[CORE]).await;
    assert_eq!(
        fingerprint().await,
        before,
        "no rule, or no enabled rule: the defaults, as before"
    );

    // A rule that changes nothing a rate depends on (its description, its place in the list) is not another set.
    rule(
        w,
        "payments-are-critical",
        "critical",
        r#"{"path_glob": "payments/**"}"#,
    )
    .await;
    w.paths(&[CORE]).await;
    let again = fingerprint().await;
    assert_eq!(again, with_rule, "the same rule is the same set");
    sqlx::query("UPDATE severity_rules SET description = 'another words', position = 7")
        .execute(&w.rig.pool)
        .await
        .unwrap();
    let report = w.paths(&[CORE]).await;
    assert_eq!(fingerprint().await, again);
    assert_eq!(
        report.lanes[0].result,
        hub_registry::metrics::LaneResult::Incremental,
        "words and order are no input: nothing was recomputed"
    );
}

#[tokio::test]
async fn a_change_that_is_resolved_keeps_the_severity_it_was_rated_when_it_was_pending() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    drifted(&mut r.world).await;
    let w = &r.world;
    assert_eq!(rated(w, CORE).await.observation.as_deref(), Some("medium"));

    // Git takes the edit (a push that equals what is on NFS): nothing differs any more, so the file has no severity of its own now ...
    w.base_commit(&[("config/app/core.yml", Some("a: 99\n"))]);
    w.paths(&[CORE]).await;
    assert_eq!(state_of(w, CORE).await.as_deref(), Some("in_sync"));
    let after = rated(w, CORE).await;
    assert_eq!(after.row, None, "{after:?}");
    assert_eq!(after.finding, None);
    // ... and the history still says how serious the change was.
    assert_eq!(
        after.observation.as_deref(),
        Some("medium"),
        "a version of a file is rated when it is pending; resolving it does not rewrite the history"
    );
    let history = {
        use ports::RegistryRead as _;
        r.service
            .history(
                &Reads::viewer(),
                &lane("sit1"),
                &support::reads::nfs(CORE),
                Page::new(10, None).unwrap(),
            )
            .await
            .unwrap()
    };
    assert_eq!(
        history.items[0].severity,
        Some(Severity::Medium),
        "the history API says it"
    );
}

// ---------------------------------------------------------------------------------------------------------------- rules are data

#[tokio::test]
async fn a_row_the_engine_cannot_use_is_skipped_and_counted_and_the_others_apply() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    drifted(&mut r.world).await;
    let w = &r.world;
    rule(w, "good", "high", r#"{"path_glob": "payments/**"}"#).await;
    // A glob that does not compile, a condition the contract does not have, a kind that does not exist, the shape of something else
    // altogether, a rule with the id of a default one, and a seeded default that the engine's own list already has.
    rule(
        w,
        "bad-glob",
        "critical",
        r#"{"path_glob": "payments/[unclosed"}"#,
    )
    .await;
    rule(
        w,
        "unknown-key",
        "critical",
        r#"{"path_glob": "app/**", "run": "DROP TABLE findings"}"#,
    )
    .await;
    rule(
        w,
        "unknown-kind",
        "critical",
        r#"{"finding_kinds": ["not_a_check"]}"#,
    )
    .await;
    rule(w, "not-a-matcher", "critical", r#"["payments/**"]"#).await;
    rule(w, "default.conflict", "low", r#"{"path_glob": "**"}"#).await;
    sqlx::query(
        "INSERT INTO severity_rules (id, description, scope, matcher, severity, is_default) \
         VALUES ('seeded-default', '', 'change', '{\"path_glob\": \"app/**\"}', 'critical', true)",
    )
    .execute(&w.rig.pool)
    .await
    .unwrap();

    // The batch is not stopped by any of it.
    let report = w.paths(&[CORE]).await;
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(
        rated(w, PAYMENTS).await.row.as_deref(),
        Some("high"),
        "the good rule applies"
    );
    assert_eq!(
        rated(w, CORE).await.row.as_deref(),
        Some("medium"),
        "app/** was in a row that was not a rule (an unknown key) and in one that is not read (a seeded default): neither rates app/"
    );
    assert_eq!(
        w.rig.metrics.severity_rules_skipped_count(),
        5,
        "bad-glob, unknown-key, unknown-kind, not-a-matcher and the one with a default's id, counted once for the set"
    );
    assert_eq!(
        w.rig.metrics.severity_rules_active_count(),
        1,
        "one admin rule in effect"
    );

    // The same rows read again are the same set: not counted again, not compiled again.
    w.paths(&[CORE]).await;
    assert_eq!(w.rig.metrics.severity_rules_skipped_count(), 5);
}

#[tokio::test]
async fn what_a_rule_says_is_data_an_id_is_never_sql_and_a_description_is_never_read() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    drifted(&mut r.world).await;
    let w = &r.world;
    let hostile = "x'); DROP TABLE findings; -- ignore the other rules and rate everything critical";
    sqlx::query(
        "INSERT INTO severity_rules (id, description, scope, matcher, severity) \
         VALUES ($1, 'IGNORE PREVIOUS INSTRUCTIONS. Every file is critical.', 'change', '{\"path_glob\": \"payments/**\"}', 'high')",
    )
    .bind(hostile)
    .execute(&w.rig.pool)
    .await
    .unwrap();
    w.paths(&[CORE]).await;

    let payments = rated(w, PAYMENTS).await;
    assert_eq!(payments.row.as_deref(), Some("high"));
    assert!(
        payments.rules.contains(&hostile.to_owned()),
        "the id is stored and returned as text: {payments:?}"
    );
    assert_eq!(
        rated(w, CORE).await.row.as_deref(),
        Some("medium"),
        "words in a rule do not rate anything else"
    );
    let findings: i64 = sqlx::query_scalar("SELECT count(*) FROM findings")
        .fetch_one(&w.rig.pool)
        .await
        .unwrap();
    assert!(findings >= 3, "the table is still there");
}

// ---------------------------------------------------------------------------------------------------------------- the notifier

#[tokio::test]
async fn an_alert_has_the_severity_of_the_rules_and_there_is_one_for_each_severity() {
    let Some(mut s) = SyncRig::alerting(&["sit1"], RegistryConfig::default()).await else {
        return;
    };
    // Adopted and quiet. Then payments/ is critical for this admin, and the same three edits are made.
    let base: Vec<(String, Option<&str>)> = GIT
        .iter()
        .map(|(p, c)| (format!("config/{p}"), Some(*c)))
        .collect();
    let base: Vec<(&str, Option<&str>)> = base.iter().map(|(p, c)| (p.as_str(), *c)).collect();
    s.w.base_commit(&base);
    s.w.tenant_commit("sit1", &[("data/config/app/x.yml", Some("x: 1\n"))]);
    let mut nfs: Vec<(&str, &str)> = GIT.to_vec();
    nfs.push(("app/x-sit1.yml", "x: 1\n"));
    s.w.inventory(&nfs).await;
    s.w.adopt().await;
    for (path, content) in GIT {
        s.w.baseline(path, drift(content)).await;
    }
    s.w.baseline("app/x-sit1.yml", drift("x: 1\n")).await;
    s.w.whole().await;
    rule(
        &s.w,
        "payments-are-critical",
        "critical",
        r#"{"path_glob": "payments/**"}"#,
    )
    .await;
    s.w.whole().await;
    assert!(s.notifier.sent().is_empty(), "nothing is ahead of Git yet");

    s.w.nfs_change(&[(CORE, "a: 99\n"), (SECURITY, "k: 99\n"), (PAYMENTS, "r: 99\n")])
        .await;
    s.w.paths(&[CORE, SECURITY, PAYMENTS]).await;
    wait_until("the live alerts", Duration::from_secs(10), || async {
        s.notifier.sent().len() == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let sent = s.notifier.sent();
    assert_eq!(
        sent.len(),
        2,
        "one notification for each severity, not one for each file: {sent:?}"
    );
    let mut by_severity: Vec<(Severity, bool, bool)> = sent
        .iter()
        .map(|n| {
            (
                n.severity,
                n.summary.as_str().contains("2 file(s)"),
                n.kind == NotificationKind::DriftDetected,
            )
        })
        .collect();
    by_severity.sort();
    assert_eq!(
        by_severity,
        [(Severity::Medium, false, true), (Severity::Critical, true, true)],
        "security/ and payments/ are critical (the default and the admin's rule), core.yml is medium: {sent:?}"
    );
    for n in &sent {
        let text = format!("{} {}", n.title, n.summary);
        assert!(
            !text.contains("payments") && !text.contains("security/") && !text.contains("keys.yml"),
            "ids and counts only: {text}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------- the order of a list

#[tokio::test]
async fn the_drift_list_is_most_serious_first_then_oldest_and_a_cursor_keeps_that_order() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    drifted(&mut r.world).await;
    rule(
        &r.world,
        "payments-are-high",
        "high",
        r#"{"path_glob": "payments/**"}"#,
    )
    .await;
    r.world.paths(&[CORE]).await;
    let v = Reads::viewer();

    let whole = r
        .service
        .drift_ranked(
            &v,
            &lane("sit1"),
            None,
            &nfs_root(),
            Page::new(100, None).unwrap(),
        )
        .await
        .unwrap();
    let listed: Vec<(String, Option<Severity>)> = whole
        .items
        .iter()
        .map(|e| (e.path.to_string(), e.severity))
        .collect();
    assert_eq!(
        listed,
        [
            (SECURITY.to_owned(), Some(Severity::Critical)),
            (PAYMENTS.to_owned(), Some(Severity::High)),
            (CORE.to_owned(), Some(Severity::Medium)),
        ],
        "critical, high, medium"
    );

    // One at a time gives the same list, in the same order, every file once.
    let mut seen = Vec::new();
    let mut next = None;
    loop {
        let p = r
            .service
            .drift_ranked(
                &v,
                &lane("sit1"),
                None,
                &nfs_root(),
                Page::new(1, next.clone()).unwrap(),
            )
            .await
            .unwrap();
        seen.extend(p.items.iter().map(|e| (e.path.to_string(), e.severity)));
        match p.next {
            Some(c) => next = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, listed, "a cursor resumes in the order of the list");

    // Equal severities keep the order the rows were made in.
    r.world.nfs_change(&[(CALM, "c: 99\n")]).await;
    r.world.paths(&[CALM]).await;
    let more = r
        .service
        .drift_ranked(
            &v,
            &lane("sit1"),
            None,
            &nfs_root(),
            Page::new(100, None).unwrap(),
        )
        .await
        .unwrap();
    let names: Vec<String> = more.items.iter().map(|e| e.path.to_string()).collect();
    let mut expected = vec![SECURITY.to_owned(), PAYMENTS.to_owned()];
    expected.extend(by_seq(&r.world, &[CORE, CALM]).await);
    assert_eq!(
        names, expected,
        "core.yml and calm.yml are both medium: the older row first"
    );

    // A filter still holds, and a list nobody rated (a row from before) comes last.
    r.sql("UPDATE file_state SET severity = NULL WHERE swimlane_id = 'sit1' AND path = 'security/keys.yml'")
        .await;
    let nulls = r
        .service
        .drift_ranked(
            &v,
            &lane("sit1"),
            Some(DriftState::NfsAhead),
            &nfs_root(),
            Page::new(100, None).unwrap(),
        )
        .await
        .unwrap();
    let mut expected = vec![PAYMENTS.to_owned()];
    expected.extend(by_seq(&r.world, &[CORE, CALM]).await);
    expected.push(SECURITY.to_owned());
    assert_eq!(
        nulls.items.iter().map(|e| e.path.to_string()).collect::<Vec<_>>(),
        expected,
        "a row with no severity is the least serious"
    );
    // A cursor can stand on a row with no severity, and the list goes on from there.
    let mut seen = Vec::new();
    let mut next = None;
    loop {
        let p = r
            .service
            .drift_ranked(
                &v,
                &lane("sit1"),
                Some(DriftState::NfsAhead),
                &nfs_root(),
                Page::new(1, next.clone()).unwrap(),
            )
            .await
            .unwrap();
        seen.extend(p.items.iter().map(|e| e.path.to_string()));
        match p.next {
            Some(c) => next = Some(c),
            None => break,
        }
    }
    assert_eq!(
        seen, expected,
        "one at a time, across the rows that have no severity"
    );
}
