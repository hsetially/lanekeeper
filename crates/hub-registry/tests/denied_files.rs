//! Denied files (T9, D79, S4, S10, S14, S17): they are listed by name, size and hash with the class `denied`, their content is withheld from
//! every read, their hashes are refused by the blob endpoint, a file that becomes denied stops being readable from then on, and the
//! bytes of one never leave through any read path or reach a log line, an error body or a metric.
//!
//! Everything runs through the real ingestion, the real pipeline and the real read service and routes over a real Postgres 16. The
//! tests are adversarial where it matters: a file that was an ordinary file with content in the store and is flagged denied later
//! (the admin adds a deny glob, and the agent reports the same file with the same hash and the flag set), a new version that arrives
//! denied so that older versions stay in the store, an agent that sends the bytes of a key anyway, and a hub whose own globs changed
//! before any agent has scanned again.
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
use domain::{
    AgentReply, ContentHash, ContentSource, DriftState, FileClass, HubCommand, Page, Role, SwimlaneId, User,
    UserStatus,
};
use http::{Method, Request};
use http_body_util::BodyExt;
use hub_platform::blobs::endpoint::{BlobPolicy, BlobState, mount};
use hub_platform::{GuardedRouter, TEST_USER_HEADER};
use hub_registry::{RegistryApi as _, RegistryBlobPolicy, RegistryConfig};
use ports::conformance::sample::user as sample_user;
use ports::{BlobStore, ReadError, RegistryRead as _, content_hash};
use support::http::{Http, Reply};
use support::log_capture::LogCapture;
use support::reads::{Reads, nfs};
use support::rig::entry;
use tower::ServiceExt;

/// In the first version of the two files that become denied. Nothing but those versions has it.
const V1: &str = "PLANTED-V1-3a9c2b";
/// In the version of `REPLACED` that arrives denied: the agent never sends it, so the hub never has it.
const V2: &str = "PLANTED-V2-5b1d7e";
/// In the bytes of a key that a misbehaving agent sends although the file is denied.
const KEY_BYTES: &str = "-----BEGIN PRIVATE KEY-----\nPLANTED-KEY-8d4f0a\n";
/// In the Git copy of the files that become denied.
const GIT: &str = "PLANTED-GIT-6c3e91";
/// In a file that an agent has not flagged and that no row says is denied, but that the hub's own globs deny once it restarts with one more.
const GLOB: &str = "PLANTED-GLOB-2f6d84";

/// An ordinary file whose agent later says it is denied: the same bytes, the same hash, the flag set.
const FLIPPED: &str = "conf/payments-secret.yml";
/// An ordinary file whose next version arrives denied.
const REPLACED: &str = "conf/notes-secret.yml";
/// Denied from the first scan (`*.pem`).
const KEY: &str = "keys/server.pem";
/// An ordinary file, until the hub's own globs are given `*glob-only*`.
const GLOBBED: &str = "conf/glob-only.yml";
/// Never denied.
const CORE: &str = "core.yml";

fn flipped_v1() -> String {
    format!("name: payments\napi_secret: {V1}\n")
}

fn replaced_v1() -> String {
    format!("name: notes\napi_secret: {V1}\n")
}

fn replaced_v2() -> String {
    format!("name: notes\napi_secret: {V2}\n")
}

fn globbed() -> String {
    format!("name: glob\napi_secret: {GLOB}\n")
}

/// What the agent says of a file it will not read: the hash, the size and the flag, and no bytes.
fn denied_entry(path: &str, content: &str, at: i64) -> domain::ScanEntry {
    let mut e = entry(path, content.as_bytes(), at);
    e.denied = true;
    e.bytes = None;
    e
}

/// `sit1` and `sit2` that run an application `core`. `sit1` has ordinary files that are denied by the end of this function in each of the
/// ways above, an adopted baseline for one of them (its raw version is in the store, as adoption leaves it), and Git copies of them.
async fn world() -> Option<Http> {
    build(None).await
}

/// The same world, read by a hub whose own deny globs have one more: it restarted with it, and no agent has scanned since, so no row says
/// anything new.
async fn world_with_glob(glob: &str) -> Option<Http> {
    build(Some(glob)).await
}

async fn build(extra_glob: Option<&str>) -> Option<Http> {
    let mut reads = Reads::new(&["sit1"]).await?;
    reads.world.rig.add_swimlane("sit2", &["sit2"], false).await;
    for id in ["sit1", "sit2"] {
        reads
            .sql(&format!(
                "INSERT INTO services (swimlane_id, namespace, name, env_values) \
                 VALUES ('{id}', 'ns', 'core', '{{\"SPRING_APPLICATION_NAME\": \"core\"}}')"
            ))
            .await;
    }
    reads.world.base_commit(&[
        ("config/core.yml", Some("port: 1\n")),
        (
            "config/conf/payments-secret.yml",
            Some(&format!("api_secret: {GIT}\n")),
        ),
        (
            "config/conf/notes-secret.yml",
            Some(&format!("api_secret: {GIT}\n")),
        ),
    ]);
    reads
        .world
        .tenant_commit("sit1", &[("data/config/core.yml", Some("name: t\n"))]);
    // The first scan: every file is an ordinary one, but the key, which the hub's own globs deny whatever the agent says (it sent the bytes).
    reads
        .world
        .inventory(&[
            (CORE, "port: 2\n"),
            (FLIPPED, &flipped_v1()),
            (REPLACED, &replaced_v1()),
            (GLOBBED, &globbed()),
            (KEY, KEY_BYTES),
        ])
        .await;
    reads
        .world
        .second_lane(
            "sit2",
            &["sit2"],
            &[
                (CORE, "port: 3\n"),
                (FLIPPED, "name: other\n"),
                (GLOBBED, "name: other-glob\n"),
            ],
        )
        .await;
    reads.index_all().await;
    reads.world.adopt().await;
    reads.world.baseline(FLIPPED, drift_of(&flipped_v1())).await;
    reads
        .sql(&format!(
            "UPDATE baselines SET blob_hash = nfs_hash FROM file_state f \
             WHERE baselines.swimlane_id = f.swimlane_id AND baselines.path = f.path AND f.path = '{FLIPPED}'"
        ))
        .await;
    reads.world.whole().await;

    // Later: the agent's deny list now matches the first file (the same version, with the flag), and the second file's next version is
    // one it will not send.
    reads
        .world
        .nfs_raw(|at| vec![denied_entry(FLIPPED, &flipped_v1(), at)])
        .await;
    reads
        .world
        .nfs_raw(|at| vec![denied_entry(REPLACED, &replaced_v2(), at)])
        .await;
    reads.index_all().await;
    reads.world.whole().await;
    if let Some(glob) = extra_glob {
        let config = RegistryConfig {
            extra_deny_globs: vec![glob.to_owned()],
            ..reads.config.clone()
        };
        reads.service = reads.service_with(config.clone());
        reads.config = config;
    }
    reads.service.clear_caches();
    Some(Http::over(reads))
}

fn drift_of(text: &str) -> ContentHash {
    engine::drift_hash(Some(&Bytes::from(text.to_owned())), FileClass::Structured).unwrap()
}

/// The three files, and what each says of the way it is denied.
const DENIED: [&str; 3] = [FLIPPED, REPLACED, KEY];

fn enc(s: &str) -> String {
    s.replace('/', "%2F").replace(':', "%3A").replace('@', "%40")
}

fn viewer() -> User {
    Reads::viewer()
}

// ---------------------------------------------------------------------------------------------------------------- listed

#[tokio::test]
async fn denied_files_are_listed_with_class_denied() {
    let Some(h) = world().await else { return };
    for file in DENIED {
        let row: (String, bool, Option<Vec<u8>>, Option<i64>) = sqlx::query_as(
            "SELECT class, denied, nfs_hash::bytea, size FROM file_state WHERE swimlane_id = 'sit1' AND path = $1",
        )
        .bind(file)
        .fetch_one(&h.reads.world.rig.pool)
        .await
        .unwrap();
        assert_eq!(
            (row.0.as_str(), row.1, row.2.is_some(), row.3.is_some()),
            ("denied", true, true, true),
            "{file}: name, size and hash, and the class"
        );
    }

    // The tree: every denied file is an entry of class denied that says its content is withheld; a folder holding them is not one.
    let tree = h.view("/api/v1/swimlanes/sit1/tree?prefix=conf&limit=500").await;
    assert_eq!(tree.status, 200, "{}", tree.text());
    let entries = tree.json()["items"].as_array().unwrap().clone();
    let by = |name: &str| entries.iter().find(|e| e["path"] == name).unwrap().clone();
    for file in [FLIPPED, REPLACED] {
        let e = by(file);
        assert_eq!(e["class"], "denied", "{file}: {e}");
        assert_eq!(e["content_withheld"], true, "{file}");
        assert!(
            e["hash"].is_string() && e["size"].as_u64().unwrap() > 0,
            "{file}: hash and size are listed"
        );
    }
    let root = h.view("/api/v1/swimlanes/sit1/tree?limit=500").await;
    let root = root.json()["items"].as_array().unwrap().clone();
    let folder = root.iter().find(|e| e["path"] == "keys").unwrap();
    assert_eq!(folder["is_dir"], true);
    assert_ne!(folder["class"], "denied", "a folder is not a denied file");
    let keys = h.view("/api/v1/swimlanes/sit1/tree?prefix=keys").await.json();
    assert_eq!(keys["items"][0]["class"], "denied");
    let core = root.iter().find(|e| e["path"] == CORE).unwrap();
    assert_eq!(core["class"], "structured");
    assert_eq!(core["content_withheld"], false);

    // The drift list: a denied file that is not in sync is in it, as a denied file, and with a severity: a denied file changing is critical.
    let drift = h.view("/api/v1/swimlanes/sit1/drift?limit=500").await;
    let listed: Vec<(String, String, bool, Option<String>)> = drift.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["path"].as_str().unwrap().to_owned(),
                e["class"].as_str().unwrap().to_owned(),
                e["content_withheld"].as_bool().unwrap(),
                e["severity"].as_str().map(str::to_owned),
            )
        })
        .collect();
    for file in DENIED {
        let item = listed.iter().find(|i| i.0 == file);
        if let Some(item) = item {
            assert_eq!((item.1.as_str(), item.2), ("denied", true), "{file}");
            assert_eq!(
                item.3.as_deref(),
                Some("critical"),
                "{file}: a denied file changing (D76)"
            );
        }
    }
    assert!(
        listed.iter().any(|i| i.0 == FLIPPED || i.0 == REPLACED),
        "a denied file that differs from Git is listed: {listed:?}"
    );

    // Through the port as well: the same class and the same flag (the MCP tools read the port).
    let page = h
        .reads
        .service
        .tree(
            &viewer(),
            &support::reads::lane("sit1"),
            &nfs("conf"),
            Page::new(100, None).unwrap(),
        )
        .await
        .unwrap();
    for e in page.items.iter().filter(|e| DENIED.contains(&e.path.as_str())) {
        assert_eq!(e.class, FileClass::Denied, "{}", e.path);
        assert!(e.content_withheld, "{}", e.path);
    }
    assert_eq!(
        page.items
            .iter()
            .filter(|e| DENIED.contains(&e.path.as_str()))
            .count(),
        2,
        "the two that are under conf/"
    );
}

#[tokio::test]
async fn a_file_the_hubs_own_globs_deny_is_listed_as_denied_before_any_agent_has_said_so() {
    let Some(h) = world().await else { return };
    // The hub restarts with a new glob, and no agent has scanned since: the rows are as they were.
    let config = RegistryConfig {
        extra_deny_globs: vec!["core.yml".to_owned()],
        ..h.reads.config.clone()
    };
    let service = h.reads.service_with(config);
    let flag: bool =
        sqlx::query_scalar("SELECT denied FROM file_state WHERE swimlane_id = 'sit1' AND path = 'core.yml'")
            .fetch_one(&h.reads.world.rig.pool)
            .await
            .unwrap();
    assert!(!flag, "the row has not been told");
    let tree = service
        .tree_filtered(
            &viewer(),
            &support::reads::lane("sit1"),
            &domain::NfsPath::root(),
            None,
            Page::new(100, None).unwrap(),
        )
        .await
        .unwrap();
    let core = tree.items.iter().find(|e| e.path.as_str() == "core.yml").unwrap();
    assert_eq!(
        core.class,
        FileClass::Denied,
        "the hub's glob decides too (S17, defence in depth)"
    );
    assert!(core.content_withheld);
    let content = service
        .content(
            &viewer(),
            &support::reads::lane("sit1"),
            &nfs("core.yml"),
            ContentSource::Nfs,
        )
        .await
        .unwrap();
    assert!(content.withheld && content.bytes.is_none());
    for (what, left, right) in [
        ("nfs against nfs", "nfs:sit1", "nfs:sit2"),
        ("nfs against baseline", "nfs:sit1", "baseline:sit1"),
        ("effective against effective", "effective:sit1", "effective:sit2"),
    ] {
        let c = service
            .compare(
                &viewer(),
                left.parse().unwrap(),
                right.parse().unwrap(),
                Some("core.yml".parse().unwrap()),
            )
            .await;
        if let Ok(c) = c {
            assert_eq!(c.class, FileClass::Denied, "{what}");
            assert!(
                c.hunks.is_empty() && c.setting_changes.is_empty(),
                "{what}: nothing of it is compared"
            );
        }
    }
    assert_eq!(
        service
            .effective(
                &viewer(),
                &support::reads::lane("sit1"),
                &"core.yml".parse().unwrap()
            )
            .await
            .unwrap_err(),
        ReadError::NotFound,
        "a denied file has no effective config"
    );
}

// ---------------------------------------------------------------------------------------------------------------- content

#[tokio::test]
async fn content_withheld_has_no_bytes() {
    let Some(h) = world().await else { return };
    let lane = support::reads::lane("sit1");
    // Every source, for every denied file: the answer says withheld, names the class, and holds no bytes.
    for file in DENIED {
        let path = nfs(file);
        let current_hash: Vec<u8> = sqlx::query_scalar(
            "SELECT nfs_hash::bytea FROM file_state WHERE swimlane_id = 'sit1' AND path = $1",
        )
        .bind(file)
        .fetch_one(&h.reads.world.rig.pool)
        .await
        .unwrap();
        let old_hashes: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT DISTINCT hash::bytea FROM file_observations WHERE swimlane_id = 'sit1' AND path = $1 AND hash IS NOT NULL",
        )
        .bind(file)
        .fetch_all(&h.reads.world.rig.pool)
        .await
        .unwrap();
        let mut sources = vec![ContentSource::Nfs, ContentSource::Baseline];
        for old in old_hashes.iter().chain(std::iter::once(&current_hash)) {
            sources.push(ContentSource::Blob {
                hash: ContentHash::from_bytes(old.clone().try_into().unwrap()),
            });
        }
        for source in sources {
            let result = h
                .reads
                .service
                .content(&viewer(), &lane, &path, source.clone())
                .await;
            match result {
                Ok(content) => {
                    assert!(content.withheld, "{file} {source:?}");
                    assert!(
                        content.bytes.is_none(),
                        "{file} {source:?}: withheld means no bytes"
                    );
                    assert_eq!(content.class, FileClass::Denied, "{file} {source:?}");
                }
                // A source the file has none of (no baseline, a version it never had) is not found, which says nothing either.
                Err(e) => assert_eq!(e, ReadError::NotFound, "{file} {source:?}"),
            }
        }
        // From Git: the file is not read there either.
        let git = h
            .reads
            .service
            .content(
                &viewer(),
                &lane,
                &path,
                ContentSource::Git {
                    repo: domain::RepoKind::Base,
                    git_ref: "master".parse().unwrap(),
                },
            )
            .await;
        assert!(
            matches!(&git, Err(ReadError::NotFound))
                || git.as_ref().is_ok_and(|c| c.withheld && c.bytes.is_none()),
            "{file}: {git:?}"
        );
    }

    // Over HTTP: `content_withheld`, no text, and the hash and size that were all the hub ever had.
    for file in DENIED {
        let r = h
            .view(&format!(
                "/api/v1/swimlanes/sit1/files/content?path={}",
                enc(file)
            ))
            .await;
        assert_eq!(r.status, 200, "{file}: {}", r.text());
        let body = r.json();
        assert_eq!(body["content_withheld"], true, "{file}");
        assert_eq!(body["class"], "denied", "{file}");
        assert!(body["text"].is_null(), "{file}: no text");
        assert!(
            body["hash"].is_string(),
            "{file}: the hash is the one thing it has"
        );
    }

    // An ordinary file of the same swimlane is served, so that the above is the denial and not a broken route.
    let ok = h
        .view(&format!(
            "/api/v1/swimlanes/sit1/files/content?path={}",
            enc(CORE)
        ))
        .await;
    assert_eq!(ok.json()["content_withheld"], false);
    assert_eq!(ok.json()["text"], "port: 2\n");

    // The hub never stored a byte of the key the agent sent, nor of the version it would not send.
    let blobs = &h.reads.world.rig.blobs;
    assert!(
        blobs
            .get(&content_hash(KEY_BYTES.as_bytes()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        blobs
            .get(&content_hash(replaced_v2().as_bytes()))
            .await
            .unwrap()
            .is_none()
    );
}

// ---------------------------------------------------------------------------------------------------------------- blobs

fn blob_request(hash: &ContentHash) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/blobs/{hash}"))
        .header(TEST_USER_HEADER, "viewer")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn the_blob_policy_refuses_their_hashes() {
    let Some(h) = world().await else { return };
    let policy = RegistryBlobPolicy::new(
        h.reads.world.rig.pool.clone(),
        &h.reads.config,
        h.reads.world.rig.metrics.clone(),
    );
    let v1 = content_hash(flipped_v1().as_bytes());
    let notes_v1 = content_hash(replaced_v1().as_bytes());
    let store: Arc<dyn BlobStore> = Arc::clone(&h.reads.world.rig.blobs) as _;
    // The bytes of the first versions are in the store: that is what the policy is for.
    assert!(store.get(&v1).await.unwrap().is_some());
    assert!(store.get(&notes_v1).await.unwrap().is_some());

    // Every version a denied file has had is refused, not only the one it has now: the history of the file names them.
    assert!(
        !policy.allowed(&v1).await,
        "the version a file had before its agent flagged it denied"
    );
    assert!(
        !policy.allowed(&notes_v1).await,
        "the version before the one that arrived denied"
    );
    assert!(
        !policy.allowed(&content_hash(replaced_v2().as_bytes())).await,
        "the current version of a denied file"
    );
    // An ordinary file's content, and what nobody has seen, are allowed (the endpoint says 404 for the second itself).
    assert!(policy.allowed(&content_hash(b"port: 2\n")).await);
    assert!(policy.allowed(&content_hash(b"nobody has ever seen this")).await);
    assert!(h.reads.world.rig.metrics.blob_withheld_count() >= 3);

    // Through hub-platform's own endpoint: a refused hash is answered as an unknown one is, and carries nothing.
    let (router, _) = mount(
        GuardedRouter::new(support::http::auth(), None),
        BlobState::new(Arc::clone(&store), Arc::new(policy)),
    )
    .into_parts()
    .unwrap();
    let get = |hash: ContentHash| {
        let router = router.clone();
        async move {
            let response = router.oneshot(blob_request(&hash)).await.unwrap();
            let (parts, body) = response.into_parts();
            (parts.status, body.collect().await.unwrap().to_bytes())
        }
    };
    let unknown = get(content_hash(b"nobody has ever seen this")).await;
    assert_eq!(unknown.0, 404);
    for hash in [v1, notes_v1] {
        let refused = get(hash).await;
        assert_eq!(refused, unknown, "indistinguishable from an unknown hash");
        assert!(!String::from_utf8_lossy(&refused.1).contains(V1));
    }
    assert_eq!(
        get(content_hash(b"port: 2\n")).await.0,
        200,
        "an ordinary blob is served"
    );

    // The file's history says which hashes those are, so the refusal is not security by obscurity: the list is the file's own.
    let history = h
        .view(&format!(
            "/api/v1/swimlanes/sit1/files/history?path={}",
            enc(FLIPPED)
        ))
        .await;
    assert_eq!(history.status, 200);
    assert!(
        history.text().contains(&v1.to_string()),
        "the hashes of the versions are listed"
    );
    assert!(!history.text().contains(V1));

    // Once no denied file has had the content, nothing holds it back: the same bytes under another name are an ordinary file's.
    sqlx::query("DELETE FROM file_state WHERE swimlane_id = 'sit1' AND path = ANY($1::text[])")
        .bind(vec![FLIPPED, REPLACED])
        .execute(&h.reads.world.rig.pool)
        .await
        .unwrap();
    let policy = RegistryBlobPolicy::new(
        h.reads.world.rig.pool.clone(),
        &h.reads.config,
        h.reads.world.rig.metrics.clone(),
    );
    assert!(
        policy.allowed(&v1).await,
        "no file that is denied has had it any more"
    );
}

// ---------------------------------------------------------------------------------------------------------------- becoming denied

#[tokio::test]
async fn a_file_that_becomes_denied_is_denied_from_then_on_and_one_that_stops_being_denied_is_readable_again()
{
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    let content = "name: payments\napi_secret: PLANTED-FLIP-4c1b\n";
    r.world
        .inventory(&[("conf/a.yml", content), ("conf/b.yml", "x: 1\n")])
        .await;
    let row = |r: &Reads| {
        let pool = r.world.rig.pool.clone();
        async move {
            sqlx::query_as::<_, (String, bool)>(
                "SELECT class, denied FROM file_state WHERE swimlane_id = 'sit1' AND path = 'conf/a.yml'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(row(&r).await, ("structured".to_owned(), false));
    let observations = |r: &Reads| {
        let pool = r.world.rig.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM file_observations WHERE swimlane_id = 'sit1' AND path = 'conf/a.yml'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(observations(&r).await, 1);
    r.world.adopt().await;

    // The agent's deny list now has it: the same bytes, so the same hash, with the flag. The hub must act on the flag and not on the hash.
    r.world
        .nfs_raw(|at| vec![denied_entry("conf/a.yml", content, at)])
        .await;
    assert_eq!(
        row(&r).await,
        ("denied".to_owned(), true),
        "a change of the flag is a change of the file, whatever its hash did"
    );
    assert_eq!(observations(&r).await, 2, "and the history says when");
    let events: Vec<String> = r
        .world
        .rig
        .bus
        .published()
        .await
        .into_iter()
        .filter_map(|e| match e {
            domain::DomainEvent::FileObserved { path, .. } if path.as_str() == "conf/a.yml" => {
                Some(path.to_string())
            }
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 2, "the pipeline hears of it: {events:?}");
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE path = 'conf/a.yml' AND action = 'file_change_detected'",
    )
    .fetch_one(&r.world.rig.pool)
    .await
    .unwrap();
    assert_eq!(
        audited, 1,
        "the change of what the hub does with a file is audited (rule 8, D79)"
    );

    // The same message again (the agent retries): nothing more happens.
    let before = observations(&r).await;
    r.world
        .nfs_raw(|at| vec![denied_entry("conf/a.yml", content, at + 5_000)])
        .await;
    assert_eq!(
        observations(&r).await,
        before,
        "a replay of what is stored changes nothing"
    );

    // It is denied for every read now.
    r.world.paths(&["conf/a.yml"]).await;
    let c = r
        .service
        .content(
            &Reads::viewer(),
            &support::reads::lane("sit1"),
            &nfs("conf/a.yml"),
            ContentSource::Nfs,
        )
        .await
        .unwrap();
    assert!(c.withheld && c.bytes.is_none());
    let tree = r
        .service
        .tree_filtered(
            &Reads::viewer(),
            &support::reads::lane("sit1"),
            &nfs("conf"),
            None,
            Page::new(10, None).unwrap(),
        )
        .await
        .unwrap();
    let a = tree
        .items
        .iter()
        .find(|e| e.path.as_str() == "conf/a.yml")
        .unwrap();
    assert_eq!(a.class, FileClass::Denied);
    assert!(a.content_withheld);
    assert_eq!(
        r.world
            .tables()
            .await
            .files
            .keys()
            .filter(|f| f.as_str() == "conf/a.yml")
            .count(),
        0,
        "never an effective of a denied file"
    );

    // And the other way: the admin removes the glob, the agent reads the file again and sends its bytes with the flag cleared.
    r.world
        .nfs_raw(|at| vec![entry("conf/a.yml", content.as_bytes(), at + 10_000)])
        .await;
    assert_eq!(
        row(&r).await,
        ("structured".to_owned(), false),
        "a file that is not denied any more is an ordinary file again"
    );
    r.world.paths(&["conf/a.yml"]).await;
    let c = r
        .service
        .content(
            &Reads::viewer(),
            &support::reads::lane("sit1"),
            &nfs("conf/a.yml"),
            ContentSource::Nfs,
        )
        .await
        .unwrap();
    assert!(!c.withheld && c.bytes.is_some(), "it is served again");
}

// ---------------------------------------------------------------------------------------------------------------- no read path

/// Everything a viewer can read about one file, over HTTP, with a name for the failure message.
fn reads_of(file: &str) -> Vec<(String, String)> {
    let f = enc(file);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |name: &str, uri: String| out.push((name.to_owned(), uri));
    add(
        "content",
        format!("/api/v1/swimlanes/sit1/files/content?path={f}"),
    );
    add(
        "content, baseline",
        format!("/api/v1/swimlanes/sit1/files/content?path={f}&source=baseline"),
    );
    add(
        "content, git",
        format!("/api/v1/swimlanes/sit1/files/content?path={f}&source=git&repo=base&ref=master"),
    );
    add(
        "history",
        format!("/api/v1/swimlanes/sit1/files/history?path={f}"),
    );
    for (name, left, right) in [
        ("compare nfs nfs", "nfs:sit1", "nfs:sit2"),
        ("compare nfs baseline", "nfs:sit1", "baseline:sit1"),
        ("compare effective", "effective:sit1", "effective:sit2"),
        ("compare nfs git", "nfs:sit1", "git:base@master"),
        ("compare git nfs", "git:base@master", "nfs:sit1"),
    ] {
        add(
            name,
            format!("/api/v1/compare?left={}&right={}&file={f}", enc(left), enc(right)),
        );
    }
    add(
        "effective",
        format!("/api/v1/swimlanes/sit1/effective?file={f}&tenant=sit1"),
    );
    add("grid", format!("/api/v1/grid?file={f}&swimlanes=sit1,sit2"));
    add(
        "served",
        format!("/api/v1/swimlanes/sit1/served?application=core&tenant=sit1&file={f}"),
    );
    out
}

/// The reads that are not about one file: the searches (for the planted words and for the names of the settings), the lists, the trees and
/// the swimlane.
fn reads_of_everything(words: &[&str]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for w in words {
        out.push((
            format!("text search for {w}"),
            format!("/api/v1/search/text?pattern={w}"),
        ));
    }
    for (name, uri) in [
        (
            "text search, regex",
            "/api/v1/search/text?pattern=PLANTED-%5BA-Z0-9%5D&regex=true".to_owned(),
        ),
        (
            "text search, setting name",
            "/api/v1/search/text?pattern=api_secret".to_owned(),
        ),
        (
            "settings search",
            "/api/v1/search/settings?q=api_secret".to_owned(),
        ),
        (
            "settings search, name",
            "/api/v1/search/settings?q=name".to_owned(),
        ),
        (
            "settings search, one swimlane",
            "/api/v1/search/settings?q=secret&swimlanes=sit1".to_owned(),
        ),
        (
            "tree",
            "/api/v1/swimlanes/sit1/tree?prefix=conf&limit=500".to_owned(),
        ),
        (
            "tree of the root",
            "/api/v1/swimlanes/sit1/tree?limit=500".to_owned(),
        ),
        ("drift", "/api/v1/swimlanes/sit1/drift?limit=500".to_owned()),
        ("findings", "/api/v1/findings?limit=500".to_owned()),
        (
            "compare tree",
            "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2&limit=500".to_owned(),
        ),
        (
            "compare tree, effective",
            "/api/v1/compare/tree?left=effective%3Asit1&right=effective%3Asit2&limit=500".to_owned(),
        ),
        ("swimlane", "/api/v1/swimlanes/sit1".to_owned()),
        ("swimlanes", "/api/v1/swimlanes".to_owned()),
        (
            "pending restarts",
            "/api/v1/swimlanes/sit1/pending-restarts".to_owned(),
        ),
    ] {
        out.push((name.to_owned(), uri));
    }
    out
}

/// What the config-server would answer for anything, if it were asked: the planted words. A denied file is not asked for.
fn serve_the_planted_words(h: &Http, words: &[&str]) {
    let body = Bytes::from(words.join("\n"));
    h.reads.gateway.attach(
        &SwimlaneId::parse("sit1").unwrap(),
        Arc::new(move |cmd: &HubCommand| match cmd {
            HubCommand::FetchServed { request_id, .. } => Some(AgentReply::Served {
                request_id: request_id.clone(),
                status: 200,
                bytes: body.clone(),
            }),
            _ => None,
        }),
    );
}

async fn replies(h: &Http, reads: Vec<(String, String)>) -> Vec<(String, Reply)> {
    let mut out = Vec::new();
    for (name, uri) in reads {
        out.push((name, h.view(&uri).await));
    }
    out
}

/// The blob endpoint of the platform, behind the registry's policy, asked for these hashes.
async fn blobs_by_hash(h: &Http, hashes: Vec<Vec<u8>>) -> Vec<(String, Reply)> {
    let store: Arc<dyn BlobStore> = Arc::clone(&h.reads.world.rig.blobs) as _;
    let policy = RegistryBlobPolicy::new(
        h.reads.world.rig.pool.clone(),
        &h.reads.config,
        h.reads.world.rig.metrics.clone(),
    );
    let (router, _) = mount(
        GuardedRouter::new(support::http::auth(), None),
        BlobState::new(store, Arc::new(policy)),
    )
    .into_parts()
    .unwrap();
    let mut out = Vec::new();
    for hash in hashes {
        let hash = ContentHash::from_bytes(hash.try_into().unwrap());
        let response = router.clone().oneshot(blob_request(&hash)).await.unwrap();
        let (parts, body) = response.into_parts();
        out.push((
            format!("blob {hash}"),
            Reply {
                status: parts.status,
                headers: parts.headers,
                body: body.collect().await.unwrap().to_bytes(),
            },
        ));
    }
    out
}

/// None of the planted words is in any reply, and the replies are mostly real answers (not a wall of refusals).
fn assert_nothing_leaks(replies: &[(String, Reply)], planted: &[&str], at_least_answers: usize) {
    let mut answers = 0;
    for (name, reply) in replies {
        let text = reply.text();
        for p in planted {
            assert!(
                !text.contains(p),
                "{name} returned a denied file's content ({p}): {text}"
            );
        }
        if reply.status == 200 {
            answers += 1;
        }
    }
    assert!(
        answers >= at_least_answers,
        "the reads are real reads, not a wall of errors: {answers} of {}",
        replies.len()
    );
}

/// The metrics of the registry, as the hub would expose them.
fn metrics_text(h: &Http) -> String {
    let mut registry = prometheus_client::registry::Registry::default();
    h.reads.world.rig.metrics.register(&mut registry);
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
    text
}

fn fetches(h: &Http) -> usize {
    h.reads
        .gateway
        .received(&SwimlaneId::parse("sit1").unwrap())
        .iter()
        .filter(|c| matches!(c, HubCommand::FetchServed { .. }))
        .count()
}

#[tokio::test]
async fn a_denied_files_bytes_leave_through_no_read_path_and_reach_no_log_error_or_metric() {
    let capture = LogCapture::install_everywhere();
    let Some(h) = world().await else { return };
    let planted = [V1, V2, "PLANTED-KEY", GIT];
    serve_the_planted_words(&h, &planted);
    // What is in the store that must not come out: the bytes of the versions the files had before they were denied.
    let store = &h.reads.world.rig.blobs;
    assert!(
        store
            .get(&content_hash(flipped_v1().as_bytes()))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .get(&content_hash(replaced_v1().as_bytes()))
            .await
            .unwrap()
            .is_some()
    );

    let mut reads: Vec<(String, String)> = Vec::new();
    for file in DENIED {
        reads.extend(
            reads_of(file)
                .into_iter()
                .map(|(n, u)| (format!("{n} of {file}"), u)),
        );
    }
    reads.extend(reads_of_everything(&planted));
    let mut all = replies(&h, reads).await;
    // The blob endpoint, for every hash the files have had.
    let hashes: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT DISTINCT hash::bytea FROM file_observations WHERE hash IS NOT NULL AND path = ANY($1::text[])",
    )
    .bind(DENIED.to_vec())
    .fetch_all(&h.reads.world.rig.pool)
    .await
    .unwrap();
    assert!(hashes.len() >= 4, "{}", hashes.len());
    all.extend(blobs_by_hash(&h, hashes).await);

    // Replies include the errors: a problem body is among them too.
    assert!(
        all.iter().any(|(_, r)| r.status != 200),
        "some of the reads are refusals, and are looked at"
    );
    assert_nothing_leaks(&all, &planted, 20);
    assert_eq!(
        fetches(&h),
        0,
        "the agent is not asked for the config-server's copy of a denied file"
    );

    // Logs: from any thread, since compare and search run on the blocking and rayon pools.
    let logs = capture.text();
    for p in planted {
        assert!(
            !logs.contains(p),
            "a log line holds a denied file's content ({p}): {logs}"
        );
    }
    let metrics = metrics_text(&h);
    for p in planted {
        assert!(
            !metrics.contains(p),
            "a metric holds a denied file's content ({p})"
        );
    }
}

#[tokio::test]
async fn a_hub_whose_own_globs_changed_withholds_what_its_rows_have_not_yet_been_told_about() {
    // `conf/glob-only.yml` was an ordinary file: its effective config, its settings and its text were computed and are in the store. The
    // hub restarts with `*glob-only*` in its deny globs, and no agent has scanned since, so its row says it is an ordinary file.
    let Some(h) = world_with_glob("*glob-only*").await else {
        return;
    };
    let flag: bool =
        sqlx::query_scalar("SELECT denied FROM file_state WHERE swimlane_id = 'sit1' AND path = $1")
            .bind(GLOBBED)
            .fetch_one(&h.reads.world.rig.pool)
            .await
            .unwrap();
    assert!(!flag, "the row has not been told");
    assert!(
        h.reads
            .world
            .rig
            .blobs
            .get(&content_hash(globbed().as_bytes()))
            .await
            .unwrap()
            .is_some(),
        "its content is in the store: that is what is at stake"
    );
    let planted = [GLOB];
    serve_the_planted_words(&h, &planted);
    let mut reads = reads_of(GLOBBED);
    reads.extend(reads_of_everything(&planted));
    let all = replies(&h, reads).await;
    assert_nothing_leaks(&all, &planted, 15);
    assert_eq!(fetches(&h), 0, "the config-server's copy is not asked for either");

    // The listings say so: class denied, content withheld.
    let tree = h
        .view("/api/v1/swimlanes/sit1/tree?prefix=conf&limit=500")
        .await
        .json();
    let e = tree["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == GLOBBED)
        .unwrap()
        .clone();
    assert_eq!(
        (e["class"].as_str(), e["content_withheld"].as_bool()),
        (Some("denied"), Some(true)),
        "{e}"
    );
    // And the control: a hub without the glob serves it, so that the above is the glob and not a file that cannot be read.
    let control = world().await.unwrap();
    let r = control
        .view(&format!(
            "/api/v1/swimlanes/sit1/files/content?path={}",
            enc(GLOBBED)
        ))
        .await;
    assert!(r.text().contains(GLOB), "without the glob it is an ordinary file");
}

// ---------------------------------------------------------------------------------------------------------------- what a denied file would fill in

#[tokio::test]
async fn a_denied_property_source_gives_no_value_to_the_text_it_would_have_filled_in() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    r.sql(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) \
         VALUES ('sit1', 'ns', 'core', '{\"SPRING_APPLICATION_NAME\": \"core\"}')",
    )
    .await;
    let source = "greeting: PLANTED-RENDER-91ab3c\n";
    r.world
        .inventory(&[("core/core.yml", source), ("core/msg.txt", "say ${greeting}\n")])
        .await;
    let rendered = |text: &str| engine::normalised_hash(&Bytes::from(text.to_owned()), FileClass::Text);
    let message = |service: &hub_registry::RegistryService| {
        let service = service.clone();
        async move {
            service.clear_caches();
            match service
                .effective_tagged(
                    &Reads::viewer(),
                    &support::reads::lane("sit1"),
                    &"core/msg.txt".parse().unwrap(),
                    Some(&"sit1".parse().unwrap()),
                    None,
                    None,
                )
                .await
                .unwrap()
            {
                hub_registry::Tagged::Fresh { value, .. } => value,
                hub_registry::Tagged::NotModified { .. } => panic!("no tag was given"),
            }
        }
    };

    // The control: the property source is an ordinary file, so its value is filled in.
    let ordinary = message(&r.service).await;
    assert_eq!(ordinary.hash, rendered("say PLANTED-RENDER-91ab3c\n"));

    // The hub's own globs now deny `core/core.yml`, and no agent has scanned since: the row says it is an ordinary file.
    let globbed = r.service_with(RegistryConfig {
        extra_deny_globs: vec!["core/core.yml".to_owned()],
        ..r.config.clone()
    });
    let e = message(&globbed).await;
    assert_eq!(
        e.hash,
        rendered("say ${greeting}\n"),
        "a denied file gives no value to another file's text"
    );
    assert_eq!(
        e.unresolved_placeholders
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["greeting"]
    );

    // The agent says it is denied: the same.
    r.world
        .nfs_raw(|at| vec![denied_entry("core/core.yml", source, at)])
        .await;
    let e = message(&r.service).await;
    assert_eq!(e.hash, rendered("say ${greeting}\n"));
    assert_eq!(
        e.unresolved_placeholders
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["greeting"]
    );
}

// ---------------------------------------------------------------------------------------------------------------- authorization

#[tokio::test]
async fn the_denial_is_behind_the_guard_on_the_route_and_on_the_port() {
    let Some(h) = world().await else { return };
    // Over HTTP the guard answers before anything is looked up: a user who may not read gets a problem body, whatever the file is.
    for user in ["pending", "disabled", "norole"] {
        for (name, uri) in reads_of(FLIPPED) {
            let r = h.get(Some(user), &uri).await;
            assert_eq!(r.status, 403, "{user} {name}");
            assert!(r.body.len() < 400, "{user} {name}: a problem body, not a file");
        }
    }
    for (name, uri) in reads_of(FLIPPED) {
        assert_eq!(h.get(None, &uri).await.status, 401, "{name}");
        // A viewer is the lowest role that reads: the answer is the withheld one, never the file.
        let r = h.get(Some("viewer"), &uri).await;
        assert!(r.status == 200 || r.status == 404, "{name}: {}", r.status);
    }

    // The port is called by MCP without the router: it refuses by itself.
    let lane = support::reads::lane("sit1");
    for user in [
        sample_user(1, Some(Role::Viewer), UserStatus::Pending),
        sample_user(2, Some(Role::Viewer), UserStatus::Disabled),
        sample_user(3, None, UserStatus::Active),
    ] {
        assert_eq!(
            h.reads
                .service
                .content(&user, &lane, &nfs(FLIPPED), ContentSource::Nfs)
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            h.reads
                .service
                .tree(&user, &lane, &nfs("conf"), Page::new(10, None).unwrap())
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            h.reads
                .service
                .drift(
                    &user,
                    &lane,
                    Some(DriftState::NfsAhead),
                    &domain::NfsPath::root(),
                    Page::new(10, None).unwrap()
                )
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
    }
}
