//! T10 (D82 to D85, S11, S21): the config-server's resolution model.
//!
//! - `file_role`: which files are property sources and which are resources;
//! - `search_locations`: the folders the config-server looks in, in order, and the channel folders it never does;
//! - `property_view`: the merge of the property sources, by precedence, rank, extension and location;
//! - `resolve_resource`: the whole file that a request gets (tenant, then base; the channel folder first);
//! - `render`: `${KEY}` is replaced only when the key is in the view;
//! - `pickup_state`: when a change takes effect.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use bytes::Bytes;
use domain::{
    AppName, ChannelName, FileClass, FileRole, LogicalFile, MergeMode, NfsPath, PickupState, TenantId,
    TenantSet, Timestamp,
};
use engine::{
    ClientSettings, Limits, PickupChange, PickupInput, PropertyFile, PropertyView, ResolveError, Syntax,
    file_role, flatten, logical_file_role, merge_mode, parse, pickup_state, property_view, render,
    render_file, resolve_resource, scan_placeholders, search_locations,
};
use proptest::prelude::*;

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn app(s: &str) -> AppName {
    AppName::parse(s).unwrap()
}

fn tenant(s: &str) -> TenantId {
    TenantId::parse(s).unwrap()
}

fn channel(s: &str) -> ChannelName {
    ChannelName::parse(s).unwrap()
}

fn tenants(names: &[&str]) -> TenantSet {
    TenantSet::new(names.iter().map(|n| tenant(n))).unwrap()
}

fn apps(names: &[&str]) -> Vec<AppName> {
    names.iter().map(|n| app(n)).collect()
}

// --- file_role ------------------------------------------------------------------------------------------------------------

#[test]
fn file_role_table() {
    let tenants = tenants(&["sit1", "bis"]);
    let apps = apps(&["tx-infinity-api", "holds"]);
    let table = [
        // The four property sources, in each extension.
        ("application.properties", FileRole::PropertySource),
        ("application.yml", FileRole::PropertySource),
        ("application.yaml", FileRole::PropertySource),
        ("application-sit1.yml", FileRole::PropertySource),
        ("application-anything.properties", FileRole::PropertySource),
        ("tx-infinity-api.yml", FileRole::PropertySource),
        ("tx-infinity-api-sit1.yml", FileRole::PropertySource),
        ("tx-infinity-api-bis.properties", FileRole::PropertySource),
        // In any folder: every folder but a channel's is a search location.
        ("tx-infinity-api/application.yml", FileRole::PropertySource),
        ("some/deep/folder/holds.yaml", FileRole::PropertySource),
        // Everything else is a resource.
        ("tx-infinity-core.yml", FileRole::Resource),
        ("tx-infinity-api-sit9.yml", FileRole::Resource),
        ("other-sit1.yml", FileRole::Resource),
        ("entryGroupConfig.yml", FileRole::Resource),
        ("tx-infinity-api.json", FileRole::Resource),
        ("tx-infinity-api.xsl", FileRole::Resource),
        ("tx-infinity-api", FileRole::Resource),
        ("tx-infinity-api.YML", FileRole::Resource),
        ("application", FileRole::Resource),
        ("application-.yml", FileRole::Resource),
        ("applications.yml", FileRole::Resource),
        ("receipt-ci_1.bmp", FileRole::Resource),
        ("channels.yml", FileRole::Resource),
    ];
    for (path, want) in table {
        assert_eq!(file_role(&nfs(path), &tenants, &apps), want, "{path}");
    }
    // A name that is an app name only as a prefix is not one.
    assert_eq!(
        file_role(&nfs("tx-infinity-api2.yml"), &tenants, &apps),
        FileRole::Resource
    );
}

#[test]
fn a_logical_file_has_the_role_of_the_names_it_stands_for() {
    let apps = apps(&["tx-infinity-api"]);
    let logical = |s: &str| LogicalFile::parse(s).unwrap();
    assert_eq!(
        logical_file_role(&logical("tx-infinity-api/tx-infinity-api.yml"), &apps),
        FileRole::PropertySource
    );
    assert_eq!(
        logical_file_role(&logical("application.yml"), &apps),
        FileRole::PropertySource
    );
    assert_eq!(
        logical_file_role(&logical("tx-infinity-api/entryGroupConfig.yml"), &apps),
        FileRole::Resource
    );
    // The logical file has no tenant suffix, so `x-sit1.yml` there is a file whose name has a hyphen in it.
    assert_eq!(
        logical_file_role(&logical("tx-infinity-api-sit1.yml"), &apps),
        FileRole::Resource
    );
}

#[test]
fn the_merge_mode_follows_the_role() {
    assert_eq!(merge_mode(FileRole::PropertySource), MergeMode::SpringMerge);
    assert_eq!(merge_mode(FileRole::Resource), MergeMode::WholeFile);
}

// --- search_locations -----------------------------------------------------------------------------------------------------

fn paths(list: &[&str]) -> Vec<NfsPath> {
    list.iter().map(|p| nfs(p)).collect()
}

#[test]
fn root_app_and_profile_dirs_included() {
    let files = paths(&[
        "application.yml",
        "zeta/x.yml",
        "tx-infinity-api/a.yml",
        "tx-infinity-api/sit1,default/b.yml",
        "alpha/c.yml",
    ]);
    let locations = search_locations(&app("tx-infinity-api"), Some(&tenant("sit1")), &files, &[]);
    let order: Vec<&str> = locations.iter().map(NfsPath::as_str).collect();
    // The root, `{application}`, `{application}/{profile}`, then every other directory.
    assert_eq!(
        order,
        vec![
            "",
            "tx-infinity-api",
            "tx-infinity-api/sit1,default",
            "alpha",
            "zeta"
        ]
    );
    assert_eq!(locations.position(&NfsPath::root()), Some(0));
    assert_eq!(locations.position(&nfs("alpha")), Some(3));
    assert_eq!(locations.position(&nfs("nowhere")), None);
}

#[test]
fn channel_dirs_excluded() {
    let files = paths(&[
        "ui/a.yml",
        "ui/remote-itm-teller/a.yml",
        "holds/atm-iso/h.yml",
        "holds/atm-iso/deeper/h.yml",
        "tx-infinity-api/remote-itm-teller/t.yml",
        "holds/h.yml",
    ]);
    let channels = [channel("remote-itm-teller"), channel("atm-iso"), channel("atm")];
    let locations = search_locations(&app("holds"), None, &files, &channels);
    let order: Vec<&str> = locations.iter().map(NfsPath::as_str).collect();
    assert_eq!(order, vec!["", "holds", "tx-infinity-api", "ui"]);
    // The same list without the channels would hold them all.
    let all = search_locations(&app("holds"), None, &files, &[]);
    assert_eq!(all.len(), 8);
}

#[test]
fn locations_do_not_depend_on_the_order_of_the_files() {
    let mut files = paths(&["b/x.yml", "a/y.yml", "c/z/w.yml", "application.yml"]);
    let forward = search_locations(&app("a"), None, &files, &[]);
    files.reverse();
    assert_eq!(search_locations(&app("a"), None, &files, &[]), forward);
}

// --- property_view --------------------------------------------------------------------------------------------------------

struct Source {
    path: NfsPath,
    flat: engine::Flat,
}

fn props(path: &str, text: &str) -> Source {
    let syntax = Syntax::of_path(&nfs(path)).unwrap();
    Source {
        path: nfs(path),
        flat: flatten(
            &parse(text, syntax, &Limits::default()).unwrap(),
            &Limits::default(),
        )
        .unwrap(),
    }
}

fn view_of(app_name: &str, tenant_id: Option<&str>, sources: &[Source]) -> PropertyView {
    let files: Vec<NfsPath> = sources.iter().map(|s| s.path.clone()).collect();
    let tenant = tenant_id.map(tenant);
    let locations = search_locations(&app(app_name), tenant.as_ref(), &files, &[]);
    let property_files: Vec<PropertyFile<'_>> = sources
        .iter()
        .map(|s| PropertyFile {
            path: &s.path,
            settings: &s.flat.settings,
        })
        .collect();
    property_view(
        &app(app_name),
        tenant.as_ref(),
        &locations,
        &property_files,
        &Limits::default(),
    )
    .unwrap()
}

fn value(view: &PropertyView, key: &str) -> String {
    view.get(key)
        .map_or("(none)".to_owned(), |s| s.value.text.clone())
}

#[test]
fn the_four_ranks_in_order() {
    let sources = [
        props("application.properties", "K = application\nA = application\n"),
        props("holds.properties", "K = app\nB = app\n"),
        props(
            "application-sit1.properties",
            "K = application-tenant\nC = application-tenant\n",
        ),
        props("holds-sit1.properties", "K = app-tenant\nD = app-tenant\n"),
    ];
    let view = view_of("holds", Some("sit1"), &sources);
    assert_eq!(value(&view, "K"), "app-tenant");
    // A key only a lower source has is still there.
    assert_eq!(value(&view, "A"), "application");
    assert_eq!(value(&view, "B"), "app");
    assert_eq!(value(&view, "C"), "application-tenant");
    assert_eq!(value(&view, "D"), "app-tenant");

    // Without `<app>-<tenant>`: the next one.
    let without = &sources[..3];
    assert_eq!(
        value(&view_of("holds", Some("sit1"), without), "K"),
        "application-tenant"
    );
    // Without the tenant in the request, the tenant files are not read.
    assert_eq!(value(&view_of("holds", None, &sources), "K"), "app");
    // Another application's files are not read.
    assert_eq!(
        value(&view_of("other", Some("sit1"), &sources), "K"),
        "application-tenant"
    );
    // Another tenant's neither.
    assert_eq!(value(&view_of("holds", Some("bis"), &sources), "K"), "app");
}

#[test]
fn properties_beats_yml_at_equal_rank() {
    let sources = [
        props("application.yml", "K: from-yml\nonly_yml: 1\n"),
        props("application.properties", "K = from-properties\n"),
    ];
    let view = view_of("holds", None, &sources);
    assert_eq!(value(&view, "K"), "from-properties");
    assert_eq!(value(&view, "only_yml"), "1");
    // The order of the input does not matter.
    let reversed = [
        props("application.properties", "K = from-properties\n"),
        props("application.yml", "K: from-yml\nonly_yml: 1\n"),
    ];
    assert_eq!(value(&view_of("holds", None, &reversed), "K"), "from-properties");
}

#[test]
fn a_later_search_location_beats_an_earlier_one_at_equal_rank() {
    // The root comes first, `{application}` next, and every other folder after them.
    let sources = [
        props("application.properties", "K = root\n"),
        props("holds/application.properties", "K = app-dir\n"),
        props("zeta/application.properties", "K = zeta\n"),
        props("alpha/application.properties", "K = alpha\n"),
    ];
    let view = view_of("holds", None, &sources);
    assert_eq!(value(&view, "K"), "zeta", "the last location in the order wins");
    assert_eq!(view.get("K").unwrap().source, nfs("zeta/application.properties"));
    // Without the others, the app folder beats the root.
    assert_eq!(value(&view_of("holds", None, &sources[..2]), "K"), "app-dir");
}

#[test]
fn a_rank_beats_a_location() {
    let sources = [
        props("application.properties", "K = application-at-root\n"),
        props("zeta/application.properties", "K = application-in-zeta\n"),
        props("holds-sit1.properties", "K = app-tenant-at-root\n"),
    ];
    assert_eq!(
        value(&view_of("holds", Some("sit1"), &sources), "K"),
        "app-tenant-at-root"
    );
}

#[test]
fn yaml_keys_are_dotted_and_documents_merge_in_order() {
    let sources = [props(
        "application.yml",
        "server:\n  port: 8080\n  name: a\n---\nserver:\n  port: 9090\nlist:\n  - one\n  - two\n",
    )];
    let view = view_of("holds", None, &sources);
    assert_eq!(value(&view, "server.port"), "9090", "the later document wins");
    assert_eq!(value(&view, "server.name"), "a");
    assert_eq!(value(&view, "list[1]"), "two");
    assert!(
        view.iter().all(|s| !s.key.starts_with('#')),
        "no document prefix in a property view key"
    );
}

#[test]
fn a_list_in_a_higher_source_replaces_the_lower_list() {
    let sources = [
        props("application.yml", "hosts:\n  - a\n  - b\n  - c\nname: low\n"),
        props("holds.yml", "hosts:\n  - x\n"),
    ];
    let view = view_of("holds", None, &sources);
    assert_eq!(value(&view, "hosts[0]"), "x");
    assert_eq!(
        value(&view, "hosts[1]"),
        "(none)",
        "the lower list is dropped as a unit"
    );
    assert_eq!(value(&view, "name"), "low");
}

#[test]
fn files_outside_the_search_locations_are_not_read() {
    let sources = [
        props("application.properties", "K = root\n"),
        props("ui/remote-itm-teller/application.properties", "K = channel\n"),
    ];
    let files: Vec<NfsPath> = sources.iter().map(|s| s.path.clone()).collect();
    let channels = [channel("remote-itm-teller")];
    let locations = search_locations(&app("holds"), None, &files, &channels);
    let property_files: Vec<PropertyFile<'_>> = sources
        .iter()
        .map(|s| PropertyFile {
            path: &s.path,
            settings: &s.flat.settings,
        })
        .collect();
    let view = property_view(
        &app("holds"),
        None,
        &locations,
        &property_files,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(value(&view, "K"), "root");
}

#[test]
fn the_view_is_bounded_by_the_settings_limit() {
    let mut text = String::new();
    for i in 0..10 {
        writeln!(text, "k{i} = v").unwrap();
    }
    let sources = [props("application.properties", &text)];
    let files = [sources[0].path.clone()];
    let locations = search_locations(&app("holds"), None, &files, &[]);
    let property_files = [PropertyFile {
        path: &sources[0].path,
        settings: &sources[0].flat.settings,
    }];
    let at = Limits {
        settings: 10,
        ..Limits::default()
    };
    assert!(property_view(&app("holds"), None, &locations, &property_files, &at).is_ok());
    let under = Limits {
        settings: 9,
        ..Limits::default()
    };
    assert_eq!(
        property_view(&app("holds"), None, &locations, &property_files, &under).unwrap_err(),
        ResolveError::Limit(engine::LimitKind::Settings)
    );
}

/// The eight property sources an application `holds` and a tenant `sit1` can read, strongest first.
const STRONGEST_FIRST: [&str; 8] = [
    "holds-sit1.properties",
    "holds-sit1.yml",
    "application-sit1.properties",
    "application-sit1.yml",
    "holds.properties",
    "holds.yml",
    "application.properties",
    "application.yml",
];

proptest! {
    /// The precedence order of the documentation holds for any subset of the sources, in any order of the input:
    /// `<app>-<tenant>`, `application-<tenant>`, `<app>`, `application`; `.properties` over `.yml` inside a rank.
    #[test]
    fn precedence_prop(present in proptest::collection::vec(any::<bool>(), 8), shuffle in any::<u64>()) {
        let chosen: Vec<&str> = STRONGEST_FIRST
            .iter()
            .zip(&present)
            .filter(|(_, p)| **p)
            .map(|(n, _)| *n)
            .collect();
        prop_assume!(!chosen.is_empty());
        let mut sources: Vec<Source> = chosen
            .iter()
            .map(|name| {
                if name.ends_with(".properties") {
                    props(name, &format!("K = {name}\n"))
                } else {
                    props(name, &format!("K: {name}\n"))
                }
            })
            .collect();
        // A deterministic shuffle from the seed.
        let n = sources.len();
        let mut seed = shuffle;
        for i in (1..n).rev() {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            sources.swap(i, (seed >> 33) as usize % (i + 1));
        }
        let view = view_of("holds", Some("sit1"), &sources);
        prop_assert_eq!(value(&view, "K"), format!("{}", chosen[0]));
    }
}

// --- resolve_resource -----------------------------------------------------------------------------------------------------

fn set(list: &[&str]) -> BTreeSet<NfsPath> {
    list.iter().map(|p| nfs(p)).collect()
}

fn logical(s: &str) -> LogicalFile {
    LogicalFile::parse(s).unwrap()
}

#[test]
fn the_tenant_file_is_chosen_whole_and_base_otherwise() {
    let files = set(&["svc/a.xsl", "svc/a-sit1.xsl", "svc/b.xsl"]);
    let choice = resolve_resource(&logical("svc/a.xsl"), Some(&tenant("sit1")), None, &files).unwrap();
    assert_eq!(choice.path, nfs("svc/a-sit1.xsl"));
    assert!(choice.tenant_copy && !choice.in_channel);
    // Another tenant has no copy: base.
    let choice = resolve_resource(&logical("svc/a.xsl"), Some(&tenant("sit2")), None, &files).unwrap();
    assert_eq!(choice.path, nfs("svc/a.xsl"));
    assert!(!choice.tenant_copy);
    // No tenant in the request: base, whatever tenant copies exist.
    let choice = resolve_resource(&logical("svc/a.xsl"), None, None, &files).unwrap();
    assert_eq!(choice.path, nfs("svc/a.xsl"));
    // A file that is not there is not served.
    assert!(resolve_resource(&logical("svc/missing.xsl"), Some(&tenant("sit1")), None, &files).is_none());
    // A tenant copy alone is served to its tenant, and to nobody else.
    let only = set(&["svc/c-sit1.xsl"]);
    assert!(resolve_resource(&logical("svc/c.xsl"), Some(&tenant("sit1")), None, &only).is_some());
    assert!(resolve_resource(&logical("svc/c.xsl"), Some(&tenant("sit2")), None, &only).is_none());
}

#[test]
fn the_channel_folder_wins_when_the_request_names_the_channel() {
    let files = set(&[
        "svc/e.yml",
        "svc/e-bis.yml",
        "svc/remoteteller/e.yml",
        "svc/remoteteller/e-bis.yml",
        "svc/atm/e.yml",
    ]);
    let ch = channel("remoteteller");
    let bis = tenant("bis");
    let choice = resolve_resource(&logical("svc/e.yml"), Some(&bis), Some(&ch), &files).unwrap();
    assert_eq!(choice.path, nfs("svc/remoteteller/e-bis.yml"));
    assert!(choice.tenant_copy && choice.in_channel);
    // Another tenant gets the channel folder's base copy.
    let choice = resolve_resource(&logical("svc/e.yml"), Some(&tenant("sit1")), Some(&ch), &files).unwrap();
    assert_eq!(choice.path, nfs("svc/remoteteller/e.yml"));
    // A channel with no copy falls back to the folder above.
    let choice =
        resolve_resource(&logical("svc/e.yml"), Some(&bis), Some(&channel("other")), &files).unwrap();
    assert_eq!(choice.path, nfs("svc/e-bis.yml"));
    assert!(!choice.in_channel);
    // No channel in the request: the channel folders are not looked at.
    let choice = resolve_resource(&logical("svc/e.yml"), Some(&bis), None, &files).unwrap();
    assert_eq!(choice.path, nfs("svc/e-bis.yml"));
}

#[test]
fn an_extensionless_file_has_the_suffix_at_the_end() {
    let files = set(&["jwt/mappings", "jwt/mappings-sit1"]);
    let choice = resolve_resource(&logical("jwt/mappings"), Some(&tenant("sit1")), None, &files).unwrap();
    assert_eq!(choice.path, nfs("jwt/mappings-sit1"));
}

// --- render ---------------------------------------------------------------------------------------------------------------

fn view_from(pairs: &[(&str, &str)]) -> PropertyView {
    let mut text = String::new();
    for (k, v) in pairs {
        writeln!(text, "{k} = {v}").unwrap();
    }
    view_of("holds", None, &[props("application.properties", &text)])
}

fn rendered(text: &str, view: &PropertyView) -> String {
    render(text, view, &Limits::default()).unwrap().text
}

#[test]
fn keys_in_the_view_are_replaced() {
    let view = view_from(&[("A", "1"), ("b.c", "two"), ("d[0]", "x-y")]);
    assert_eq!(rendered("a: ${A}, ${b.c} / ${d[0]}", &view), "a: 1, two / x-y");
    let r = render("${A}${A}", &view, &Limits::default()).unwrap();
    assert_eq!((r.text.as_str(), r.replaced), ("11", 2));
}

#[test]
fn unknown_key_left_as_written() {
    let view = view_from(&[("A", "1")]);
    let r = render(
        "x: ${A} y: ${NOPE}\nz: ${NOPE}\nw: ${OTHER}",
        &view,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(r.text, "x: 1 y: ${NOPE}\nz: ${NOPE}\nw: ${OTHER}");
    let unresolved: Vec<(&str, u32)> = r.unresolved.iter().map(|p| (p.key.as_str(), p.line)).collect();
    assert_eq!(
        unresolved,
        vec![("NOPE", 1), ("OTHER", 3)],
        "once each, at the first line, sorted by key"
    );
}

#[test]
fn value_containing_placeholder_not_re_expanded() {
    let view = view_from(&[("A", "${B}"), ("B", "secret-b")]);
    assert_eq!(rendered("v: ${A}", &view), "v: ${B}");
}

#[test]
fn forms_that_are_not_plain_keys_are_left_as_written() {
    let view = view_from(&[("A", "1"), ("a", "lower")]);
    for text in [
        "${A:default}",
        "${${A}}",
        "${A${A}}",
        "$${A}",
        "${}",
        "${ A }",
        "${A",
        "$ {A}",
        "{A}",
        "${A-}",
    ] {
        let r = render(text, &view, &Limits::default()).unwrap();
        // Nothing here is a plain `${KEY}` with a key in the view, so nothing is replaced.
        assert_eq!(r.text, text, "{text}");
        assert!(r.unresolved.iter().all(|p| p.key != "A"), "{text}");
    }
    // Keys are case-sensitive.
    assert_eq!(rendered("${a}/${A}", &view), "lower/1");
}

#[test]
fn placeholder_scan_gives_keys_and_lines() {
    let found = scan_placeholders("a: ${X}\r\nb: ${Y.z}\n\n${X} ${W[0]} ${bad key}");
    let got: Vec<(&str, u32)> = found.iter().map(|p| (p.key.as_str(), p.line)).collect();
    assert_eq!(got, vec![("X", 1), ("Y.z", 2), ("X", 4), ("W[0]", 4)]);
}

#[test]
fn render_is_bounded() {
    let view = view_from(&[("A", &"x".repeat(1000))]);
    let tiny = Limits {
        input_bytes: 100,
        ..Limits::default()
    };
    // Input over the limit.
    assert_eq!(
        render(&"y".repeat(101), &view, &tiny).unwrap_err(),
        ResolveError::Limit(engine::LimitKind::InputBytes)
    );
    // Output over the limit: five bytes in, a thousand out.
    assert_eq!(
        render("${A}", &view, &tiny).unwrap_err(),
        ResolveError::Limit(engine::LimitKind::InputBytes)
    );
    // Exactly at the limit passes.
    let ok = Limits {
        input_bytes: 1000,
        ..Limits::default()
    };
    assert_eq!(render("${A}", &view, &ok).unwrap().text.len(), 1000);
}

#[test]
fn a_latin1_file_within_the_limit_is_rendered_although_its_text_is_longer() {
    // The limit is on the file. A Latin-1 byte over 0x7F is two bytes of text, so this file of 64 bytes is a text of 124.
    let view = view_from(&[("A", "1")]);
    let limits = Limits {
        input_bytes: 100,
        ..Limits::default()
    };
    let mut data = b"${A}".to_vec();
    data.extend(std::iter::repeat_n(0xE9, 60));
    let data = Bytes::from(data);
    assert!(data.len() <= limits.input_bytes);
    let r = render_file(&data, FileClass::Text, &view, &limits).unwrap();
    assert_eq!(r.replaced, 1);
    let mut want = b"1".to_vec();
    want.extend(std::iter::repeat_n(0xE9, 60));
    assert_eq!(r.bytes.as_ref(), want.as_slice());
    // A file over the limit is refused as ever, and so is a rendering that is.
    let over = Bytes::from(vec![b'x'; 101]);
    assert_eq!(
        render_file(&over, FileClass::Text, &view, &limits).unwrap_err(),
        ResolveError::Decode(engine::DecodeError::TooLarge)
    );
    let long = view_from(&[("A", &"y".repeat(101))]);
    assert_eq!(
        render_file(&data, FileClass::Text, &long, &limits).unwrap_err(),
        ResolveError::Limit(engine::LimitKind::InputBytes)
    );
}

#[test]
fn a_hostile_run_of_openers_is_linear_enough() {
    let view = view_from(&[("A", "1")]);
    let text = "${".repeat(200_000);
    let r = render(&text, &view, &Limits::default()).unwrap();
    assert_eq!(r.text, text);
}

#[test]
fn binary_not_rendered() {
    let view = view_from(&[("A", "1")]);
    let data = Bytes::from_static(b"\x89PNG ${A} \x00\x01");
    for class in [FileClass::Binary, FileClass::Denied] {
        let r = render_file(&data, class, &view, &Limits::default()).unwrap();
        assert_eq!(r.bytes, data, "{class:?}");
        assert_eq!(r.replaced, 0);
    }
}

#[test]
fn rendering_keeps_the_encoding_the_bom_and_the_line_endings() {
    let view = view_from(&[("A", "1")]);
    // UTF-8 with a BOM and CRLF.
    let mut utf8 = vec![0xEF, 0xBB, 0xBF];
    utf8.extend_from_slice(b"a: ${A}\r\nb: ${B}\r\n");
    let r = render_file(
        &Bytes::from(utf8),
        FileClass::Structured,
        &view,
        &Limits::default(),
    )
    .unwrap();
    let mut want = vec![0xEF, 0xBB, 0xBF];
    want.extend_from_slice(b"a: 1\r\nb: ${B}\r\n");
    assert_eq!(r.bytes.to_vec(), want);
    // UTF-16 LE with a BOM.
    let text = "a: ${A}\n";
    let mut utf16 = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    let r = render_file(&Bytes::from(utf16), FileClass::Text, &view, &Limits::default()).unwrap();
    let mut want = vec![0xFF, 0xFE];
    for u in "a: 1\n".encode_utf16() {
        want.extend_from_slice(&u.to_le_bytes());
    }
    assert_eq!(r.bytes.to_vec(), want);
    // The hash is the hash of what is served, line endings aside.
    let lf = render_file(
        &Bytes::from_static(b"a: ${A}\n"),
        FileClass::Structured,
        &view,
        &Limits::default(),
    )
    .unwrap();
    let crlf = render_file(
        &Bytes::from_static(b"a: ${A}\r\n"),
        FileClass::Structured,
        &view,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(lf.hash, crlf.hash);
}

#[test]
fn env_values_never_used() {
    // The process environment has HOME (and PATH); the engine has no way to read it, so a placeholder with such a key stays.
    assert!(std::env::var_os("PATH").is_some());
    let view = view_from(&[("A", "1")]);
    assert_eq!(rendered("${PATH} ${HOME} ${A}", &view), "${PATH} ${HOME} 1");
}

// --- pickup_state ---------------------------------------------------------------------------------------------------------

const TTL: u32 = 1_200;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_unix_millis(secs * 1000)
}

fn pickup(
    change: PickupChange,
    notifications: bool,
    f: impl FnOnce(&mut PickupInput<'static>),
) -> engine::Pickup {
    let mut input = PickupInput {
        change,
        client: ClientSettings {
            notifications_enabled: notifications,
            cache_ttl_secs: TTL,
        },
        changed_at: at(10_000),
        now: at(10_000),
        notified_at: None,
        pod_starts: &[],
        config_server_started: at(1_000),
    };
    f(&mut input);
    pickup_state(&input)
}

#[test]
fn resource_live_within_ttl_then_live() {
    let p = pickup(PickupChange::ResourceFile, false, |i| i.now = at(10_100));
    assert_eq!(p.state, PickupState::LiveWithinTtl);
    assert_eq!(
        p.live_by,
        Some(at(10_000 + i64::from(TTL))),
        "the cache expires at HH:MM"
    );
    // Just before and exactly at the end of the lifetime.
    let p = pickup(PickupChange::ResourceFile, false, |i| {
        i.now = at(10_000 + i64::from(TTL) - 1);
    });
    assert_eq!(p.state, PickupState::LiveWithinTtl);
    let p = pickup(PickupChange::ResourceFile, false, |i| {
        i.now = at(10_000 + i64::from(TTL));
    });
    assert_eq!((p.state, p.live_by), (PickupState::Live, None));
}

#[test]
fn a_notification_makes_a_resource_live_at_once_when_the_client_listens() {
    let listening = pickup(PickupChange::ResourceFile, true, |i| {
        i.now = at(10_100);
        i.notified_at = Some(at(10_050));
    });
    assert_eq!(listening.state, PickupState::Live);
    // A client that does not listen to notifications waits for its cache.
    let deaf = pickup(PickupChange::ResourceFile, false, |i| {
        i.now = at(10_100);
        i.notified_at = Some(at(10_050));
    });
    assert_eq!(deaf.state, PickupState::LiveWithinTtl);
    // A notification from before the change says nothing about it.
    let stale = pickup(PickupChange::ResourceFile, true, |i| {
        i.now = at(10_100);
        i.notified_at = Some(at(9_000));
    });
    assert_eq!(stale.state, PickupState::LiveWithinTtl);
}

#[test]
fn property_source_with_notifications() {
    let p = pickup(PickupChange::PropertySource, true, |i| i.now = at(10_500));
    assert_eq!(p.state, PickupState::NeedsNotifyOrRestart);
    assert!(p.refreshable_by_notification);
    assert_eq!(p.live_by, None);
    // The notification arrives after the change: the context refreshes.
    let p = pickup(PickupChange::PropertySource, true, |i| {
        i.now = at(10_500);
        i.notified_at = Some(at(10_010));
    });
    assert_eq!(p.state, PickupState::Live);
}

#[test]
fn property_source_without_notifications() {
    let p = pickup(PickupChange::PropertySource, false, |i| i.now = at(99_999));
    assert_eq!(p.state, PickupState::NeedsNotifyOrRestart);
    assert!(!p.refreshable_by_notification, "only a restart helps");
    // A notification does not help a client that does not listen.
    let p = pickup(PickupChange::PropertySource, false, |i| {
        i.notified_at = Some(at(10_010));
        i.now = at(10_500);
    });
    assert_eq!(p.state, PickupState::NeedsNotifyOrRestart);
}

#[test]
fn new_folder_or_channels_yml_needs_config_server_restart() {
    for change in [PickupChange::NewFolder, PickupChange::ChannelsYml] {
        // The config-server started before the change: it has not seen it.
        let p = pickup(change, true, |i| i.now = at(50_000));
        assert_eq!(p.state, PickupState::NeedsConfigServerRestart, "{change:?}");
        // Whatever the client does, a pod restart does not help.
        let p = pickup(change, true, |i| {
            i.now = at(50_000);
            i.notified_at = Some(at(10_010));
        });
        assert_eq!(p.state, PickupState::NeedsConfigServerRestart, "{change:?}");
        // The config-server restarted after the change.
        let p = pickup(change, false, |i| {
            i.now = at(50_000);
            i.config_server_started = at(20_000);
        });
        assert_eq!(p.state, PickupState::Live, "{change:?}");
    }
}

#[test]
fn pending_iff_any_pod_started_before_change() {
    let all_after = [at(10_001), at(10_000), at(12_000)];
    let one_before = [at(10_001), at(9_999)];
    for change in [PickupChange::PropertySource, PickupChange::ResourceFile] {
        let live = {
            let mut input = base_input(change);
            input.pod_starts = &all_after;
            pickup_state(&input)
        };
        assert_eq!(
            live.state,
            PickupState::Live,
            "{change:?}: every pod started after the change"
        );
        let pending = {
            let mut input = base_input(change);
            input.pod_starts = &one_before;
            pickup_state(&input)
        };
        assert_ne!(
            pending.state,
            PickupState::Live,
            "{change:?}: one pod started before the change"
        );
    }
}

fn base_input(change: PickupChange) -> PickupInput<'static> {
    PickupInput {
        change,
        client: ClientSettings {
            notifications_enabled: true,
            cache_ttl_secs: TTL,
        },
        changed_at: at(10_000),
        now: at(10_100),
        notified_at: None,
        pod_starts: &[],
        config_server_started: at(1_000),
    }
}

#[test]
fn pickup_is_deterministic_and_total() {
    let a = pickup(PickupChange::ResourceFile, true, |i| i.now = at(10_100));
    let b = pickup(PickupChange::ResourceFile, true, |i| i.now = at(10_100));
    assert_eq!(a, b);
    // Extreme times do not overflow.
    let mut input = base_input(PickupChange::ResourceFile);
    input.changed_at = Timestamp::from_unix_millis(i64::MAX);
    input.client.cache_ttl_secs = u32::MAX;
    let _ = pickup_state(&input);
    input.changed_at = Timestamp::from_unix_millis(i64::MIN);
    let _ = pickup_state(&input);
}
