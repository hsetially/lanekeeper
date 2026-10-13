//! T10 (D83, D87): the three checks that come with the config-server's resolution model.
//!
//! - C9 `ambiguous_file_name`: one name in several folders that the config-server searches;
//! - C10 `unresolved_placeholder`: a `${KEY}` that no property view and no environment variable name can fill;
//! - C11 `config_server_restart_required`: what the config-server has not seen because it started before it was there.
//!
//! The synthetic fixtures plant the same shapes as the real base repository (3 ambiguous names at small scale, 13 at target),
//! so the manifest is the oracle for C9, as it is for C1 to C8 in `checks_fixture.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use domain::{
    AppName, ChannelName, ContentHash, FileClass, FileKind, FindingKind, LogicalFile, MergeMode, NfsPath,
    RepoPath, TenantId, Timestamp,
};
use engine::{
    ChannelsYml, CheckDetail, CheckFile, ConfigServerFacts, EnvNames, FileFacts, FolderSeen, Limits,
    PlaceholderFile, PropertyFile, PropertyView, RestartReason, Syntax, TenantView, c9_ambiguous_file_names,
    c10_unresolved_placeholders, c11_config_server_restart_required, flatten, parse, property_view,
    search_locations,
};
use support::lanes::load_lanes;
use support::small_fixtures;

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn channel(s: &str) -> ChannelName {
    ChannelName::parse(s).unwrap()
}

fn tenant(s: &str) -> TenantId {
    TenantId::parse(s).unwrap()
}

fn base_facts(path: &str) -> FileFacts {
    FileFacts {
        kind: FileKind::Base,
        logical: LogicalFile::parse(path).unwrap(),
        tenant: None,
        repo_path: RepoPath::parse(&format!("config/{path}")).unwrap(),
    }
}

fn tenant_facts(logical: &str, t: &str) -> FileFacts {
    FileFacts {
        kind: FileKind::Tenant,
        logical: LogicalFile::parse(logical).unwrap(),
        tenant: Some(tenant(t)),
        repo_path: RepoPath::parse(&format!("data/config/{logical}")).unwrap(),
    }
}

/// `CheckFile`s for C9, which reads the logical file and nothing else.
fn check_files<'a>(paths: &'a [NfsPath], facts: &'a [FileFacts]) -> Vec<CheckFile<'a>> {
    paths
        .iter()
        .zip(facts)
        .map(|(path, facts)| CheckFile {
            path,
            facts,
            class: FileClass::Text,
            hash: ContentHash::from_bytes([0; 32]),
            merge_mode: MergeMode::WholeFile,
            structured: None,
            root_settings: None,
            drift: None,
            overwrite: None,
            base_history: None,
        })
        .collect()
}

// --- C9 -------------------------------------------------------------------------------------------------------------------

fn c9(list: &[&str], channels: &[&str]) -> Vec<engine::CheckResult> {
    let paths: Vec<NfsPath> = list.iter().map(|p| nfs(p)).collect();
    let facts: Vec<FileFacts> = list.iter().map(|p| base_facts(p)).collect();
    let channels: Vec<ChannelName> = channels.iter().map(|c| channel(c)).collect();
    c9_ambiguous_file_names(&check_files(&paths, &facts), &channels, &Limits::default())
}

fn ambiguous(r: &engine::CheckResult) -> (&str, Vec<&str>, usize) {
    let CheckDetail::AmbiguousFileName { name, folders, total } = &r.detail else {
        panic!("not a C9 result: {r:?}");
    };
    (
        name.as_str(),
        folders.iter().map(NfsPath::as_str).collect(),
        *total,
    )
}

#[test]
fn checks_c9_same_name_in_two_search_folders() {
    let results = c9(
        &[
            "core/a/authentication-config.yaml",
            "core/b/authentication-config.yaml",
            "core/c/authentication-config.yaml",
            "core/a/unique.yaml",
            "root.yml",
        ],
        &[],
    );
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.kind, FindingKind::AmbiguousFileName);
    assert_eq!(
        ambiguous(r),
        (
            "authentication-config.yaml",
            vec!["core/a", "core/b", "core/c"],
            3
        )
    );
    assert_eq!(
        r.path.as_ref().unwrap().as_str(),
        "core/a/authentication-config.yaml"
    );
    assert!(r.message.as_str().contains("authentication-config.yaml") && r.message.as_str().contains('3'));
    assert!(r.locations.is_empty());
}

#[test]
fn checks_c9_channel_folders_do_not_count() {
    // The channel copy of a file is how a channel overrides it, not an ambiguity.
    let list = [
        "ui/remote-itm-teller/layout.yml",
        "ui/layout.yml",
        "holds/atm-iso/h.yml",
        "holds/atm-iso/deeper/h.yml",
        "holds/h.yml",
    ];
    assert!(c9(&list, &["remote-itm-teller", "atm-iso"]).is_empty());
    // Without the channel list the same tree is ambiguous twice.
    assert_eq!(c9(&list, &[]).len(), 2);
}

#[test]
fn checks_c9_a_name_in_the_root_and_one_folder_is_ambiguous() {
    let results = c9(&["x.yml", "svc/x.yml"], &[]);
    assert_eq!(results.len(), 1);
    assert_eq!(ambiguous(&results[0]), ("x.yml", vec!["", "svc"], 2));
}

#[test]
fn checks_c9_tenant_copies_are_the_same_name_as_their_base() {
    // `a/x-sit1.yml` and `b/x.yml` are two files called `x.yml` as far as a request for a tenant goes.
    let paths = [nfs("a/x-sit1.yml"), nfs("b/x.yml"), nfs("a/solo-sit1.yml")];
    let facts = [
        tenant_facts("a/x.yml", "sit1"),
        base_facts("b/x.yml"),
        tenant_facts("a/solo.yml", "sit1"),
    ];
    let results = c9_ambiguous_file_names(&check_files(&paths, &facts), &[], &Limits::default());
    assert_eq!(results.len(), 1);
    assert_eq!(ambiguous(&results[0]), ("x.yml", vec!["a", "b"], 2));
    // A folder that has the name twice (base and tenant copy) is one folder.
    let paths = [nfs("a/x-sit1.yml"), nfs("a/x.yml")];
    let facts = [tenant_facts("a/x.yml", "sit1"), base_facts("a/x.yml")];
    assert!(c9_ambiguous_file_names(&check_files(&paths, &facts), &[], &Limits::default()).is_empty());
}

#[test]
fn checks_c9_lists_are_capped_and_counted() {
    let list: Vec<String> = (0..80).map(|i| format!("f{i:02}/dup.yml")).collect();
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    let results = c9(&refs, &[]);
    let (_, folders, total) = ambiguous(&results[0]);
    assert_eq!(
        (folders.len(), total),
        (Limits::default().locations_per_finding, 80)
    );
    assert_eq!(folders[0], "f00");
}

#[test]
fn checks_c9_the_path_is_a_file_that_is_there_and_not_the_name_of_a_copy_that_is_not() {
    // The first folder has only a tenant copy: `a/x.yml` is not a file, `a/x-sit1.yml` is.
    let paths = [nfs("a/x-sit1.yml"), nfs("b/x.yml")];
    let facts = [tenant_facts("a/x.yml", "sit1"), base_facts("b/x.yml")];
    let results = c9_ambiguous_file_names(&check_files(&paths, &facts), &[], &Limits::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].path.as_ref().unwrap().as_str(), "a/x-sit1.yml");
    assert_eq!(ambiguous(&results[0]), ("x.yml", vec!["a", "b"], 2));
    // The first copy in path order, in whatever order the files are given, and the root's file has no folder in its path.
    let paths = [nfs("x.yml"), nfs("x-sit1.yml"), nfs("b/x.yml")];
    let facts = [
        base_facts("x.yml"),
        tenant_facts("x.yml", "sit1"),
        base_facts("b/x.yml"),
    ];
    let forward = c9_ambiguous_file_names(&check_files(&paths, &facts), &[], &Limits::default());
    assert_eq!(forward[0].path.as_ref().unwrap().as_str(), "x-sit1.yml");
    let (paths, facts): (Vec<_>, Vec<_>) = paths.into_iter().zip(facts).rev().unzip();
    assert_eq!(
        c9_ambiguous_file_names(&check_files(&paths, &facts), &[], &Limits::default()),
        forward
    );
}

#[test]
fn checks_c9_a_cap_of_zero_lists_nothing_and_still_reports_the_name() {
    let cap = Limits {
        locations_per_finding: 0,
        ..Limits::default()
    };
    let paths = [nfs("a/x.yml"), nfs("b/x.yml")];
    let facts = [base_facts("a/x.yml"), base_facts("b/x.yml")];
    let results = c9_ambiguous_file_names(&check_files(&paths, &facts), &[], &cap);
    assert_eq!(
        results.len(),
        1,
        "an ambiguity is not hidden by the length of a list"
    );
    assert_eq!(ambiguous(&results[0]), ("x.yml", Vec::new(), 2));
    assert_eq!(results[0].path.as_ref().unwrap().as_str(), "a/x.yml");
}

#[test]
fn checks_c9_results_do_not_depend_on_the_input_order() {
    let mut list = vec!["b/x.yml", "a/x.yml", "c/y.yml", "d/y.yml", "d/z.yml"];
    let forward = c9(&list, &[]);
    list.reverse();
    assert_eq!(c9(&list, &[]), forward);
    assert!(forward.windows(2).all(|w| w[0].message <= w[1].message));
}

#[test]
fn checks_c9_match_manifest_oracle() {
    let fx = small_fixtures();
    let lanes = load_lanes(&fx.root, usize::MAX);
    let channels: Vec<ChannelName> = fx.manifest.channels.iter().map(|c| channel(c)).collect();
    let planted: BTreeMap<String, BTreeSet<String>> = fx
        .manifest
        .planted
        .duplicate_names
        .iter()
        .map(|d| (d.name.clone(), d.folders.iter().cloned().collect()))
        .collect();
    assert!(
        planted.len() >= 3,
        "the fixtures plant ambiguous names: {planted:?}"
    );
    for lane in &lanes {
        let views: Vec<CheckFile<'_>> = lane
            .files
            .iter()
            .map(|f| CheckFile {
                path: &f.path,
                facts: &f.facts,
                class: f.class,
                hash: ContentHash::from_bytes([0; 32]),
                merge_mode: MergeMode::WholeFile,
                structured: None,
                root_settings: None,
                drift: None,
                overwrite: None,
                base_history: None,
            })
            .collect();
        let found: BTreeMap<String, BTreeSet<String>> =
            c9_ambiguous_file_names(&views, &channels, &Limits::default())
                .iter()
                .map(|r| {
                    let (name, folders, total) = ambiguous(r);
                    assert_eq!(total, folders.len(), "small scale: nothing is cut");
                    (name.to_owned(), folders.into_iter().map(str::to_owned).collect())
                })
                .collect();
        assert_eq!(found, planted, "swimlane {}", lane.id);
    }
}

/// The same at target scale, where the generator plants the real base's 13 ambiguous names (one in 12 folders, one in 4):
/// `cargo test -p engine --test checks_c9_c10_c11 -- --ignored`.
#[test]
#[ignore = "target scale: reads the target fixtures (generated when missing)"]
fn checks_c9_report_thirteen_ambiguous_names_at_target_scale() {
    let base = std::env::var_os("LK_FIXTURES_DIR").map_or_else(
        || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/target"),
        std::path::PathBuf::from,
    );
    if !base.join("manifest.json").is_file() {
        xtask::fixtures::generate(&xtask::fixtures::Config {
            scale: xtask::fixtures::Scale::Target,
            seed: xtask::fixtures::DEFAULT_SEED,
            out: base.clone(),
        })
        .unwrap();
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(base.join("manifest.json")).unwrap()).unwrap();
    let channels: Vec<ChannelName> = manifest["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| channel(c.as_str().unwrap()))
        .collect();
    let lanes = load_lanes(&base, 1);
    let views: Vec<CheckFile<'_>> = lanes[0]
        .files
        .iter()
        .map(|f| CheckFile {
            path: &f.path,
            facts: &f.facts,
            class: f.class,
            hash: ContentHash::from_bytes([0; 32]),
            merge_mode: MergeMode::WholeFile,
            structured: None,
            root_settings: None,
            drift: None,
            overwrite: None,
            base_history: None,
        })
        .collect();
    let found: BTreeMap<String, usize> = c9_ambiguous_file_names(&views, &channels, &Limits::default())
        .iter()
        .map(|r| {
            let (name, _, total) = ambiguous(r);
            (name.to_owned(), total)
        })
        .collect();
    assert_eq!(found.len(), 13, "{found:?}");
    assert_eq!(found["authentication-config.yaml"], 12);
    assert_eq!(found["error-codes-config.yaml"], 4);
}

// --- C10 ------------------------------------------------------------------------------------------------------------------

fn view_of(files: &[(&str, &str)], tenant_id: Option<&str>) -> PropertyView {
    let parsed: Vec<(NfsPath, engine::Flat)> = files
        .iter()
        .map(|(path, text)| {
            let path = nfs(path);
            let syntax = Syntax::of_path(&path).unwrap();
            let flat = flatten(
                &parse(text, syntax, &Limits::default()).unwrap(),
                &Limits::default(),
            )
            .unwrap();
            (path, flat)
        })
        .collect();
    let paths: Vec<NfsPath> = parsed.iter().map(|(p, _)| p.clone()).collect();
    let app = AppName::parse("holds").unwrap();
    let tenant_id = tenant_id.map(tenant);
    let locations = search_locations(&app, tenant_id.as_ref(), &paths, &[]);
    let property_files: Vec<PropertyFile<'_>> = parsed
        .iter()
        .map(|(path, flat)| PropertyFile {
            path,
            settings: &flat.settings,
        })
        .collect();
    property_view(
        &app,
        tenant_id.as_ref(),
        &locations,
        &property_files,
        &Limits::default(),
    )
    .unwrap()
}

fn keys(r: &engine::CheckResult) -> Vec<(String, Vec<String>, Vec<u32>)> {
    let CheckDetail::UnresolvedPlaceholders { keys, .. } = &r.detail else {
        panic!("not a C10 result: {r:?}");
    };
    keys.iter()
        .map(|k| {
            (
                k.key.clone(),
                k.tenants.iter().map(ToString::to_string).collect(),
                k.lines.clone(),
            )
        })
        .collect()
}

#[test]
fn checks_c9_c10_c11_c10_flags_a_key_in_no_view_and_no_environment() {
    let sit1 = tenant("sit1");
    let view = view_of(&[("application.properties", "A = 1\n")], Some("sit1"));
    let path = nfs("svc/a.yml");
    let text = "a: ${A}\nb: ${MISSING}\nc: ${FROM_ENV}\nd: ${MISSING}\ne: ${spring.thing}\n";
    let env = EnvNames::new(["FROM_ENV", "OTHER"]);
    let file = PlaceholderFile {
        path: &path,
        text,
        settings: None,
    };
    let results = c10_unresolved_placeholders(
        &[file],
        &[TenantView {
            tenant: &sit1,
            view: &view,
        }],
        &env,
        &Limits::default(),
    );
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.kind, FindingKind::UnresolvedPlaceholder);
    assert_eq!(r.path.as_ref().unwrap(), &path);
    assert_eq!(
        keys(r),
        vec![
            ("MISSING".to_owned(), vec!["sit1".to_owned()], vec![2, 4]),
            ("spring.thing".to_owned(), vec!["sit1".to_owned()], vec![5]),
        ]
    );
    assert!(r.message.as_str().contains("MISSING"));
}

#[test]
fn checks_c9_c10_c11_c10_a_key_is_resolved_for_the_tenants_whose_view_has_it() {
    let (sit1, sit2) = (tenant("sit1"), tenant("sit2"));
    let v1 = view_of(&[("application.properties", "X = 1\n")], Some("sit1"));
    let v2 = view_of(&[("application.properties", "Y = 1\n")], Some("sit2"));
    let path = nfs("a.yml");
    let file = PlaceholderFile {
        path: &path,
        text: "${X} ${Y}\n",
        settings: None,
    };
    let views = [
        TenantView {
            tenant: &sit1,
            view: &v1,
        },
        TenantView {
            tenant: &sit2,
            view: &v2,
        },
    ];
    let results = c10_unresolved_placeholders(&[file], &views, &EnvNames::new([]), &Limits::default());
    assert_eq!(
        keys(&results[0]),
        vec![
            ("X".to_owned(), vec!["sit2".to_owned()], vec![1]),
            ("Y".to_owned(), vec!["sit1".to_owned()], vec![1]),
        ]
    );
    // An environment variable of a consumer fills it for every tenant.
    let env = EnvNames::new(["X", "Y"]);
    assert!(c10_unresolved_placeholders(&[file], &views, &env, &Limits::default()).is_empty());
}

#[test]
fn checks_c9_c10_c11_c10_the_tenants_of_a_finding_do_not_depend_on_the_order_of_the_views() {
    let (sit1, sit2, sit3) = (tenant("sit1"), tenant("sit2"), tenant("sit3"));
    let empty = view_of(&[("application.properties", "OTHER = 1\n")], None);
    let path = nfs("a.yml");
    let file = PlaceholderFile {
        path: &path,
        text: "${GONE}\n",
        settings: None,
    };
    let view = |tenant| TenantView { tenant, view: &empty };
    let run = |views: &[TenantView<'_>]| {
        c10_unresolved_placeholders(&[file], views, &EnvNames::new([]), &Limits::default())
    };
    let forward = run(&[view(&sit1), view(&sit2), view(&sit3)]);
    assert_eq!(
        keys(&forward[0]),
        vec![(
            "GONE".to_owned(),
            vec!["sit1".to_owned(), "sit2".to_owned(), "sit3".to_owned()],
            vec![1]
        )]
    );
    assert_eq!(run(&[view(&sit3), view(&sit1), view(&sit2)]), forward);
    assert_eq!(run(&[view(&sit2), view(&sit3), view(&sit1)]), forward);
}

#[test]
fn checks_c9_c10_c11_c10_environment_names_match_the_way_spring_binds_them() {
    let env = EnvNames::new(["SERVER_PORT", "MY_APP_NAME", "plain"]);
    assert!(env.contains("SERVER_PORT"));
    assert!(env.contains("server.port"));
    assert!(env.contains("my-app.name"));
    assert!(env.contains("plain"));
    assert!(!env.contains("server.host"));
    assert!(!env.contains(""));
}

#[test]
fn checks_c9_c10_c11_c10_without_any_view_only_the_environment_fills_a_key() {
    let path = nfs("a.yml");
    let file = PlaceholderFile {
        path: &path,
        text: "${A} ${B}",
        settings: None,
    };
    let results = c10_unresolved_placeholders(&[file], &[], &EnvNames::new(["B"]), &Limits::default());
    assert_eq!(keys(&results[0]), vec![("A".to_owned(), Vec::new(), vec![1])]);
}

#[test]
fn checks_c9_c10_c11_c10_points_at_the_settings_that_hold_the_placeholder() {
    let text = "ok: 1\nurl: http://${HOST}:80/x\nnested:\n  token: ${TOKEN}\n";
    let path = nfs("svc/a.yml");
    let flat = flatten(
        &parse(text, Syntax::Yaml, &Limits::default()).unwrap(),
        &Limits::default(),
    )
    .unwrap();
    let file = PlaceholderFile {
        path: &path,
        text,
        settings: Some(&flat.settings),
    };
    let results = c10_unresolved_placeholders(&[file], &[], &EnvNames::new([]), &Limits::default());
    let found: Vec<(String, u32)> = results[0]
        .locations
        .iter()
        .map(|l| (l.path.to_string(), l.lines.start()))
        .collect();
    assert_eq!(found, vec![("nested.token".to_owned(), 4), ("url".to_owned(), 2)]);
}

#[test]
fn checks_c9_c10_c11_c10_leaves_the_forms_that_are_not_plain_keys_alone() {
    let path = nfs("a.yml");
    let file = PlaceholderFile {
        path: &path,
        text: "${A:default} ${${B}} $${C} ${D",
        settings: None,
    };
    assert!(c10_unresolved_placeholders(&[file], &[], &EnvNames::new([]), &Limits::default()).is_empty());
}

#[test]
fn checks_c9_c10_c11_c10_results_never_hold_a_value() {
    let sit1 = tenant("sit1");
    let view = view_of(
        &[("application.properties", "PASSWORD = hunter2-planted\n")],
        Some("sit1"),
    );
    let path = nfs("a.yml");
    let file = PlaceholderFile {
        path: &path,
        text: "p: ${PASSWORD}\nq: ${NOPE}\n",
        settings: None,
    };
    let results = c10_unresolved_placeholders(
        &[file],
        &[TenantView {
            tenant: &sit1,
            view: &view,
        }],
        &EnvNames::new([]),
        &Limits::default(),
    );
    let shown = format!("{results:?}");
    assert!(!shown.contains("hunter2"), "{shown}");
    assert!(
        !format!("{view:?}").contains("hunter2"),
        "a property view must not print values"
    );
}

// --- C11 ------------------------------------------------------------------------------------------------------------------

fn at(secs: i64) -> Timestamp {
    Timestamp::from_unix_millis(secs * 1000)
}

struct Facts {
    started: Timestamp,
    now: Vec<ChannelName>,
    at_start: Option<Vec<ChannelName>>,
    yml: Option<(NfsPath, Timestamp)>,
    folders: Vec<(NfsPath, Timestamp)>,
}

impl Facts {
    fn new() -> Self {
        Self {
            started: at(1_000),
            now: vec![channel("remote-itm-teller"), channel("atm")],
            at_start: Some(vec![channel("remote-itm-teller"), channel("atm")]),
            yml: Some((nfs("channels.yml"), at(500))),
            folders: vec![
                (nfs("ui"), at(100)),
                (nfs("ui/remote-itm-teller"), at(100)),
                (nfs("limits"), at(100)),
                (nfs("limits/atm"), at(100)),
            ],
        }
    }

    fn run(&self) -> Vec<engine::CheckResult> {
        self.run_with(&Limits::default())
    }

    fn run_with(&self, limits: &Limits) -> Vec<engine::CheckResult> {
        let folders: Vec<FolderSeen<'_>> = self
            .folders
            .iter()
            .map(|(dir, first_seen)| FolderSeen {
                dir,
                first_seen: *first_seen,
            })
            .collect();
        c11_config_server_restart_required(
            &ConfigServerFacts {
                started: self.started,
                channels_now: &self.now,
                channels_at_start: self.at_start.as_deref(),
                channels_yml: self.yml.as_ref().map(|(path, changed_at)| ChannelsYml {
                    path,
                    changed_at: *changed_at,
                }),
                folders: &folders,
            },
            limits,
        )
    }
}

fn reasons(results: &[engine::CheckResult]) -> Vec<RestartReason> {
    results
        .iter()
        .map(|r| {
            assert_eq!(r.kind, FindingKind::ConfigServerRestartRequired);
            let CheckDetail::ConfigServerRestart { reason, .. } = &r.detail else {
                panic!("not a C11 result: {r:?}");
            };
            *reason
        })
        .collect()
}

#[test]
fn checks_c9_c10_c11_c11_nothing_to_report_when_the_config_server_is_up_to_date() {
    assert!(Facts::new().run().is_empty());
}

#[test]
fn checks_c9_c10_c11_c11_a_folder_created_after_the_config_server_started() {
    let mut facts = Facts::new();
    facts.folders.push((nfs("payments"), at(2_000)));
    facts.folders.push((nfs("payments/sub"), at(2_100)));
    // A new folder below a channel folder is not a search location either way.
    facts.folders.push((nfs("ui/remote-itm-teller/new"), at(2_000)));
    // A folder first seen exactly when it started was there.
    facts.folders.push((nfs("early"), at(1_000)));
    let results = facts.run();
    assert_eq!(reasons(&results), vec![RestartReason::NewFolders]);
    let CheckDetail::ConfigServerRestart { folders, total, .. } = &results[0].detail else {
        panic!()
    };
    assert_eq!(
        folders.iter().map(NfsPath::as_str).collect::<Vec<_>>(),
        vec!["payments", "payments/sub"]
    );
    assert_eq!(*total, 2);
    assert_eq!(results[0].path.as_ref().unwrap().as_str(), "payments");
}

#[test]
fn checks_c9_c10_c11_c11_a_new_channel_folder_that_the_server_already_excludes_is_fine() {
    let mut facts = Facts::new();
    // `channels.yml` listed `atm` when the server started, so a folder called that is excluded from the start.
    facts.folders.push((nfs("holds/atm"), at(2_000)));
    assert!(facts.run().is_empty());
}

#[test]
fn checks_c9_c10_c11_c11_channels_yml_changed_after_the_start() {
    let mut facts = Facts::new();
    facts.yml = Some((nfs("channels.yml"), at(1_001)));
    let results = facts.run();
    assert_eq!(reasons(&results), vec![RestartReason::ChannelsYmlChanged]);
    assert_eq!(results[0].path.as_ref().unwrap().as_str(), "channels.yml");
    // Not newer than the start: nothing.
    facts.yml = Some((nfs("channels.yml"), at(1_000)));
    assert!(facts.run().is_empty());
}

#[test]
fn checks_c9_c10_c11_c11_a_listed_channel_without_a_folder_and_the_reverse() {
    let mut facts = Facts::new();
    facts.now.push(channel("atm-iso"));
    facts.at_start = Some(facts.now.clone());
    let results = facts.run();
    assert_eq!(reasons(&results), vec![RestartReason::ChannelListedWithoutFolder]);
    let CheckDetail::ConfigServerRestart { channels, .. } = &results[0].detail else {
        panic!()
    };
    assert_eq!(channels, &vec![channel("atm-iso")]);

    // A channel that `channels.yml` lists now and did not list at the start: its folder was a search location then.
    let mut facts = Facts::new();
    facts.now.push(channel("limits"));
    facts.folders.push((nfs("holds/limits"), at(100)));
    let results = facts.run();
    assert_eq!(
        reasons(&results),
        vec![RestartReason::ChannelFolderNotListedAtStart]
    );
    let CheckDetail::ConfigServerRestart { folders, .. } = &results[0].detail else {
        panic!()
    };
    let shown: Vec<&str> = folders.iter().map(NfsPath::as_str).collect();
    assert!(
        shown.contains(&"holds/limits") && shown.contains(&"limits"),
        "{shown:?}"
    );
    // Without the start-time list the engine does not guess.
    facts.at_start = None;
    assert!(!reasons(&facts.run()).contains(&RestartReason::ChannelFolderNotListedAtStart));
}

#[test]
fn checks_c9_c10_c11_c11_every_reason_at_once_in_a_stable_order() {
    let mut facts = Facts::new();
    facts.folders.push((nfs("payments"), at(2_000)));
    facts.yml = Some((nfs("channels.yml"), at(1_500)));
    facts.now.push(channel("ghost"));
    let results = facts.run();
    assert_eq!(
        reasons(&results),
        vec![
            RestartReason::NewFolders,
            RestartReason::ChannelsYmlChanged,
            RestartReason::ChannelListedWithoutFolder,
        ]
    );
    assert_eq!(facts.run(), results);
}

#[test]
fn checks_c9_c10_c11_c11_every_list_is_capped_and_the_folders_are_counted() {
    // 70 channels that the list has now and did not have at the start, each with a folder: the lists of the finding are
    // capped like every other (`Limits::locations_per_finding`), and the folders are counted.
    let mut facts = Facts::new();
    for i in 0..70 {
        let name = format!("c{i:02}");
        facts.now.push(channel(&name));
        facts.folders.push((nfs(&format!("ui/{name}")), at(100)));
    }
    facts.at_start = Some(vec![channel("remote-itm-teller"), channel("atm")]);
    let cap = Limits::default().locations_per_finding;
    let results = facts.run();
    assert_eq!(
        reasons(&results),
        vec![RestartReason::ChannelFolderNotListedAtStart]
    );
    let CheckDetail::ConfigServerRestart {
        folders,
        channels,
        total,
        ..
    } = &results[0].detail
    else {
        panic!()
    };
    assert_eq!((folders.len(), channels.len(), *total), (cap, cap, 70));
    assert_eq!(channels[0], channel("c00"));
    // A cap of zero lists nothing and still reports.
    let none = Limits {
        locations_per_finding: 0,
        ..Limits::default()
    };
    let results = facts.run_with(&none);
    assert_eq!(
        reasons(&results),
        vec![RestartReason::ChannelFolderNotListedAtStart]
    );
    let CheckDetail::ConfigServerRestart {
        folders,
        channels,
        total,
        ..
    } = &results[0].detail
    else {
        panic!()
    };
    assert_eq!((folders.len(), channels.len(), *total), (0, 0, 70));
}
