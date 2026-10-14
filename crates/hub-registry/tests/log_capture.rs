//! S10: logs hold ids, paths, hashes and counts, never file contents, settings, environment values or secrets. Secret-looking
//! values are planted in a file, an environment variable (allowlisted and not), a Deployment name and a delta that is refused, and
//! none of them may reach a log line. The capture is proven to work by the lines the failing database produces.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{ClusterReport, DeploymentInfo, EnvValue, PodInfo, ServiceRef, Timestamp};
use support::log_capture::LogCapture;
use support::rig::{Rig, T, delta, entry, root, text};

const PLANTED: [&str; 5] = [
    "PLANTED-FILE-SECRET-7f3a",
    "PLANTED-ENV-VALUE-91bc",
    "PLANTED-ALLOWED-VALUE-c0de",
    "PLANTED-FORGED-BODY-5e5e",
    "PLANTED-ENV-NAME-d00d",
];

#[tokio::test]
async fn planted_secret_never_reaches_a_log_line() {
    let Some(rig) = Rig::new().await else { return };
    let (capture, _guard) = LogCapture::install();

    rig.hello().await;
    // Delta ingest: the content of a file, an out-of-band change, a refused delta, a delta over the limits.
    rig.delta(delta(
        1,
        None,
        root(1),
        vec![entry(
            "app.yml",
            format!("password: {}\n", PLANTED[0]).as_bytes(),
            T,
        )],
    ))
    .await
    .unwrap();
    rig.delta(delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry(
            "app.yml",
            format!("password: {}-changed\n", PLANTED[0]).as_bytes(),
            T + 5_000,
        )],
    ))
    .await
    .unwrap();
    let mut forged = entry("forged.yml", format!("body: {}\n", PLANTED[3]).as_bytes(), T);
    forged.hash = ports::content_hash(b"other");
    assert!(rig.delta(delta(3, None, root(3), vec![forged])).await.is_err());
    let huge = vec![b'x'; 2 * 1024 * 1024 + 1];
    assert!(
        rig.delta(delta(4, None, root(4), vec![entry("huge.yml", &huge, T)]))
            .await
            .is_err()
    );
    // A denied file whose bytes the agent sent anyway.
    assert!(
        rig.delta(delta(
            5,
            None,
            root(5),
            vec![entry(
                "tls/server.key",
                format!("KEY {}", PLANTED[0]).as_bytes(),
                T
            )]
        ))
        .await
        .is_ok(),
        "dropped, not refused: the hub's own globs deny it"
    );

    // Cluster report: environment values (allowlisted and not) and a hostile "name".
    let mut dep = DeploymentInfo {
        service: ServiceRef::new("csp", "svc").unwrap(),
        pods: vec![PodInfo {
            name: text("svc-1"),
            started_at: Timestamp::from_unix_millis(T),
        }],
        env_values: vec![
            EnvValue {
                name: text("DB_PASSWORD"),
                value: text(PLANTED[1]),
            },
            EnvValue {
                name: text("CONFIG_CLIENT_CACHE_TTL"),
                value: text(PLANTED[2]),
            },
        ],
        env_names: vec![text(&format!("name={}", PLANTED[4]))],
        removed: false,
    };
    dep.env_names.push(text("OK_NAME"));
    rig.cluster(ClusterReport {
        full: true,
        deployments: vec![dep],
        release_hints: vec![],
        config_server_started_at: None,
        sync_windows: vec![],
    })
    .await
    .unwrap();

    // A database that fails: this is what produces log lines of the registry, and the secrets must not be in them. A delta with
    // no content to store reaches the database through the registry's own `Db`.
    rig.pool.close().await;
    let mut late = delta(6, Some(root(2)), root(6), vec![]);
    late.removed = vec![support::rig::path("gone.yml")];
    let err = rig.delta(late).await.unwrap_err();

    let text = capture.text();
    let ours: Vec<&str> = text.lines().filter(|l| l.contains(" hub_registry")).collect();
    assert!(
        ours.iter()
            .any(|l| l.contains("database call failed") && l.contains("why=")),
        "the capture works and the failing database was logged: {ours:?}"
    );
    for secret in PLANTED {
        assert!(!text.contains(secret), "{secret} reached the log: {text}");
        assert!(!format!("{err} {err:?}").contains(secret));
    }
    // The driver's own lines (sqlx, at debug level) carry statements with `$n` placeholders and never a bound value; the
    // registry's lines carry a fixed word and nothing else.
    for line in ours {
        for sql in ["SELECT", "INSERT", "UPDATE", "unnest"] {
            assert!(
                !line.contains(sql),
                "SQL text reached a log line of the registry: {line}"
            );
        }
    }
}

/// The settings index reads file contents and writes them to a table, so its lines are the ones most likely to leak: a flagged
/// value, a plain value, a file that does not parse, a blob store that fails, a sweep, and a moved Git head over a file that holds a
/// secret. Every one of them logs, and none of them may show a value.
#[tokio::test]
async fn planted_secret_never_reaches_a_log_line_of_the_index() {
    use std::sync::Arc;

    use bytes::Bytes;
    use domain::{FileClass, RepoKind, RepoPath};
    use hub_registry::config::RegistryConfig;
    use hub_registry::{GitIndex, GitIndexDeps, RegistryMetrics, SettingsSweeper};
    use ports::BlobStore;
    use ports::fakes::FakeGit;

    const FLAGGED: &str = "ghp_aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bD1fH3jL5";
    const PLAIN: &str = "PLANTED-PLAIN-VALUE-1a2b";
    const BROKEN: &str = "PLANTED-BROKEN-9f9f";
    const IN_GIT: &str = "PLANTED-GIT-VALUE-77aa";

    let Some(rig) = Rig::with_index("sit1", RegistryConfig::default()).await else {
        return;
    };
    let (capture, _guard) = LogCapture::install();

    let secrets = format!("token: {FLAGGED}\nname: {PLAIN}\n");
    let broken = format!("a: [{BROKEN}\n");
    rig.first_scan(&[("secrets.yml", secrets.as_str()), ("broken.yml", broken.as_str())])
        .await;
    rig.index().idle().await;

    // A blob the store cannot read: the batch fails and says so.
    let failing = format!("k: {PLAIN}-again\n");
    rig.blobs.put(Bytes::from(failing.clone())).await.unwrap();
    rig.blobs.set_failing_reads(true);
    let job = hub_registry::IndexJob {
        hash: ports::content_hash(failing.as_bytes()),
        syntax: engine::Syntax::Yaml,
    };
    assert!(rig.index().index_now(vec![job]).await.is_err());
    rig.blobs.set_failing_reads(false);

    // A sweep over files whose markers are gone, so that it indexes them again.
    sqlx::query("DELETE FROM settings_index_blobs")
        .execute(&rig.pool)
        .await
        .unwrap();
    let (sweeping, _worker) = rig.another_indexer(RegistryConfig::default(), RegistryMetrics::default());
    let swept = SettingsSweeper::new(sweeping).sweep(|| Ok(())).await.unwrap();
    assert_eq!(swept.found, 2);

    // A moved Git head over a file that holds a secret, and the hash of that file.
    rig.add_swimlane("sit1", &["sit1"], true).await;
    let fake = FakeGit::new();
    let c1 = fake
        .commit(
            RepoKind::Base,
            "master",
            &[("config/a.yml", Some(b"x: 1\n"))],
            "1",
        )
        .unwrap();
    let body = format!("password: {IN_GIT}\n");
    let c2 = fake
        .commit(
            RepoKind::Base,
            "master",
            &[("config/a.yml", Some(body.as_bytes()))],
            "2",
        )
        .unwrap();
    let git = GitIndex::new(GitIndexDeps {
        pool: rig.pool.clone(),
        git: Arc::new(fake),
        config: RegistryConfig::default(),
        metrics: RegistryMetrics::default(),
    })
    .unwrap();
    assert!(
        !git.affected(RepoKind::Base, "master", Some(&c1), &c2)
            .await
            .unwrap()
            .is_empty()
    );
    git.git_hash(
        RepoKind::Base,
        &c2,
        &RepoPath::parse("config/a.yml").unwrap(),
        FileClass::Structured,
    )
    .unwrap();
    // The mirror cannot be read for a path outside the tree index: an error, and its line.
    let _ = git.git_hash(
        RepoKind::Tenant,
        &c2,
        &RepoPath::parse("config/a.yml").unwrap(),
        FileClass::Structured,
    );

    let text = capture.text();
    let ours: Vec<&str> = text.lines().filter(|l| l.contains(" hub_registry")).collect();
    assert!(
        ours.iter().any(|l| l.contains("settings index batch")),
        "the capture sees the index: {ours:?}"
    );
    assert!(
        ours.iter().any(|l| l.contains("git head moved")),
        "and the Git index: {ours:?}"
    );
    for secret in [FLAGGED, PLAIN, BROKEN, IN_GIT, "MIIE"] {
        assert!(!text.contains(secret), "{secret} reached the log: {text}");
    }
    for line in ours {
        for sql in ["SELECT", "INSERT", "unnest"] {
            assert!(
                !line.contains(sql),
                "SQL text reached a log line of the registry: {line}"
            );
        }
    }
}
