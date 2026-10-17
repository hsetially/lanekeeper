//! The blob endpoint under the hub's own deny globs, and after a denied file is gone (05 fix round 1, T9, D104; S10, S17, S21).
//!
//! `GET /api/v1/blobs/{hash}` asks the registry's policy before it serves a hash. The policy used to read only the stored flag, so it
//! had two windows that every other read did not have (D104: "the one rule is applied wherever one file is decided"):
//!
//! - **The hub's globs.** The hub restarts with other `extra_deny_globs` and no agent has reported the files again, so the rows still
//!   say "ordinary": REST and the port withhold the file because of the glob, and the endpoint served its content to a viewer who knew
//!   the hash (a file's hash is shown in the tree).
//! - **A deleted file.** The pipeline drops the row of a file that is gone, and the flag with it, so the second probe of the flag no
//!   longer refused the old versions of a denied file.
//!
//! The policy now also reads the files that have had the content (the paths, the flags and the classes, by an index that exists) and
//! applies `crate::denied::is_denied` to them, bounded, and failing closed. Every refusal here has a control: the same hub without the
//! glob, or the same file before it was deleted, serves the blob, so a refusal is the rule and not a blob that cannot be read. All of it
//! runs through the real ingestion and pipeline, and through hub-platform's own endpoint.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use domain::ContentHash;
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use hub_platform::blobs::endpoint::{BlobPolicy, BlobState, mount};
use hub_platform::{GuardedRouter, TEST_USER_HEADER};
use hub_registry::blob_policy::{DENIED_HASH_SQL, HASH_PATHS_LIMIT, HASH_PATHS_SQL};
use hub_registry::{RegistryBlobPolicy, RegistryConfig, RegistryMetrics};
use ports::{BlobStore, content_hash};
use prometheus_client::registry::Registry;
use support::log_capture::LogCapture;
use support::reads::Reads;
use support::rig::entry;
use tower::ServiceExt;

/// In the two versions of the file that only the hub's glob denies.
const GLOB_V1: &str = "PLANTED-GLOB-V1-4e7a21";
const GLOB_V2: &str = "PLANTED-GLOB-V2-90c3bd";
/// In the version of the vault that the agent never flagged, and in the one that arrived denied (never sent).
const VAULT_V1: &str = "PLANTED-VAULT-V1-5d18f6";
const VAULT_V2: &str = "PLANTED-VAULT-V2-c8a047";
/// In the file whose agent flagged the very version that the store has.
const FLIPPED_V1: &str = "PLANTED-FLIPPED-V1-b3902d";
const OTHER: &str = "PLANTED-OTHER-1b62e9";

const CORE: &str = "core.yml";
const GLOBBED: &str = "conf/glob-only.yml";
const VAULT: &str = "conf/vault.yml";
const FLIPPED: &str = "conf/flipped.yml";
const ORDINARY: &str = "conf/other.yml";
const GLOB: &str = "*glob-only*";

fn glob_v1() -> String {
    format!("name: glob\napi_secret: {GLOB_V1}\n")
}
fn glob_v2() -> String {
    format!("name: glob\napi_secret: {GLOB_V2}\n")
}
fn vault_v1() -> String {
    format!("name: vault\napi_secret: {VAULT_V1}\n")
}
fn vault_v2() -> String {
    format!("name: vault\napi_secret: {VAULT_V2}\n")
}
fn flipped_v1() -> String {
    format!("name: flipped\napi_secret: {FLIPPED_V1}\n")
}
fn ordinary() -> String {
    format!("name: other\nnote: {OTHER}\n")
}

fn h(text: &str) -> ContentHash {
    content_hash(text.as_bytes())
}

/// What the agent says of a file it will not read: the hash, the size and the flag, and no bytes.
fn denied_entry(path: &str, content: &str, at: i64) -> domain::ScanEntry {
    let mut e = entry(path, content.as_bytes(), at);
    e.denied = true;
    e.bytes = None;
    e
}

/// One swimlane with five files, as ingestion and the pipeline leave them:
///
/// - `core.yml` and `conf/other.yml`: ordinary files, never denied by anyone.
/// - `conf/glob-only.yml`: ordinary, with two versions. The hub's glob `*glob-only*` (given to a policy below) is the only thing that denies
///   it: no row says so.
/// - `conf/vault.yml`: an ordinary first version in the store, then a new version that its agent flagged denied (it never sent the bytes).
/// - `conf/flipped.yml`: the agent flagged the very version that the store has (the same hash, the flag set).
async fn world() -> Option<Reads> {
    let mut r = Reads::new(&["sit1"]).await?;
    r.world
        .inventory(&[
            (CORE, "port: 1\n"),
            (GLOBBED, &glob_v1()),
            (VAULT, &vault_v1()),
            (FLIPPED, &flipped_v1()),
            (ORDINARY, &ordinary()),
        ])
        .await;
    r.world.nfs_change(&[(GLOBBED, &glob_v2())]).await;
    r.world
        .nfs_raw(|at| vec![denied_entry(VAULT, &vault_v2(), at)])
        .await;
    r.world
        .nfs_raw(|at| vec![denied_entry(FLIPPED, &flipped_v1(), at)])
        .await;
    Some(r)
}

/// A policy of a hub that has `globs` as its own deny globs.
fn policy(r: &Reads, globs: &[&str]) -> (RegistryBlobPolicy, RegistryMetrics) {
    let metrics = RegistryMetrics::default();
    let config = RegistryConfig {
        extra_deny_globs: globs.iter().map(|g| (*g).to_owned()).collect(),
        ..r.config.clone()
    };
    let policy = RegistryBlobPolicy::new(r.world.rig.pool.clone(), &config, metrics.clone()).unwrap();
    (policy, metrics)
}

/// `GET /api/v1/blobs/{hash}` through hub-platform's own endpoint, over the store of the test, behind `policy`.
async fn get(r: &Reads, policy: RegistryBlobPolicy, hash: &ContentHash) -> (StatusCode, Bytes) {
    let store: Arc<dyn BlobStore> = Arc::clone(&r.world.rig.blobs) as _;
    let (router, _) = mount(
        GuardedRouter::new(support::http::auth(), None),
        BlobState::new(store, Arc::new(policy)),
    )
    .into_parts()
    .unwrap();
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/blobs/{hash}"))
        .header(TEST_USER_HEADER, "viewer")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    (parts.status, body.collect().await.unwrap().to_bytes())
}

async fn flag_of(r: &Reads, path: &str) -> Option<bool> {
    sqlx::query_scalar("SELECT denied FROM file_state WHERE swimlane_id = 'sit1' AND path = $1")
        .bind(path)
        .fetch_optional(&r.world.rig.pool)
        .await
        .unwrap()
}

async fn in_the_store(r: &Reads, hash: &ContentHash) -> bool {
    r.world.rig.blobs.get(hash).await.unwrap().is_some()
}

fn exposition(metrics: &RegistryMetrics) -> String {
    let mut registry = Registry::default();
    metrics.register(&mut registry);
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
    text
}

// ------------------------------------------------------------------------------------------------ the hub's own globs

#[tokio::test]
async fn a_hub_whose_globs_changed_before_the_agent_said_so_refuses_the_blob_by_hash() {
    let Some(r) = world().await else { return };
    // The premise: the row has not been told, and the bytes are in the store (that is what is at stake).
    assert_eq!(
        flag_of(&r, GLOBBED).await,
        Some(false),
        "the row has not been told"
    );
    let (v1, v2) = (h(&glob_v1()), h(&glob_v2()));
    assert!(in_the_store(&r, &v1).await && in_the_store(&r, &v2).await);

    // The control: a hub without the glob serves both versions, by the policy and through the endpoint, bytes and all.
    let (plain, _) = policy(&r, &[]);
    assert!(plain.allowed(&v1).await, "the old version");
    assert!(plain.allowed(&v2).await, "the current version");
    let (plain, _) = policy(&r, &[]);
    let (status, body) = get(&r, plain, &v2).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        String::from_utf8_lossy(&body).contains(GLOB_V2),
        "without the glob the blob is an ordinary file's"
    );

    // The hub that restarted with the glob: both versions are refused, the old one as well as the one the file has now.
    let (globbed, metrics) = policy(&r, &[GLOB]);
    assert!(
        !globbed.allowed(&v2).await,
        "the current version of a file that the hub's own glob denies"
    );
    assert!(
        !globbed.allowed(&v1).await,
        "and the version it had before: every version a denied file has had"
    );
    assert_eq!(metrics.blob_withheld_count(), 2);

    // Through hub-platform's endpoint a refused hash is answered as an unknown one is, and carries nothing.
    let (globbed, _) = policy(&r, &[GLOB]);
    let (unknown_status, unknown_body) = get(&r, globbed, &h("nobody has ever seen this")).await;
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    for hash in [&v1, &v2] {
        let (globbed, _) = policy(&r, &[GLOB]);
        let (status, body) = get(&r, globbed, hash).await;
        assert_eq!(
            (status, &body),
            (unknown_status, &unknown_body),
            "indistinguishable"
        );
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains(GLOB_V1) && !text.contains(GLOB_V2));
    }

    // What is not denied is served by the same hub: the glob names one file and not the store.
    for (text, planted) in [("port: 1\n".to_owned(), "port: 1"), (ordinary(), OTHER)] {
        let (globbed, _) = policy(&r, &[GLOB]);
        let (status, body) = get(&r, globbed, &h(&text)).await;
        assert_eq!(status, StatusCode::OK, "an unrelated blob");
        assert!(String::from_utf8_lossy(&body).contains(planted));
    }
}

// ------------------------------------------------------------------------------------------------ a deleted file

#[tokio::test]
async fn a_denied_files_versions_are_refused_after_the_file_is_deleted_and_the_pipeline_dropped_its_row() {
    let Some(mut r) = world().await else { return };
    let vault_v1 = h(&vault_v1());
    let flipped = h(&flipped_v1());
    let (v1, v2) = (h(&glob_v1()), h(&glob_v2()));
    assert!(in_the_store(&r, &vault_v1).await && in_the_store(&r, &flipped).await);

    // Before: the agent's flag keeps the vault and the flipped file out (the old probe's job), and the glob keeps the third out.
    let (plain, _) = policy(&r, &[]);
    assert!(
        !plain.allowed(&vault_v1).await,
        "the version a file had before its next version arrived denied"
    );
    assert!(!plain.allowed(&flipped).await);
    let (globbed, _) = policy(&r, &[GLOB]);
    assert!(!globbed.allowed(&v2).await);

    // The three files are deleted on NFS and the pipeline drops their rows (a file that is gone has no row, and so no flag).
    r.world.nfs_delete(&[VAULT, FLIPPED, GLOBBED, ORDINARY]).await;
    r.world.whole().await;
    for path in [VAULT, FLIPPED, GLOBBED, ORDINARY] {
        assert_eq!(flag_of(&r, path).await, None, "the row of {path} is gone");
    }

    // After: nothing in `file_state` says anything, and the blobs are in the store still. The files' own observations do.
    assert!(in_the_store(&r, &vault_v1).await && in_the_store(&r, &flipped).await);
    let (plain, _) = policy(&r, &[]);
    assert!(
        !plain.allowed(&vault_v1).await,
        "the file's newest classified observation says it was denied: its old version is refused although its row is gone"
    );
    assert!(
        !plain.allowed(&flipped).await,
        "the version whose own observation was flagged"
    );
    let (globbed, _) = policy(&r, &[GLOB]);
    assert!(
        !globbed.allowed(&v1).await && !globbed.allowed(&v2).await,
        "a deleted file that the hub's glob names: both versions"
    );
    // The controls: the same deleted file is an ordinary one to a hub without the glob, and an ordinary deleted file's versions are served.
    let (plain, _) = policy(&r, &[]);
    assert!(plain.allowed(&v1).await && plain.allowed(&v2).await);
    let (globbed, _) = policy(&r, &[GLOB]);
    assert!(
        globbed.allowed(&h(&ordinary())).await,
        "the deleted ordinary file's content is served (an unrelated blob)"
    );
    let (plain, _) = policy(&r, &[]);
    let (status, body) = get(&r, plain, &h(&ordinary())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains(OTHER));
    let (plain, _) = policy(&r, &[]);
    assert_eq!(
        get(&r, plain, &vault_v1).await.0,
        StatusCode::NOT_FOUND,
        "through the endpoint, as an unknown hash"
    );
}

#[tokio::test]
async fn a_file_that_stopped_being_denied_before_it_was_deleted_is_not_refused() {
    // The rule is the current one, as in every read: a file that its agent stopped flagging is ordinary again, and so are its versions,
    // also when it is deleted later (its newest classified observation is then an ordinary one).
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    r.world.inventory(&[(CORE, "port: 1\n")]).await;
    r.world
        .nfs_raw(|at| vec![denied_entry(CORE, "port: 1\n", at)])
        .await;
    let (plain, _) = policy(&r, &[]);
    assert!(!plain.allowed(&h("port: 1\n")).await, "while it is flagged");
    r.world.nfs_raw(|at| vec![entry(CORE, b"port: 1\n", at)]).await;
    assert_eq!(flag_of(&r, CORE).await, Some(false), "the flag is cleared again");
    let (plain, _) = policy(&r, &[]);
    assert!(
        plain.allowed(&h("port: 1\n")).await,
        "and then it is readable again"
    );
    r.world.nfs_delete(&[CORE]).await;
    r.world.whole().await;
    assert_eq!(flag_of(&r, CORE).await, None);
    let (plain, _) = policy(&r, &[]);
    assert!(
        plain.allowed(&h("port: 1\n")).await,
        "deleted later, the last thing known of the file is that it was ordinary"
    );
}

// ------------------------------------------------------------------------------------------------ bounded, and failing closed

/// `count` observations of `hash` at paths `bulk/f<from>.yml` to `bulk/f<from + count - 1>.yml`: a content that was copied everywhere.
async fn plant(r: &Reads, hash: &ContentHash, from: usize, count: usize) {
    r.sql(&format!(
        "INSERT INTO file_observations (swimlane_id, path, hash, source, class, observed_at) \
         SELECT 'sit1', 'bulk/f' || n || '.yml', '\\x{}', 'scan', 'structured', now() - (n || ' seconds')::interval \
         FROM generate_series({from}, {}) n",
        hex::encode(hash.as_bytes()),
        from + count - 1
    ))
    .await;
}

/// Like [`plant`], and the last of the `count` observations (the last rows of the hash in every index, as they are written by one statement
/// on one connection in order) is an observation of `path`, an older version of a file that exists: `count` sightings of `hash`, the file's
/// the last one.
async fn plant_ending_with(r: &Reads, hash: &ContentHash, from: usize, count: usize, path: &str) {
    let last = from + count - 1;
    r.sql(&format!(
        "INSERT INTO file_observations (swimlane_id, path, hash, source, class, observed_at) \
         SELECT 'sit1', CASE WHEN n = {last} THEN '{path}' ELSE 'bulk/f' || n || '.yml' END, '\\x{}', 'scan', 'structured', \
                now() - (n || ' seconds')::interval \
         FROM generate_series({from}, {last}) n ORDER BY n",
        hex::encode(hash.as_bytes()),
    ))
    .await;
}

#[tokio::test]
async fn a_hash_that_more_files_have_had_than_the_policy_reads_is_refused_as_undecided() {
    let Some(r) = world().await else { return };
    let copied = h("a content that was copied into a great many places\n");
    plant(&r, &copied, 1, HASH_PATHS_LIMIT).await;
    let (policy, metrics) = policy(&r, &[]);
    assert!(
        policy.allowed(&copied).await,
        "at the limit the hash is decided, and nothing that has had it is denied"
    );
    assert_eq!(metrics.blob_withheld_count(), 0);

    plant(&r, &copied, HASH_PATHS_LIMIT + 1, 1).await;
    assert!(
        !policy.allowed(&copied).await,
        "one more than the policy reads: it cannot say that none of them is denied, so the blob is withheld"
    );
    assert_eq!(metrics.blob_withheld_count(), 1);
    // The bound is on what is read, not on what exists: with four times the limit in the table, the statement the policy runs (sqlx's own
    // log of it says how many rows came back) returns the limit and one row more, and no more.
    plant(&r, &copied, HASH_PATHS_LIMIT + 2, 3 * HASH_PATHS_LIMIT).await;
    let (capture, _guard) = LogCapture::install();
    assert!(!policy.allowed(&copied).await);
    let ran = support::statements::statements(&capture);
    assert!(
        ran.len() >= 2,
        "the probe of the flag and the statement of the paths: {}",
        ran.len()
    );
    assert_eq!(
        ran.iter().map(|s| s.rows).max(),
        Some(u64::try_from(HASH_PATHS_LIMIT + 1).unwrap()),
        "the paths statement returned the limit plus one rows of {} observations",
        4 * HASH_PATHS_LIMIT
    );
    // And a hash that the limit did not touch is decided as before.
    assert!(policy.allowed(&h(&ordinary())).await);
}

/// The statements of the policy among the ones sqlx logged on this thread (the rig's outbox relay runs its own, which are not these).
fn policy_statements(capture: &LogCapture) -> Vec<support::statements::Statement> {
    support::statements::statements(capture)
        .into_iter()
        .filter(|s| s.text.contains("FROM file_state") || s.text.contains("FROM file_observations"))
        .collect()
}

/// The decisions that the flag probe and the paths statement share, with a popular content (05 FIX, review round 2, non-blocking 1). The
/// probe of the flag asks one thing, "does a denied file have this content now?", through a partial index, so its cost does not grow with
/// the files that have had the content; every other question (a denied file that had the content once, the hub's globs, a deleted file) is
/// the paths statement's, and it reads at most the limit plus one rows and refuses a hash that has more. These two tests pin that the
/// answer to every combination of "a denied file" and "many ordinary sightings" is still a refusal, and that the flag is what decides when
/// a file's current content is the hash (one statement, however many files have had it).
#[tokio::test]
async fn a_denied_files_current_content_is_refused_by_the_probe_of_the_flag_however_many_files_have_had_it() {
    let Some(r) = world().await else { return };
    let flipped = h(&flipped_v1());
    // The control: the same flagged file with no other sighting is refused too, by one statement.
    let (policy, metrics) = policy(&r, &[]);
    {
        let (capture, _guard) = LogCapture::install();
        assert!(!policy.allowed(&flipped).await);
        assert_eq!(policy_statements(&capture).len(), 1);
    }
    // 1,500 ordinary files have had the same bytes (a key that was copied everywhere): more than the policy reads.
    plant(&r, &flipped, 1, 3 * HASH_PATHS_LIMIT / 2).await;
    let (capture, _guard) = LogCapture::install();
    assert!(
        !policy.allowed(&flipped).await,
        "a denied file's content is refused however many ordinary files have had it"
    );
    let ran = policy_statements(&capture);
    assert_eq!(
        ran.len(),
        1,
        "the probe of the flag decided it: the statement that reads the paths did not have to run"
    );
    assert!(
        ran[0].text.contains("nfs_hash = $1 AND denied"),
        "{}",
        ran[0].text
    );
    assert_eq!(metrics.blob_withheld_count(), 2);
}

#[tokio::test]
async fn a_denied_files_earlier_version_is_refused_at_the_limit_and_above_it_whatever_order_the_rows_come_in()
{
    // A file that was ordinary when its content was recorded and that is denied now: the probe of the flag does not see this hash (the
    // file's current content is another one), so the paths statement decides. It reads the rows of the hash in no order, and a denied
    // row that is not among the ones it returns must not be a way to a "yes": a hash with more sightings than the limit is refused whole.
    let Some(mut r) = world().await else { return };
    let (policy, metrics) = policy(&r, &[]);

    // At the limit: the file's own sighting and `limit - 1` others, and the file is ordinary, then denied.
    let at_the_limit = h("a content a file had before it was denied: at the limit\n");
    plant(&r, &at_the_limit, 10_000, HASH_PATHS_LIMIT - 1).await;
    r.world
        .nfs_change(&[(
            "conf/late-a.yml",
            "a content a file had before it was denied: at the limit\n",
        )])
        .await;
    assert!(
        policy.allowed(&at_the_limit).await,
        "the control: {HASH_PATHS_LIMIT} sightings, none of them denied"
    );
    r.world
        .nfs_raw(|at| vec![denied_entry("conf/late-a.yml", "its denied version A\n", at)])
        .await;
    assert_eq!(flag_of(&r, "conf/late-a.yml").await, Some(true));
    assert!(
        !policy.allowed(&at_the_limit).await,
        "the file is denied now, and this hash is one of its earlier versions"
    );
    assert_eq!(metrics.blob_withheld_count(), 1);
    // The flag alone is enough, whatever the class of the row says (a row that disagrees with itself is denied): the paths statement reads the
    // flag of the file as it is now for the observation of an earlier version, and it is what refuses this hash.
    r.sql(
        "UPDATE file_state SET class = 'structured' WHERE swimlane_id = 'sit1' AND path = 'conf/late-a.yml'",
    )
    .await;
    assert!(
        !policy.allowed(&at_the_limit).await,
        "the flag of the file, with the class of an ordinary one"
    );
    assert_eq!(metrics.blob_withheld_count(), 2);

    // Above the limit, with the file's sighting written after all the others (the last rows of the hash in the index): the statement
    // returns the limit plus one rows of the others and never the file's, so only the count can refuse this hash. (The count of a hash
    // with no denied file is `a_hash_that_more_files_have_had_than_the_policy_reads_is_refused_as_undecided`.) The file is denied, and
    // one of its older versions is the content that 1,500 other files have had.
    let above = h("a content a file had before it was denied: above the limit\n");
    r.world
        .nfs_raw(|at| vec![denied_entry("conf/late-b.yml", "its denied version B\n", at)])
        .await;
    assert_eq!(flag_of(&r, "conf/late-b.yml").await, Some(true));
    plant_ending_with(&r, &above, 20_000, 3 * HASH_PATHS_LIMIT / 2, "conf/late-b.yml").await;
    // The premise, asked of the statement itself: the denied file's sighting is not among the rows that come back. Which rows a `LIMIT`
    // without an `ORDER BY` returns is the planner's: on a database with no statistics it joins the other way round and the rows that
    // have a `file_state` row (this one) come first. So the statistics are gathered, as a running hub's autovacuum does, and the premise
    // is checked and not assumed.
    let mut admin = r.world.rig.db.admin().await;
    sqlx::raw_sql("ANALYZE file_observations; ANALYZE file_state")
        .execute(&mut admin)
        .await
        .unwrap();
    let returned: Vec<(String, bool)> = sqlx::query_as(HASH_PATHS_SQL)
        .bind(above.as_bytes().as_slice())
        .bind(i64::try_from(HASH_PATHS_LIMIT + 1).unwrap())
        .fetch_all(&r.world.rig.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|(path, flagged, _class): (String, bool, Option<String>)| (path, flagged))
        .collect();
    assert_eq!(returned.len(), HASH_PATHS_LIMIT + 1);
    assert!(
        returned
            .iter()
            .all(|(path, flagged)| path != "conf/late-b.yml" && !flagged),
        "the denied file's sighting is outside the rows that the statement returns"
    );
    let (capture, _guard) = LogCapture::install();
    assert!(
        !policy.allowed(&above).await,
        "a denied file's earlier version among more sightings than are read is refused, by the count"
    );
    let ran = policy_statements(&capture);
    assert_eq!(
        ran.iter().map(|s| s.rows).max(),
        Some(u64::try_from(HASH_PATHS_LIMIT + 1).unwrap()),
        "the paths statement ran, and returned the limit plus one rows"
    );
    assert_eq!(metrics.blob_withheld_count(), 3);
}

#[tokio::test]
async fn a_lookup_that_fails_says_no_even_when_the_probe_of_the_flag_worked() {
    let Some(r) = world().await else { return };
    let (policy, _) = policy(&r, &[]);
    assert!(policy.allowed(&h("port: 1\n")).await, "the control: it works");

    // The app role may not read `file_observations` at all. The probe of the flag reads `file_state` only, so it still works, and the
    // statement that reads the paths and the classes is refused by the database.
    let mut admin = r.world.rig.db.admin().await;
    sqlx::raw_sql("REVOKE SELECT ON file_observations FROM lanekeeper_app")
        .execute(&mut admin)
        .await
        .unwrap();
    let flag: bool = sqlx::query_scalar(DENIED_HASH_SQL)
        .bind(h("port: 1\n").as_bytes().as_slice())
        .fetch_one(&r.world.rig.pool)
        .await
        .expect("the probe of the flag still works");
    assert!(!flag);
    for text in ["port: 1\n", "never seen", &ordinary(), &glob_v2()] {
        assert!(
            !policy.allowed(&h(text)).await,
            "fail closed: the second question could not be asked, and the blob is withheld"
        );
    }
}

// ------------------------------------------------------------------------------------------------ what the logs and the metrics hold

#[tokio::test]
async fn no_hash_path_or_content_reaches_a_log_line_or_a_metric_of_the_policy() {
    let Some(r) = world().await else { return };
    let (plain, plain_metrics) = policy(&r, &[]);
    let (globbed, globbed_metrics) = policy(&r, &[GLOB]);
    let copied = h("a content that was copied into a great many places\n");
    plant(&r, &copied, 1, HASH_PATHS_LIMIT + 1).await;
    let hashes = [
        h(&glob_v1()),
        h(&glob_v2()),
        h(&vault_v1()),
        h(&vault_v2()),
        h(&flipped_v1()),
        h(&ordinary()),
        h("port: 1\n"),
        copied,
    ];

    let mut words: Vec<String> = hashes.iter().map(ToString::to_string).collect();
    words.extend(
        [
            GLOB_V1,
            GLOB_V2,
            VAULT_V1,
            VAULT_V2,
            FLIPPED_V1,
            OTHER,
            "glob-only",
            "vault",
            "flipped",
            "bulk/",
            "conf/",
            "core.yml",
        ]
        .map(str::to_owned),
    );

    // What any viewer who knows a hash can ask for at request rate: allowed, refused by the flag, refused by the hub's glob, and refused as
    // undecided (a content that more files have had than the policy reads). Each refusal is counted and none is a warning: a viewer must
    // not be able to fill the log, and no line says which hash it was (S10).
    {
        let (capture, _guard) = LogCapture::install();
        for hash in &hashes {
            let _ = plain.allowed(hash).await;
            let _ = globbed.allowed(hash).await;
        }
        let before = plain_metrics.blob_withheld_count();
        assert!(!plain.allowed(&copied).await, "undecided");
        assert_eq!(
            plain_metrics.blob_withheld_count(),
            before + 1,
            "an undecided hash is counted, as a refusal is"
        );
        let log = capture.text();
        assert!(
            !log.contains("WARN") && !log.contains("ERROR"),
            "a refusal, also of an undecided hash, is an expected outcome and not a warning: {log}"
        );
        for word in &words {
            assert!(!log.contains(word.as_str()), "`{word}` is in a log line: {log}");
        }
    }

    // And two that fail, which an operator has to hear of: the database is gone. The line is a fixed word.
    let (capture, _guard) = LogCapture::install();
    r.world.rig.pool.close().await;
    for hash in &hashes[..2] {
        assert!(!plain.allowed(hash).await);
    }
    let log = capture.text();
    assert!(
        log.contains("WARN"),
        "a failure to ask is said, as a fixed word: {log}"
    );
    for word in &words {
        assert!(!log.contains(word.as_str()), "`{word}` is in a log line: {log}");
    }
    let metrics = format!("{}\n{}", exposition(&plain_metrics), exposition(&globbed_metrics));
    assert!(metrics.contains("registry_blob_withheld"), "{metrics}");
    for word in &words {
        assert!(!metrics.contains(word.as_str()), "`{word}` is in the metrics");
    }
}
