//! The registry and the write paths in the composed hub (plan 18, T5; S4, S8, S9, S10, S18, P2; rule 8): the process that
//! `hub::process::entry` starts, against a real Postgres, with an agent that joins through the real agent listener, a browser with a
//! session, and GitHub faked at the HTTP edge.
//!
//! What is proved:
//!
//! - **A report lands and a read returns it.** An agent whose swimlane an administrator registered joins with a Google ID token, gets
//!   its certificate from the hub's CA, opens the stream with it, sends a scan, and the registry's read routes answer with what it
//!   sent: the tree, the content, and the blob by its hash (through the registry's policy). Without a session the same routes are
//!   `401`.
//! - **A write runs end to end** with the real `GithubClient` against a server in this process: storing a token checks it with
//!   GitHub, keeps it encrypted, and writes exactly one audit event; so does deleting it; so does an edit that goes to the agent over
//!   the stream. A retry with the same key does nothing twice. The token is in no log line, and the chain of audit events verifies.
//! - **Each job runs on one replica.** Two hubs on the one database: every lease has one holder, and the replica that does not hold
//!   a job's lease does not run it.
//! - **Push to drift** through the composed hub meets P2: a signed webhook for a push moves a drift state within 10 seconds.
//!
//! `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes a missing daemon a failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown
)]

#[path = "support/child.rs"]
mod child;
#[path = "support/composed.rs"]
mod composed;
// The fake server is hub-writes' own test support, included as it is: its clippy allowances are that crate's tests' crate-level ones.
#[allow(dead_code, clippy::large_futures)]
#[path = "../../hub-writes/tests/support/fake_github.rs"]
mod fake_github;
#[path = "support/git_remote.rs"]
mod git_remote;
#[path = "support/net.rs"]
mod net;
#[path = "support/platform_support.rs"]
mod platform_support;
#[path = "support/rig.rs"]
mod rig;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

use std::sync::Arc;
use std::time::{Duration, Instant};

use composed::{
    BASE_REPO, Composed, GITHUB_LOGIN, GITHUB_TOKEN, GOOGLE_TOKEN, SWIMLANE, WEBHOOK_SECRET, eventually,
    full_scan, hash_of,
};
use domain::{Role, UserStatus};
use hub_identity::audit::{ChainVerifier, EcdsaP256Verifier, MemoryBucket};
use serde_json::{Value, json};

const A_YML: &[u8] = b"a: 1\n";
const B_YML: &[u8] = b"b:\n  c: 2\n";

fn json_of(reply: &net::Reply) -> Value {
    serde_json::from_str(&reply.body).unwrap_or(Value::Null)
}

/// Whether the tree of `swimlane` lists `path`.
async fn tree_lists(c: &Composed, cookies: &composed::Cookies, path: &str) -> bool {
    let reply = c
        .get(cookies, &format!("/api/v1/swimlanes/{SWIMLANE}/tree"))
        .await;
    reply.status == 200
        && json_of(&reply)["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["path"] == path))
}

/// The drift state the tree reports for `path`, if the tree lists it.
async fn drift_of(c: &Composed, cookies: &composed::Cookies, path: &str) -> Option<String> {
    let reply = c
        .get(cookies, &format!("/api/v1/swimlanes/{SWIMLANE}/tree"))
        .await;
    json_of(&reply)["items"]
        .as_array()?
        .iter()
        .find(|item| item["path"] == path)
        .and_then(|item| item["drift"].as_str().map(str::to_owned))
}

/// The audit chain of the whole database verifies (every link and hash; no checkpoints are asked of an empty bucket).
async fn chain_is_clean(c: &Composed) {
    let report = ChainVerifier::new(
        c.pool.clone(),
        Arc::new(MemoryBucket::new()),
        Arc::new(EcdsaP256Verifier::new()),
    )
    .verify()
    .await
    .unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert!(report.events_checked > 0);
}

// ---------------------------------------------------------------------------------------------------- a report, and a read

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agents_report_lands_and_a_read_route_returns_it() {
    let Some(c) = Composed::start(1, &[]).await else {
        return;
    };
    let (_, viewer) = c.signed_in(1, Some(Role::Viewer), UserStatus::Active).await;

    // Before anything is reported the swimlane exists and is empty; and the read routes are the guard's: no session, no answer.
    let tree = format!("/api/v1/swimlanes/{SWIMLANE}/tree");
    let empty = c.get(&viewer, &tree).await;
    assert_eq!(empty.status, 200, "{empty:?}");
    assert_eq!(json_of(&empty)["items"], json!([]));
    for path in [
        tree.as_str(),
        "/api/v1/swimlanes",
        "/api/v1/findings",
        "/api/v1/grid",
    ] {
        let anonymous = c.web(0, "GET", path, None, &[], None).await;
        assert_eq!(anonymous.status, 401, "{path}: {anonymous:?}");
    }

    // The agent joins through the real listener (a Google ID token for the registered service account, a certificate from the
    // hub's CA), says hello and sends its first scan.
    let agent = c.connect_agent(0, SWIMLANE, GOOGLE_TOKEN).await;
    assert_eq!(agent.chain_der.len(), 2, "the agent's certificate and the CA's");
    assert_eq!(
        c.count("SELECT count(*) FROM agents").await,
        1,
        "the join is recorded"
    );
    agent
        .send_delta(full_scan(1, &[("a.yml", A_YML), ("svc/b.yml", B_YML)], 5_000))
        .await;

    eventually(
        Duration::from_secs(30),
        "the tree to list the reported file",
        || async { tree_lists(&c, &viewer, "a.yml").await },
    )
    .await;

    // The content is what the agent sent, and the blob endpoint serves it by its hash under the registry's policy.
    let content = c
        .get(
            &viewer,
            &format!("/api/v1/swimlanes/{SWIMLANE}/files/content?path=a.yml"),
        )
        .await;
    assert_eq!(content.status, 200, "{content:?}");
    let body = json_of(&content);
    assert_eq!(body["hash"], hash_of(A_YML).to_string(), "{body}");
    assert!(
        body["text"].as_str().unwrap_or_default().contains("a: 1"),
        "{body}"
    );
    let blob = c.get(&viewer, &format!("/api/v1/blobs/{}", hash_of(A_YML))).await;
    assert_eq!(blob.status, 200, "{blob:?}");
    assert_eq!(blob.body, "a: 1\n");
    assert_eq!(
        blob.header("cache-control"),
        Some("private, max-age=31536000, immutable")
    );

    // The swimlane's summary says its agent is connected, and the report is in the audit chain, which verifies.
    let summary = c.get(&viewer, &format!("/api/v1/swimlanes/{SWIMLANE}")).await;
    assert_eq!(summary.status, 200, "{summary:?}");
    assert_eq!(json_of(&summary)["id"], SWIMLANE);
    assert!(
        c.count(&format!(
            "SELECT count(*) FROM audit_events WHERE swimlane_id = '{SWIMLANE}'"
        ))
        .await
            > 0,
        "an applied report writes its audit event in its transaction"
    );
    chain_is_clean(&c).await;
}

// ---------------------------------------------------------------------------------------------------- one list of deny globs

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_one_list_of_deny_globs_withholds_a_files_content_from_the_read_api_and_the_blob_endpoint() {
    // `LK_EXTRA_DENY_GLOBS` is the registry's, the blob policy's and the write path's list (D79).
    let Some(c) = Composed::start(1, &[("LK_EXTRA_DENY_GLOBS", "secrets/**")]).await else {
        return;
    };
    let (_, viewer) = c.signed_in(6, Some(Role::Viewer), UserStatus::Active).await;
    let secret = b"key: not-for-viewers\n";
    let agent = c.connect_agent(0, SWIMLANE, GOOGLE_TOKEN).await;
    agent
        // The same bytes are also reported under a path nobody denies: the store keeps what an allowed path has (a denied file's
        // bytes are never stored), so the blob exists, and it is the endpoint's policy that must refuse it.
        .send_delta(full_scan(
            1,
            &[("ok.yml", A_YML), ("copy.yml", secret), ("secrets/k.yml", secret)],
            5_000,
        ))
        .await;
    eventually(Duration::from_secs(30), "the report to land", || async {
        c.count(&format!(
            "SELECT count(*) FROM file_state WHERE swimlane_id = '{SWIMLANE}'"
        ))
        .await
            == 3
    })
    .await;
    // The file is flagged denied by the hub's own list, and its content is withheld by the read API.
    let denied: bool =
        sqlx::query_scalar("SELECT denied FROM file_state WHERE swimlane_id = $1 AND path = 'secrets/k.yml'")
            .bind(SWIMLANE)
            .fetch_one(&c.pool)
            .await
            .unwrap();
    assert!(denied, "the hub's list denies secrets/**");
    let content = c
        .get(
            &viewer,
            &format!("/api/v1/swimlanes/{SWIMLANE}/files/content?path=secrets/k.yml"),
        )
        .await;
    assert_eq!(content.status, 200, "{content:?}");
    let body = json_of(&content);
    assert_eq!(body["content_withheld"], true, "{body}");
    assert!(
        !content.body.contains("not-for-viewers"),
        "the text of a denied file is in the answer"
    );
    // The blob endpoint answers its hash as it answers an unknown one, and still serves an ordinary file's. (Content addressing: the
    // hash is refused while any path that has had that content is denied, an over-refusal the registry accepts, D79.)
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE hash = sha256($1)")
        .bind(secret.as_slice())
        .fetch_one(&c.pool)
        .await
        .unwrap();
    assert_eq!(stored, 1, "the bytes are in the store, so a 404 is the policy's");
    let blob = c
        .get(&viewer, &format!("/api/v1/blobs/{}", hash_of(secret)))
        .await;
    assert_eq!(blob.status, 404, "{blob:?}");
    assert!(!blob.body.contains("not-for-viewers"));
    let ok = c.get(&viewer, &format!("/api/v1/blobs/{}", hash_of(A_YML))).await;
    assert_eq!(ok.status, 200, "{ok:?}");
    // The write path refuses the same path (it is built from the same list): a 403 or a 422, and not a write.
    let (_, editor) = c.signed_in(7, Some(Role::Editor), UserStatus::Active).await;
    sqlx::query("UPDATE swimlanes SET editing_unlocked = true WHERE id = $1")
        .bind(SWIMLANE)
        .execute(&c.pool)
        .await
        .unwrap();
    let mark = c.audit_mark().await;
    let edit = json!({
        "path": "secrets/k.yml",
        "expected": { "state": "hash", "hash": hash_of(secret).to_string() },
        "content": "key: changed\n",
    });
    let refused = c
        .web(
            0,
            "PUT",
            &format!("/api/v1/swimlanes/{SWIMLANE}/files"),
            Some(&editor),
            &[("Idempotency-Key", "d-1")],
            Some(&edit),
        )
        .await;
    assert!(matches!(refused.status, 403 | 422), "{refused:?}");
    assert!(
        !agent
            .received()
            .iter()
            .any(|command| matches!(command, domain::HubCommand::WriteFile { .. })),
        "the agent was asked to write a denied file"
    );
    assert!(
        c.audit_after(mark)
            .await
            .iter()
            .all(|action| action != "file_edited"),
        "no edit event for a denied file"
    );
}

// ---------------------------------------------------------------------------------------------------- every route has a guard

fn concrete(path: &str) -> String {
    path.split('/')
        .map(|segment| if segment.starts_with('{') { "x1" } else { segment })
        .collect::<Vec<_>>()
        .join("/")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_registry_and_write_routes_are_mounted_behind_the_guard_in_the_composed_process() {
    let Some(c) = Composed::start(1, &[]).await else {
        return;
    };
    let (_, viewer) = c.signed_in(4, Some(Role::Viewer), UserStatus::Active).await;
    let (_, pending) = c.signed_in(5, None, UserStatus::Pending).await;
    // The routes the two crates say they mount: the registry's fifteen reads and the write paths' eighteen.
    let mut routes: Vec<(String, String, Role)> = hub_registry::registry_route_specs()
        .into_iter()
        .map(|spec| {
            let hub_platform::RouteGuard::Role(role) = spec.guard else {
                panic!("a registry route is public: {}", spec.path)
            };
            (spec.method, spec.path, role)
        })
        .collect();
    routes.extend(
        hub_writes::routes::WRITE_ROUTES
            .iter()
            .map(|(method, path, role)| ((*method).to_owned(), (*path).to_owned(), *role)),
    );
    assert_eq!(routes.len(), 15 + 18);
    for (method, template, role) in &routes {
        let path = concrete(template);
        // An unregistered path under `/api` is a 404, so a 401 says: mounted, and behind the guard.
        let anonymous = c.web(0, method, &path, None, &[], None).await;
        assert_eq!(anonymous.status, 401, "{method} {template}: {anonymous:?}");
        // A pending user passes nothing.
        let pending_user = c.web(0, method, &path, Some(&pending), &[], None).await;
        assert_eq!(pending_user.status, 403, "{method} {template}: {pending_user:?}");
        // A Viewer is stopped by the role of the route, before the handler reads anything.
        if *role > Role::Viewer {
            let reply = c.web(0, method, &path, Some(&viewer), &[], None).await;
            assert_eq!(reply.status, 403, "{method} {template} needs {role:?}: {reply:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------------- a write, end to end

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_write_has_exactly_one_audit_event_and_the_chain_verifies() {
    let Some(c) = Composed::start(1, &[]).await else {
        return;
    };
    let (_, editor) = c.signed_in(2, Some(Role::Editor), UserStatus::Active).await;
    let token_path = "/api/v1/me/github-token";
    let body = json!({ "token": GITHUB_TOKEN });

    // 1. Storing a token: the real client asks GitHub who it belongs to and whether it can push to both repositories, the vault
    //    wraps a fresh data key with KMS and keeps the ciphertext, and one audit event is written.
    let mark = c.audit_mark().await;
    let stored = c
        .web(
            0,
            "PUT",
            token_path,
            Some(&editor),
            &[("Idempotency-Key", "t-1")],
            Some(&body),
        )
        .await;
    assert_eq!(stored.status, 200, "{stored:?}");
    assert_eq!(json_of(&stored)["github_login"], GITHUB_LOGIN);
    assert_eq!(c.audit_after(mark).await, ["github_token_stored"]);
    let paths: Vec<String> = c.github.seen().into_iter().map(|s| s.path).collect();
    assert!(paths.contains(&"/user".to_owned()), "{paths:?}");
    for repo in [BASE_REPO, composed::TENANT_REPO] {
        assert!(paths.contains(&format!("/repos/{repo}")), "{paths:?}");
    }
    assert_eq!(c.count("SELECT count(*) FROM github_credentials").await, 1);
    // The token is on the wire to GitHub as the bearer and nowhere else: not in the table, in plain.
    let plain: bool = sqlx::query_scalar(
        "SELECT position(convert_to($1, 'UTF8') in ciphertext) > 0 OR position(convert_to($1, 'UTF8') in wrapped_dek) > 0 \
         FROM github_credentials",
    )
    .bind(GITHUB_TOKEN)
    .fetch_one(&c.pool)
    .await
    .unwrap();
    assert!(!plain, "the token is stored encrypted");

    // 2. The same request again (a retry): the stored answer, and no second event.
    let mark = c.audit_mark().await;
    let again = c
        .web(
            0,
            "PUT",
            token_path,
            Some(&editor),
            &[("Idempotency-Key", "t-1")],
            Some(&body),
        )
        .await;
    assert_eq!(again.status, 200, "{again:?}");
    assert!(
        c.audit_after(mark).await.is_empty(),
        "a retry with the same key writes nothing"
    );

    // 3. A token GitHub does not know is refused and writes nothing.
    let bad =
        json!({ "token": "github_pat_11T5UNKNOWN0123456789abcdefghijklmnopqrstuvwxyz_ABCDEFGHIJKLMNOP" });
    let refused = c
        .web(
            0,
            "PUT",
            token_path,
            Some(&editor),
            &[("Idempotency-Key", "t-2")],
            Some(&bad),
        )
        .await;
    assert_eq!(refused.status, 422, "{refused:?}");
    assert!(
        c.audit_after(mark).await.is_empty(),
        "a refused token leaves no event"
    );

    // 4. A token that matches its own cookie and not the session (what a page of another origin could set) is stopped by the
    //    hub-wide layer before the write path sees it.
    let other = composed::Cookies {
        csrf: "A".repeat(43),
        ..editor.clone()
    };
    let forged = c
        .web(
            0,
            "PUT",
            token_path,
            Some(&other),
            &[("Idempotency-Key", "t-3")],
            Some(&body),
        )
        .await;
    assert_eq!(forged.status, 403, "{forged:?}");
    assert!(c.audit_after(mark).await.is_empty());

    // 5. Deleting it: one more event.
    let mark = c.audit_mark().await;
    let deleted = c
        .web(
            0,
            "DELETE",
            token_path,
            Some(&editor),
            &[("Idempotency-Key", "t-4")],
            None,
        )
        .await;
    assert_eq!(deleted.status, 204, "{deleted:?}");
    assert_eq!(c.audit_after(mark).await, ["github_token_deleted"]);
    assert_eq!(c.count("SELECT count(*) FROM github_credentials").await, 0);

    // 6. An edit that goes to the agent over its stream. The swimlane's baseline is confirmed (editing unlocked), the agent has
    //    reported the file, and the agent answers the write with success.
    sqlx::query("UPDATE swimlanes SET editing_unlocked = true WHERE id = $1")
        .bind(SWIMLANE)
        .execute(&c.pool)
        .await
        .unwrap();
    let agent = c.connect_agent(0, SWIMLANE, GOOGLE_TOKEN).await;
    agent.send_delta(full_scan(1, &[("a.yml", A_YML)], 5_000)).await;
    eventually(Duration::from_secs(30), "the report to land", || async {
        c.count(&format!(
            "SELECT count(*) FROM file_state WHERE swimlane_id = '{SWIMLANE}' AND path = 'a.yml'"
        ))
        .await
            == 1
    })
    .await;
    let mark = c.audit_mark().await;
    let edit = json!({
        "path": "a.yml",
        "expected": { "state": "hash", "hash": hash_of(A_YML).to_string() },
        "content": "a: 2\n",
    });
    let edited = c
        .web(
            0,
            "PUT",
            &format!("/api/v1/swimlanes/{SWIMLANE}/files"),
            Some(&editor),
            &[("Idempotency-Key", "e-1")],
            Some(&edit),
        )
        .await;
    assert_eq!(edited.status, 200, "{edited:?}");
    assert_eq!(json_of(&edited)["outcome"], "applied", "{edited:?}");
    assert!(
        agent
            .received()
            .iter()
            .any(|command| matches!(command, domain::HubCommand::WriteFile { .. })),
        "the write reached the agent"
    );
    let events = c.audit_after(mark).await;
    assert_eq!(events.len(), 1, "one event for one applied edit: {events:?}");
    // A hash that is not the file's is a conflict (rule 11): the hub sends the hash it expects and the agent refuses on a mismatch,
    // so the file is as it was and no event is written. The first edit's content is what the agent holds.
    assert_eq!(agent.file("a.yml"), Some(hash_of(b"a: 2\n")));
    let mark = c.audit_mark().await;
    let stale = json!({
        "path": "a.yml",
        "expected": { "state": "hash", "hash": hash_of(b"not what is on NFS\n").to_string() },
        "content": "a: 3\n",
    });
    let conflict = c
        .web(
            0,
            "PUT",
            &format!("/api/v1/swimlanes/{SWIMLANE}/files"),
            Some(&editor),
            &[("Idempotency-Key", "e-2")],
            Some(&stale),
        )
        .await;
    assert_eq!(conflict.status, 409, "{conflict:?}");
    assert!(c.audit_after(mark).await.is_empty(), "a conflict writes no event");
    assert_eq!(
        agent.file("a.yml"),
        Some(hash_of(b"a: 2\n")),
        "and the file is as it was"
    );

    // The token is in no line the hub wrote (S8, S10), and the chain, which holds every event above, verifies.
    let output = c.hubs[0].child.output();
    assert!(!output.contains(GITHUB_TOKEN), "the token is in the hub's output");
    chain_is_clean(&c).await;
}

// ---------------------------------------------------------------------------------------------------- the jobs, on one replica

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_job_runs_under_a_lease_on_one_replica_only() {
    // The pull-request job runs every second here (it is the one that polls for work), so a run is seen in a moment.
    let Some(c) = Composed::start(2, &[("LK_WRITES_PR_JOBS_SECS", "1")]).await else {
        return;
    };
    let jobs = [
        "writes-reconcile",
        "writes-expire-proposals",
        "writes-pr-jobs",
        "writes-credential-cleanup",
        "registry-pipeline",
        "registry-nightly",
        "registry-sync-watch",
    ];
    eventually(Duration::from_secs(30), "every job to have a holder", || async {
        let mut all = true;
        for job in jobs {
            all &= c
                .count(&format!(
                    "SELECT count(*) FROM leases WHERE name = '{job}' AND expires_at > now()"
                ))
                .await
                == 1;
        }
        all
    })
    .await;
    // One row per name is what a lease is, and its holder is one of the two replicas.
    let holders: Vec<(String, String)> =
        sqlx::query_as("SELECT name, holder FROM leases WHERE name = ANY($1)")
            .bind(jobs.iter().map(|j| (*j).to_owned()).collect::<Vec<_>>())
            .fetch_all(&c.pool)
            .await
            .unwrap();
    assert_eq!(holders.len(), jobs.len());
    assert!(
        holders
            .iter()
            .all(|(_, holder)| holder == "hub-0" || holder == "hub-1"),
        "{holders:?}"
    );

    // The job that polls every second has run on the holder, and on nobody else.
    let holder_of_pr_jobs = &holders
        .iter()
        .find(|(name, _)| name == "writes-pr-jobs")
        .unwrap()
        .1;
    let (holding, other) = if holder_of_pr_jobs == "hub-0" {
        (0, 1)
    } else {
        (1, 0)
    };
    let runs = |metrics: &str| -> f64 {
        metrics
            .lines()
            .filter(|l| l.starts_with("lanekeeper_job_runs_total") && l.contains(r#"job="writes-pr-jobs""#))
            .filter_map(|l| l.rsplit(' ').next()?.parse::<f64>().ok())
            .sum()
    };
    eventually(
        Duration::from_secs(30),
        "the holder to have run the job",
        || async {
            let metrics = net::http_get(c.hubs[holding].ops(), "/metrics").await.body;
            runs(&metrics) >= 2.0
        },
    )
    .await;
    let other_metrics = net::http_get(c.hubs[other].ops(), "/metrics").await.body;
    assert!(
        runs(&other_metrics) == 0.0,
        "the replica that does not hold the lease ran the job: {other_metrics}"
    );
    // And the replica that holds it says so, and the other says it does not.
    let held = |metrics: &str| {
        metrics
            .lines()
            .find(|l| l.starts_with("lanekeeper_job_lease_held") && l.contains(r#"job="writes-pr-jobs""#))
            .and_then(|l| l.rsplit(' ').next().map(str::to_owned))
    };
    let holding_metrics = net::http_get(c.hubs[holding].ops(), "/metrics").await.body;
    assert_eq!(held(&holding_metrics).as_deref(), Some("1"), "{holding_metrics}");
    assert_ne!(held(&other_metrics).as_deref(), Some("1"), "{other_metrics}");
}

// ---------------------------------------------------------------------------------------------------- push to drift (P2)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_to_drift_through_the_composed_hub_meets_p2() {
    let Some(c) = Composed::start(1, &[]).await else {
        return;
    };
    let (_, viewer) = c.signed_in(3, Some(Role::Viewer), UserStatus::Active).await;
    // The agent reports the file as Git has it (`a: 1`): in sync.
    let agent = c.connect_agent(0, SWIMLANE, GOOGLE_TOKEN).await;
    agent.send_delta(full_scan(1, &[("a.yml", A_YML)], 5_000)).await;
    eventually(Duration::from_secs(30), "the report to land", || async {
        tree_lists(&c, &viewer, "a.yml").await
    })
    .await;
    // The pipeline has computed it against Git: the two agree, so the file is in sync (and the swimlane has a baseline for it).
    eventually(
        Duration::from_secs(60),
        "the file to be computed in sync",
        || async { drift_of(&c, &viewer, "a.yml").await.as_deref() == Some("in_sync") },
    )
    .await;

    // A push to the base repository's default branch changes the file in Git. The delivery is signed as GitHub signs it.
    let outbox_mark = c.count("SELECT COALESCE(max(id), 0) FROM outbox").await;
    let before = c.rig.git.remote.head("main").unwrap();
    let after = c
        .rig
        .git
        .remote
        .commit("main", &[("config/a.yml", Some(b"a: 2\n"))], "change a");
    let body = git_remote::fake_github::push_body(BASE_REPO, "refs/heads/main", &before, &after);
    let signature = git_remote::fake_github::signature(WEBHOOK_SECRET, &body);
    let pushed = Instant::now();
    let reply = net::https_request(
        c.hubs[0].web(),
        &c.rig.ca.cert_der(),
        "POST",
        "/hooks/github",
        &[
            ("Content-Type", "application/json"),
            ("X-GitHub-Event", "push"),
            ("X-GitHub-Delivery", "d-1"),
            ("X-Hub-Signature-256", &signature),
        ],
        Some(&body),
    )
    .await;
    assert!(matches!(reply.status, 200 | 202), "{reply:?}");

    eventually(Duration::from_secs(10), "DriftChanged within P2's 10 seconds", || async {
        c.count(&format!(
            "SELECT count(*) FROM outbox WHERE id > {outbox_mark} AND event #>> '{{event,type}}' = 'drift_changed' \
             AND (event #> '{{event,paths}}') @> to_jsonb('a.yml'::text)"
        ))
        .await
            > 0
    })
    .await;
    let took = pushed.elapsed();
    assert!(took < Duration::from_secs(10), "{took:?}");
    // And the state moved with it. The swimlane has not been adopted, so there is no baseline to say which side moved (D19: no
    // baseline and NFS differs from Git is `unknown`), and the file is no longer in sync.
    eventually(
        Duration::from_secs(10),
        "the tree to show the new state",
        || async { drift_of(&c, &viewer, "a.yml").await.as_deref() == Some("unknown") },
    )
    .await;
    let drift = c
        .get(&viewer, &format!("/api/v1/swimlanes/{SWIMLANE}/drift"))
        .await;
    assert_eq!(drift.status, 200, "{drift:?}");

    // A delivery with the wrong signature is refused by the webhook itself (S18), through the same composed router.
    let forged = net::https_request(
        c.hubs[0].web(),
        &c.rig.ca.cert_der(),
        "POST",
        "/hooks/github",
        &[
            ("Content-Type", "application/json"),
            ("X-GitHub-Event", "push"),
            ("X-GitHub-Delivery", "d-2"),
            (
                "X-Hub-Signature-256",
                "sha256=0000000000000000000000000000000000000000000000000000000000000000",
            ),
        ],
        Some(&body),
    )
    .await;
    assert!(matches!(forged.status, 401 | 403), "{forged:?}");
}

// ---------------------------------------------------------------------------------------------------- the webhook's HMAC (S18)

/// One delivery of a `push` event to the web listener of the first hub, over TLS, as GitHub would send it but for the signature, which
/// the caller gives (or leaves out).
async fn deliver(c: &Composed, delivery: &str, signature: Option<&str>, body: &[u8]) -> net::Reply {
    let mut headers = vec![
        ("Content-Type", "application/json"),
        ("X-GitHub-Event", "push"),
        ("X-GitHub-Delivery", delivery),
    ];
    if let Some(signature) = signature {
        headers.push(("X-Hub-Signature-256", signature));
    }
    net::https_request(
        c.hubs[0].web(),
        &c.rig.ca.cert_der(),
        "POST",
        "/hooks/github",
        &headers,
        Some(body),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_webhook_still_requires_a_valid_hmac_through_the_composed_hub() {
    // S18: the webhook has no session, so the HMAC over the raw body is the whole of its authentication. Through the composed router
    // (the layers, the body cap, the error normaliser), a delivery whose signature is missing, malformed or made by anyone but the
    // holder of the secret, or over some other body, is `401` and leaves no trace in the delivery log; the genuine one is taken.
    let Some(c) = Composed::start(1, &[]).await else {
        return;
    };
    let before = c.rig.git.remote.head("main").unwrap();
    let after = c
        .rig
        .git
        .remote
        .commit("main", &[("config/a.yml", Some(b"a: 3\n"))], "change a");
    let body = git_remote::fake_github::push_body(BASE_REPO, "refs/heads/main", &before, &after);
    let genuine = git_remote::fake_github::signature(WEBHOOK_SECRET, &body);
    let other_body = git_remote::fake_github::push_body(BASE_REPO, "refs/heads/main", &after, &before);
    let deliveries = || c.count("SELECT count(*) FROM webhook_deliveries");
    let log_before = deliveries().await;

    let wrong_secret = git_remote::fake_github::signature("not-the-webhook-secret", &body);
    let for_another_body = git_remote::fake_github::signature(WEBHOOK_SECRET, &other_body);
    let zeros = format!("sha256={}", "0".repeat(64));
    let no_prefix = genuine.trim_start_matches("sha256=").to_owned();
    let sha1_form = format!("sha1={}", &genuine["sha256=".len()..][..40]);
    let upper = genuine.replace("sha256=", "SHA256=");
    for (what, signature) in [
        ("no signature header", None),
        ("an empty signature", Some(String::new())),
        ("all zeros", Some(zeros)),
        ("the right digest without its `sha256=` prefix", Some(no_prefix)),
        ("a `sha1=` header", Some(sha1_form)),
        ("the prefix in upper case", Some(upper)),
        ("a signature made under another secret", Some(wrong_secret)),
        ("a genuine signature over another body", Some(for_another_body)),
        (
            "a truncated signature",
            Some(genuine[..genuine.len() - 2].to_owned()),
        ),
    ] {
        let reply = deliver(&c, "forged-1", signature.as_deref(), &body).await;
        assert_eq!(reply.status, 401, "{what}: {reply:?}");
    }
    assert_eq!(
        deliveries().await,
        log_before,
        "a refused delivery left a row in the delivery log"
    );

    // The genuine one is taken, so the refusals above are about the signature and not a route that refuses everything.
    let reply = deliver(&c, "genuine-1", Some(&genuine), &body).await;
    assert!(matches!(reply.status, 200 | 202), "{reply:?}");
    assert_eq!(deliveries().await, log_before + 1);
}
