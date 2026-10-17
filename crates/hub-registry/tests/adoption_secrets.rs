//! The secret scan of the adoption pass (T5, S13, S10, Q20): where a secret may be in the base repository, the chosen tenant branches and
//! on NFS, stored as file, line, length and rule, and never the value.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::fmt::Write as _;

use hub_registry::{AdoptionStep, SecretHit};
use support::adoption::{everything_stored, scanned, secret_world, token};

fn hit(file: &str, line: u32, length: u32, rule: &str) -> SecretHit {
    SecretHit {
        file: file.to_owned(),
        line,
        length,
        rule: rule.to_owned(),
    }
}

#[tokio::test]
async fn secret_scan_covers_both_repos_and_the_nfs_content() {
    let Some((r, _planted)) = secret_world().await else {
        return;
    };
    let view = scanned(&r, &["sit1"]).await;
    assert_eq!(
        view.step,
        AdoptionStep::Confirm,
        "the scan was the last step before the confirmation"
    );
    let len = u32::try_from(token(1).len()).unwrap();
    assert_eq!(
        view.secret_scan,
        vec![
            // NFS: the file it shares with the base repository, and the one only it has, and the tenant file.
            hit("app/local.yml", 1, len, "github_token"),
            hit("app/shared.yml", 1, len, "github_token"),
            hit("app/t-sit1.yml", 1, len, "github_token"),
            // NFS: the file the scan could not check is listed, and so is not taken for clean.
            hit("app/weird.yml", 0, 0, "unscanned"),
            // The base repository's head: the file NFS does not have in that form, and the shared one under its repository path.
            hit("config/app/base.yml", 2, len, "github_token"),
            hit("config/app/shared.yml", 1, len, "github_token"),
            // The chosen tenant branch.
            hit("data/config/app/t.yml", 1, len, "github_token"),
        ],
        "NFS, the base head and the chosen branch; nothing from the denied key, nothing from the branch that was not chosen"
    );
    // The scan can be run again, and says the same.
    let again = r.svc.scan_secrets(&r.admin, view.id).await.unwrap();
    assert_eq!(again.secret_scan, view.secret_scan);
    // A pass with another branch scans that one instead.
    let other = {
        let id = r.start().await.id;
        r.choose(id, &["sit2"]).await;
        r.resolve_all(
            id,
            hub_registry::Bulk::default(),
            hub_registry::Resolution::GitIsRight,
        )
        .await;
        r.svc.scan_secrets(&r.admin, id).await.unwrap()
    };
    assert!(
        other
            .secret_scan
            .iter()
            .any(|h| h.file == "data/config/app/other.yml")
    );
    assert!(
        !other
            .secret_scan
            .iter()
            .any(|h| h.file == "data/config/app/t.yml")
    );
    assert_eq!(
        r.w.rig.metrics.adoption_secret_hits_count(),
        (view.secret_scan.len() * 2 + other.secret_scan.len()) as u64
    );
}

#[tokio::test]
async fn a_git_file_over_the_size_limit_is_not_read_and_is_listed_as_unscanned() {
    let Some((r, _planted)) = secret_world().await else {
        return;
    };
    let view = scanned(&r, &["sit1"]).await;
    let len = u32::try_from(token(1).len()).unwrap();
    assert!(
        view.secret_scan
            .contains(&hit("config/app/base.yml", 2, len, "github_token")),
        "with the default limit the base file is read and its secret found"
    );

    // A limit between the size of the Git-only base file (59 bytes) and the others (49 bytes and less): the NFS contents come from the
    // blob store and Git files that are small enough are read as before, and the base file is not read at all.
    let small = hub_registry::RegistryConfig {
        max_file_bytes: 55,
        ..hub_registry::RegistryConfig::default()
    };
    let svc = support::adoption::AdoptionRig::service_of(&r.w, small);
    let again = svc.scan_secrets(&r.admin, view.id).await.unwrap();
    let expected: Vec<SecretHit> = view
        .secret_scan
        .iter()
        .map(|h| {
            if h.file == "config/app/base.yml" {
                hit("config/app/base.yml", 0, 0, "unscanned")
            } else {
                h.clone()
            }
        })
        .collect();
    assert_eq!(again.secret_scan, expected);
}

#[tokio::test]
async fn the_scan_result_holds_locations_and_lengths_but_no_value() {
    let Some((r, planted)) = secret_world().await else {
        return;
    };
    let view = scanned(&r, &["sit1"]).await;
    assert!(!view.secret_scan.is_empty());

    // What is stored is a list of objects with exactly four keys.
    let stored: serde_json::Value = sqlx::query_scalar("SELECT secret_scan FROM adoptions WHERE id = $1")
        .bind(view.id)
        .fetch_one(&r.w.rig.pool)
        .await
        .unwrap();
    for entry in stored.as_array().unwrap() {
        let mut keys: Vec<&str> = entry.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["file", "length", "line", "rule"], "{entry}");
    }

    // Walk on to the confirmation, which writes the audit events and the outbox, and look at everything that was stored, returned or
    // counted for a value.
    let outcome = r.svc.confirm(&r.admin, view.id).await.unwrap();
    let mut seen = everything_stored(&r.w.rig.pool).await;
    write!(seen, "{view:?}\n{outcome:?}\n").unwrap();
    let mut metrics = String::new();
    let mut registry = prometheus_client::registry::Registry::default();
    r.w.rig.metrics.register(&mut registry);
    prometheus_client::encoding::text::encode(&mut metrics, &registry).unwrap();
    seen.push_str(&metrics);
    for value in planted.all() {
        assert!(
            !seen.contains(value),
            "a planted secret reached what is stored, returned or counted"
        );
        // Not even the part after the prefix, which is the secret itself.
        assert!(
            !seen.contains(&value[4..]),
            "the body of a planted secret reached what is stored"
        );
    }
    // The errors of the service carry no input either.
    let error = r.svc.confirm(&r.admin, view.id).await.unwrap_err();
    assert!(!format!("{error} {error:?}").contains("ghp_"));
}
