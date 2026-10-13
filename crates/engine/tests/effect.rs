//! T6 (D46, D84, D85, S21): what a change to a file does to what the services read, before anybody confirms it.
//!
//! The preview is a function of the layers of the logical file as they are now and of the change: it compares the effective
//! config before and after (`effective`), and names the consequences a person has to know about (the fallback to base, the
//! fork from base, a file that nobody reads, a file every service reads).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;

use bytes::Bytes;
use domain::{
    ChangeKind, FileClass, FileKind, LogicalFile, MergeMode, NfsPath, RepoPath, TenantId, TenantSet,
};
use engine::{
    EffectChange, EffectInput, EffectScope, EffectWarning, FileFacts, Flat, Layer, Layers, Limits,
    effect_preview, normalised_hash,
};
use support::yaml;

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn tenants(names: &[&str]) -> TenantSet {
    TenantSet::new(names.iter().map(|n| TenantId::parse(n).unwrap())).unwrap()
}

struct File {
    path: NfsPath,
    flat: Flat,
    hash: domain::ContentHash,
}

fn file(path: &str, text: &str) -> File {
    File {
        path: nfs(path),
        flat: yaml(text),
        hash: normalised_hash(&Bytes::copy_from_slice(text.as_bytes()), FileClass::Structured),
    }
}

impl File {
    fn layer(&self) -> Layer<'_> {
        Layer {
            path: &self.path,
            hash: self.hash,
            settings: &self.flat.settings,
        }
    }
}

fn facts(logical: &str, tenant: Option<&str>) -> FileFacts {
    FileFacts {
        kind: if tenant.is_some() {
            FileKind::Tenant
        } else {
            FileKind::Base
        },
        logical: LogicalFile::parse(logical).unwrap(),
        tenant: tenant.map(|t| TenantId::parse(t).unwrap()),
        repo_path: RepoPath::parse(&format!("config/{logical}")).unwrap(),
    }
}

const TENANT: &str = "a: 2\nb: 1\nsame: x\n";
const BASE: &str = "a: 1\nc: 3\nsame: x\n";

fn preview(
    mode: MergeMode,
    target: &FileFacts,
    deployed: &[&str],
    tenant: Option<&File>,
    base: Option<&File>,
    change: &EffectChange<'_>,
) -> engine::EffectPreview {
    let tenants = tenants(deployed);
    effect_preview(
        &EffectInput {
            mode,
            target,
            tenants: &tenants,
            layers: Layers {
                tenant: tenant.map(File::layer),
                base: base.map(File::layer),
                root: None,
            },
            target_folder_exists: true,
            change: *change,
        },
        &Limits::default(),
    )
}

fn changes(p: &engine::EffectPreview) -> Vec<(String, ChangeKind)> {
    p.changes.iter().map(|c| (c.path.to_string(), c.kind)).collect()
}

#[test]
fn delete_tenant_falls_back_to_base_with_semantic_diff() {
    let (tenant, base) = (file("svc/app-sit1.yml", TENANT), file("svc/app.yml", BASE));
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        Some(&tenant),
        Some(&base),
        &EffectChange::Delete,
    );
    assert_eq!(p.warnings, vec![EffectWarning::FallsBackToBase]);
    assert_eq!(
        changes(&p),
        vec![
            ("a".to_owned(), ChangeKind::Changed),
            ("b".to_owned(), ChangeKind::Removed),
            ("c".to_owned(), ChangeKind::Added),
        ]
    );
    // The change reads from what the service gets now to what it would get: a was 2 and becomes 1.
    let a = &p.changes[0];
    assert_eq!(
        (
            a.left.as_ref().unwrap().text.as_str(),
            a.right.as_ref().unwrap().text.as_str()
        ),
        ("2", "1")
    );
    assert_eq!(p.changes_total, 3);
    assert!(p.effective_changed);
}

#[test]
fn delete_tenant_file_when_there_is_no_base_leaves_nothing() {
    let tenant = file("svc/app-sit1.yml", TENANT);
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        Some(&tenant),
        None,
        &EffectChange::Delete,
    );
    assert_eq!(p.warnings, vec![EffectWarning::NoFileLeft]);
    assert_eq!(p.changes.len(), 3);
    assert!(p.changes.iter().all(|c| c.kind == ChangeKind::Removed));
    assert!(p.effective_changed);
}

#[test]
fn delete_tenant_file_with_the_same_settings_as_base_changes_no_setting() {
    let tenant = file("svc/app-sit1.yml", "same: x\na: 1\nc: 3\n");
    let base = file("svc/app.yml", BASE);
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        Some(&tenant),
        Some(&base),
        &EffectChange::Delete,
    );
    assert!(p.changes.is_empty(), "{:?}", changes(&p));
    assert_eq!(p.warnings, vec![EffectWarning::FallsBackToBase]);
}

#[test]
fn delete_tenant_file_of_a_property_source_only_takes_back_what_it_overrode() {
    let tenant = file("svc/app-sit1.yml", "a: 2\nonly_tenant: 5\n");
    let base = file("svc/app.yml", "a: 1\nb: 1\n");
    let p = preview(
        MergeMode::SpringMerge,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        Some(&tenant),
        Some(&base),
        &EffectChange::Delete,
    );
    assert_eq!(
        changes(&p),
        vec![
            ("a".to_owned(), ChangeKind::Changed),
            ("only_tenant".to_owned(), ChangeKind::Removed)
        ]
    );
    // A property source also needs a notification or a restart before the services see the change (D85).
    assert_eq!(
        p.warnings,
        vec![
            EffectWarning::FallsBackToBase,
            EffectWarning::NeedsNotifyOrRestart
        ]
    );
}

#[test]
fn upload_new_tenant_warns_fork() {
    let base = file("svc/app.yml", BASE);
    let new = file("svc/app-sit1.yml", TENANT);
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        None,
        Some(&base),
        &EffectChange::Write(new.layer()),
    );
    assert_eq!(p.warnings, vec![EffectWarning::ForksFromBase]);
    assert_eq!(
        changes(&p),
        vec![
            ("a".to_owned(), ChangeKind::Changed),
            ("b".to_owned(), ChangeKind::Added),
            ("c".to_owned(), ChangeKind::Removed),
        ]
    );
}

#[test]
fn upload_over_an_existing_tenant_file_is_not_a_new_fork() {
    let (tenant, base) = (file("svc/app-sit1.yml", TENANT), file("svc/app.yml", BASE));
    let new = file("svc/app-sit1.yml", "a: 9\n");
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        Some(&tenant),
        Some(&base),
        &EffectChange::Write(new.layer()),
    );
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    assert!(!p.changes.is_empty());
}

#[test]
fn upload_new_tenant_file_of_a_property_source_merges_and_does_not_fork() {
    let base = file("svc/app.yml", BASE);
    let new = file("svc/app-sit1.yml", "a: 9\n");
    let p = preview(
        MergeMode::SpringMerge,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        None,
        Some(&base),
        &EffectChange::Write(new.layer()),
    );
    // It does not fork (the file merges), but a property source needs a notification or a restart (D85).
    assert_eq!(p.warnings, vec![EffectWarning::NeedsNotifyOrRestart]);
    assert_eq!(changes(&p), vec![("a".to_owned(), ChangeKind::Changed)]);
}

#[test]
fn upload_new_tenant_file_with_no_base_has_nothing_to_fork_from() {
    let new = file("svc/app-sit1.yml", TENANT);
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        None,
        None,
        &EffectChange::Write(new.layer()),
    );
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    assert!(p.changes.iter().all(|c| c.kind == ChangeKind::Added));
}

#[test]
fn wrong_suffix_never_read() {
    let (tenant, base) = (file("svc/app-sit9.yml", TENANT), file("svc/app.yml", BASE));
    let target = facts("svc/app.yml", Some("sit9"));
    let new = file("svc/app-sit9.yml", "z: 1\n");
    for change in [EffectChange::Delete, EffectChange::Write(new.layer())] {
        let p = preview(
            MergeMode::WholeFile,
            &target,
            &["sit1", "sit2"],
            Some(&tenant),
            Some(&base),
            &change,
        );
        assert_eq!(
            p.warnings,
            vec![EffectWarning::NeverRead {
                suffix: TenantId::parse("sit9").unwrap()
            }]
        );
        assert!(p.changes.is_empty());
        assert_eq!(p.changes_total, 0);
        assert!(
            !p.effective_changed,
            "no service reads the file, so nothing changes for any"
        );
    }
}

#[test]
fn a_new_file_with_a_wrong_suffix_is_not_also_called_a_fork() {
    let base = file("svc/app.yml", BASE);
    let new = file("svc/app-sit9.yml", TENANT);
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit9")),
        &["sit1"],
        None,
        Some(&base),
        &EffectChange::Write(new.layer()),
    );
    assert_eq!(p.warnings.len(), 1);
    assert!(matches!(p.warnings[0], EffectWarning::NeverRead { .. }));
}

#[test]
fn root_file_affects_every_service() {
    let base = file("application.yml", "a: 1\n");
    let new = file("application.yml", "a: 2\n");
    let p = preview(
        MergeMode::SpringMerge,
        &facts("application.yml", None),
        &["sit1"],
        None,
        Some(&base),
        &EffectChange::Write(new.layer()),
    );
    assert_eq!(p.scope, EffectScope::EveryService);
    assert_eq!(changes(&p), vec![("a".to_owned(), ChangeKind::Changed)]);
}

#[test]
fn root_tenant_file_affects_every_service_too() {
    let p = preview(
        MergeMode::SpringMerge,
        &facts("application.yml", Some("sit1")),
        &["sit1"],
        None,
        None,
        &EffectChange::Delete,
    );
    assert_eq!(p.scope, EffectScope::EveryService);
}

#[test]
fn a_file_in_a_folder_affects_that_application() {
    let p = preview(
        MergeMode::WholeFile,
        &facts("tx-infinity-api/a.yml", None),
        &["sit1"],
        None,
        None,
        &EffectChange::Delete,
    );
    assert_eq!(
        p.scope,
        EffectScope::Application(domain::AppName::parse("tx-infinity-api").unwrap())
    );
    let p = preview(
        MergeMode::WholeFile,
        &facts("document-service/resources/receipt.bmp", None),
        &["sit1"],
        None,
        None,
        &EffectChange::Delete,
    );
    assert_eq!(
        p.scope,
        EffectScope::Application(domain::AppName::parse("document-service").unwrap())
    );
}

#[test]
fn a_folder_that_is_not_an_application_name_is_named_as_a_folder() {
    let p = preview(
        MergeMode::WholeFile,
        &facts("My Folder/a.yml", None),
        &["sit1"],
        None,
        None,
        &EffectChange::Delete,
    );
    assert_eq!(p.scope, EffectScope::Folder(nfs("My Folder")));
}

#[test]
fn the_listed_changes_are_capped_and_all_of_them_counted() {
    let mut text = String::new();
    for i in 0..120 {
        writeln!(text, "k{i:03}: {i}").unwrap();
    }
    let tenant = file("svc/app-sit1.yml", &text);
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        Some(&tenant),
        None,
        &EffectChange::Delete,
    );
    assert_eq!((p.changes.len(), p.changes_total), (50, 120));
    // The first 50 by path, so that the list does not depend on anything but the input.
    assert_eq!(p.changes[0].path.as_str(), "k000");
    assert_eq!(p.changes[49].path.as_str(), "k049");
}

#[test]
fn a_text_file_changes_by_hash_alone() {
    let hash = |data: &[u8]| normalised_hash(&Bytes::copy_from_slice(data), FileClass::Text);
    let (old, new) = (nfs("doc/t-sit1.xsl"), nfs("doc/t-sit1.xsl"));
    let tenants = tenants(&["sit1"]);
    let target = facts("doc/t.xsl", Some("sit1"));
    let run = |from: &[u8], to: &[u8]| {
        effect_preview(
            &EffectInput {
                mode: MergeMode::WholeFile,
                target: &target,
                tenants: &tenants,
                layers: Layers {
                    tenant: Some(Layer {
                        path: &old,
                        hash: hash(from),
                        settings: &[],
                    }),
                    base: None,
                    root: None,
                },
                target_folder_exists: true,
                change: EffectChange::Write(Layer {
                    path: &new,
                    hash: hash(to),
                    settings: &[],
                }),
            },
            &Limits::default(),
        )
    };
    let changed = run(b"<a/>", b"<b/>");
    assert!(changed.effective_changed && changed.changes.is_empty());
    // A change of line endings alone is not a change.
    let same = run(b"<a>\n</a>", b"<a>\r\n</a>");
    assert!(!same.effective_changed);
}

#[test]
fn debug_never_prints_a_value() {
    let tenant = file("svc/app-sit1.yml", "password: S3CR3T-VALUE\n");
    let p = preview(
        MergeMode::WholeFile,
        &facts("svc/app.yml", Some("sit1")),
        &["sit1"],
        Some(&tenant),
        None,
        &EffectChange::Delete,
    );
    assert_eq!(p.changes.len(), 1);
    assert!(!format!("{p:?}").contains("S3CR3T"), "{p:?}");
}

#[test]
fn the_same_input_gives_the_same_preview() {
    let (tenant, base) = (file("svc/app-sit1.yml", TENANT), file("svc/app.yml", BASE));
    let run = || {
        let p = preview(
            MergeMode::WholeFile,
            &facts("svc/app.yml", Some("sit1")),
            &["sit1"],
            Some(&tenant),
            Some(&base),
            &EffectChange::Delete,
        );
        (
            p.scope.clone(),
            p.warnings.clone(),
            changes(&p),
            p.changes_total,
            p.effective_changed,
        )
    };
    assert_eq!(run(), run());
}

// --- T10: what a change needs before the services see it (D85) ---

fn preview_in(
    mode: MergeMode,
    target: &FileFacts,
    folder_exists: bool,
    layers: Layers<'_>,
    change: &EffectChange<'_>,
) -> engine::EffectPreview {
    let tenants = tenants(&["sit1"]);
    effect_preview(
        &EffectInput {
            mode,
            target,
            tenants: &tenants,
            layers,
            target_folder_exists: folder_exists,
            change: *change,
        },
        &Limits::default(),
    )
}

#[test]
fn property_source_change_needs_notify_or_restart() {
    let (base, new) = (file("svc/app.yml", "a: 1\n"), file("svc/app.yml", "a: 2\n"));
    let layers = Layers {
        tenant: None,
        base: Some(base.layer()),
        root: None,
    };
    let p = preview_in(
        MergeMode::SpringMerge,
        &facts("svc/app.yml", None),
        true,
        layers,
        &EffectChange::Write(new.layer()),
    );
    assert!(
        p.warnings.contains(&EffectWarning::NeedsNotifyOrRestart),
        "{:?}",
        p.warnings
    );
    assert!(
        !p.warnings
            .contains(&EffectWarning::NewFolderNeedsConfigServerRestart)
    );
    // A write that changes nothing the services read warns about nothing.
    let same = preview_in(
        MergeMode::SpringMerge,
        &facts("svc/app.yml", None),
        true,
        layers,
        &EffectChange::Write(base.layer()),
    );
    assert!(same.warnings.is_empty(), "{:?}", same.warnings);
    // A resource file (whole file) is live within the cache lifetime: no property-source warning.
    let resource = preview_in(
        MergeMode::WholeFile,
        &facts("svc/app.yml", None),
        true,
        layers,
        &EffectChange::Write(new.layer()),
    );
    assert!(!resource.warnings.contains(&EffectWarning::NeedsNotifyOrRestart));
    // Deleting a property source is a change too.
    let deleted = preview_in(
        MergeMode::SpringMerge,
        &facts("svc/app.yml", None),
        true,
        layers,
        &EffectChange::Delete,
    );
    assert!(deleted.warnings.contains(&EffectWarning::NeedsNotifyOrRestart));
}

#[test]
fn new_folder_warns_that_the_config_server_must_restart() {
    let new = file("fresh/x.xsl", "a: 1\n");
    let layers = Layers {
        tenant: None,
        base: None,
        root: None,
    };
    let p = preview_in(
        MergeMode::WholeFile,
        &facts("fresh/x.xsl", None),
        false,
        layers,
        &EffectChange::Write(new.layer()),
    );
    assert_eq!(p.warnings, vec![EffectWarning::NewFolderNeedsConfigServerRestart]);
    // The same write into a folder that exists does not.
    let there = preview_in(
        MergeMode::WholeFile,
        &facts("fresh/x.xsl", None),
        true,
        layers,
        &EffectChange::Write(new.layer()),
    );
    assert!(there.warnings.is_empty());
    // A delete never creates a folder.
    let gone = preview_in(
        MergeMode::WholeFile,
        &facts("fresh/x.xsl", None),
        false,
        layers,
        &EffectChange::Delete,
    );
    assert!(
        !gone
            .warnings
            .contains(&EffectWarning::NewFolderNeedsConfigServerRestart)
    );
}

#[test]
fn channels_yml_change_warns_that_the_config_server_must_restart() {
    let (old, new) = (
        file("channels.yml", "- atm\n"),
        file("channels.yml", "- atm\n- web\n"),
    );
    let layers = Layers {
        tenant: None,
        base: Some(old.layer()),
        root: None,
    };
    let p = preview_in(
        MergeMode::WholeFile,
        &facts("channels.yml", None),
        true,
        layers,
        &EffectChange::Write(new.layer()),
    );
    assert_eq!(
        p.warnings,
        vec![EffectWarning::ChannelsYmlNeedsConfigServerRestart]
    );
    let unchanged = preview_in(
        MergeMode::WholeFile,
        &facts("channels.yml", None),
        true,
        layers,
        &EffectChange::Write(old.layer()),
    );
    assert!(unchanged.warnings.is_empty());
    // Deleting the list changes which folders are channels.
    let deleted = preview_in(
        MergeMode::WholeFile,
        &facts("channels.yml", None),
        true,
        layers,
        &EffectChange::Delete,
    );
    assert!(
        deleted
            .warnings
            .contains(&EffectWarning::ChannelsYmlNeedsConfigServerRestart)
    );
    // The list the config-server reads is the base file. A tenant's copy of it is a file that tenant's services are given, and
    // the server does not read it when it starts.
    let (tenant_old, tenant_new) = (
        file("channels-sit1.yml", "- atm\n"),
        file("channels-sit1.yml", "- atm\n- web\n"),
    );
    let copy = preview_in(
        MergeMode::WholeFile,
        &facts("channels.yml", Some("sit1")),
        true,
        Layers {
            tenant: Some(tenant_old.layer()),
            base: Some(old.layer()),
            root: None,
        },
        &EffectChange::Write(tenant_new.layer()),
    );
    assert!(copy.effective_changed);
    assert!(
        !copy
            .warnings
            .contains(&EffectWarning::ChannelsYmlNeedsConfigServerRestart),
        "{:?}",
        copy.warnings
    );
    // A file of another name in any folder is not the list.
    let other = preview_in(
        MergeMode::WholeFile,
        &facts("svc/channels.yaml", None),
        true,
        layers,
        &EffectChange::Write(new.layer()),
    );
    assert!(
        !other
            .warnings
            .contains(&EffectWarning::ChannelsYmlNeedsConfigServerRestart)
    );
}
