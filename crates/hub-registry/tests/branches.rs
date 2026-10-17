//! The branch classifier (T4, D22, D84) against what it must agree with and read: the mirror's own kinds for the same branches, and the
//! admin's patterns in `path_mappings`. Over a real Postgres 16 and the in-memory Git.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use domain::{BranchKind, RepoKind};
use hub_registry::RegistryError;
use hub_registry::branches::{BranchClass, BranchClassifier};
use hub_registry::db::Db;
use ports::GitReader;
use ports::fakes::FakeGit;
use support::pg::TestDb;

#[test]
fn the_registry_and_the_mirror_agree_on_sit_branches_templates_and_the_docs_branch() {
    let git = FakeGit::new();
    for branch in [
        "sit1",
        "sit12",
        "templates/symx-template",
        "templates/common-docs",
    ] {
        git.commit(
            RepoKind::Tenant,
            branch,
            &[("data/config/a.yml", Some(b"a: 1\n"))],
            "c",
        )
        .unwrap();
    }
    let classifier = BranchClassifier::builtin();
    let mirror = git.branches(RepoKind::Tenant).unwrap();
    assert_eq!(mirror.len(), 4);
    for b in &mirror {
        assert_eq!(
            classifier.classify(b.name.as_str()).kind(),
            b.kind,
            "{}: the registry and the mirror say different things",
            b.name
        );
    }
    // The agreement is not an accident of the names: the mirror says each of the four kinds that the registry has a rule for.
    let kinds: Vec<BranchKind> = mirror.iter().map(|b| b.kind).collect();
    for kind in [
        BranchKind::Deployable,
        BranchKind::Template,
        BranchKind::CommonDocs,
    ] {
        assert!(kinds.contains(&kind), "{kind:?}");
    }
}

#[test]
fn the_one_place_they_differ_is_a_plain_name_that_is_not_sit_digits() {
    // The mirror calls every branch without a slash deployable. The registry is the authority for the swimlane candidates (D22, D84),
    // so `main` is not one unless an admin includes it.
    let git = FakeGit::new();
    git.commit(
        RepoKind::Tenant,
        "main",
        &[("data/config/a.yml", Some(b"a: 1\n"))],
        "c",
    )
    .unwrap();
    let mirror = git.branches(RepoKind::Tenant).unwrap();
    assert_eq!(mirror[0].kind, BranchKind::Deployable);
    assert_eq!(BranchClassifier::builtin().classify("main"), BranchClass::Other);
    let admin = BranchClassifier::new(&["main".to_owned()], &[]).unwrap();
    assert_eq!(admin.classify("main"), BranchClass::Deployable);
}

async fn db() -> Option<(TestDb, Db)> {
    let test = TestDb::ready().await?;
    let pool = test.app_pool().await;
    Some((test, Db::new(pool, Duration::from_secs(10))))
}

#[tokio::test]
async fn with_no_row_the_builtin_rule_is_the_rule() {
    let Some((_t, db)) = db().await else { return };
    let c = BranchClassifier::load(&db).await.unwrap();
    assert_eq!(c.classify("sit4"), BranchClass::Deployable);
    assert_eq!(c.classify("uat1"), BranchClass::Other);
}

#[tokio::test]
async fn the_admins_patterns_of_the_tenant_repository_are_read() {
    let Some((_t, db)) = db().await else { return };
    // The base repository's row says nothing about tenant branches, whatever it holds.
    sqlx::query("INSERT INTO path_mappings (repo, repo_root, branch_include) VALUES ('base', 'config', '{everything*}')")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO path_mappings (repo, repo_root, branch_include, branch_exclude) \
         VALUES ('tenant', 'data/config', '{uat*,prod-eu}', '{sit9,uat-old*}')",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let c = BranchClassifier::load(&db).await.unwrap();
    assert_eq!(c.classify("uat3"), BranchClass::Deployable);
    assert_eq!(c.classify("prod-eu"), BranchClass::Deployable);
    assert_eq!(c.classify("sit9"), BranchClass::Other, "excluded");
    assert_eq!(c.classify("uat-old2"), BranchClass::Other, "excluded");
    assert_eq!(c.classify("sit1"), BranchClass::Deployable);
    assert_eq!(
        c.classify("everything1"),
        BranchClass::Other,
        "the base repository's patterns are not the tenant repository's"
    );
    assert_eq!(
        c.classify("templates/uat1"),
        BranchClass::Template,
        "a template is never deployable"
    );
}

#[tokio::test]
async fn a_stored_pattern_that_is_not_a_glob_is_refused_without_naming_it() {
    let Some((_t, db)) = db().await else { return };
    sqlx::query(
        "INSERT INTO path_mappings (repo, repo_root, branch_exclude) VALUES ('tenant', 'data/config', '{[}')",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let err = BranchClassifier::load(&db).await.unwrap_err();
    assert_eq!(
        err,
        RegistryError::Corrupt,
        "a bad exclude must not read as no exclude"
    );
    assert!(!err.to_string().contains('['));
}
