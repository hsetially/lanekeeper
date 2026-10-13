//! T4 (D14, D82, S11): `effective` for both merge modes, with the origin of every value.
//!
//! - `whole_file`: the tenant file replaces the base file. Nothing is merged.
//! - `spring_merge`: maps merge key by key, and a list or a scalar is replaced as a unit by the higher source. A root
//!   `application.*` file is the lowest layer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;

use bytes::Bytes;
use domain::{ContentHash, FileClass, MergeMode, NfsPath};
use engine::{Effective, Flat, Layer, Layers, effective, normalised_hash};
use support::yaml;

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

struct File {
    path: NfsPath,
    flat: Flat,
    hash: ContentHash,
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

/// The text form the snapshots use: the sources, then one line per value with its origin.
fn dump(e: &Effective) -> String {
    let mut out = String::new();
    writeln!(out, "mode {:?}", e.mode).unwrap();
    writeln!(out, "hash {}", e.hash).unwrap();
    for s in &e.sources {
        writeln!(out, "source #{} {} {}", s.rank, s.path, s.hash).unwrap();
    }
    for v in &e.settings {
        writeln!(
            out,
            "{} = {:?} {:?} <- #{} {} L{}-{}",
            v.path,
            v.value.text,
            v.value.ty,
            v.origin.rank,
            v.origin.file,
            v.origin.lines.start(),
            v.origin.lines.end()
        )
        .unwrap();
    }
    out
}

const BASE: &str = "\
server:
  port: 8080
  host: base.example
list:
  - 1
  - 2
  - 3
users:
  - name: x
    role: y
scalar: 1
mapish:
  k: v
only_base: yes
";

const TENANT: &str = "\
server:
  port: 9090
list:
  - 9
users:
  - name: z
scalar:
  now: a-map
mapish: flat
only_tenant: true
";

#[test]
fn whole_file() {
    let base = file("svc/app.yml", BASE);
    let tenant = file("svc/app-sit1.yml", TENANT);

    // The tenant file replaces the base file: nothing of base is left, not even `server.host`.
    let both = effective(
        MergeMode::WholeFile,
        &Layers {
            tenant: Some(tenant.layer()),
            base: Some(base.layer()),
            root: None,
        },
    )
    .unwrap();
    insta::assert_snapshot!("effective_whole_file_tenant_wins", dump(&both));
    assert_eq!(both.hash, tenant.hash);
    assert!(
        both.settings
            .iter()
            .all(|v| v.origin.file == tenant.path && v.origin.rank == 1)
    );
    assert!(both.settings.iter().all(|v| v.path.as_str() != "server.host"));

    // No tenant file: the base file.
    let only_base = effective(
        MergeMode::WholeFile,
        &Layers {
            tenant: None,
            base: Some(base.layer()),
            root: None,
        },
    )
    .unwrap();
    insta::assert_snapshot!("effective_whole_file_base_only", dump(&only_base));
    assert_eq!(only_base.hash, base.hash);

    // Only a tenant file (an orphan): it is what is served.
    let only_tenant = effective(
        MergeMode::WholeFile,
        &Layers {
            tenant: Some(tenant.layer()),
            base: None,
            root: None,
        },
    )
    .unwrap();
    assert_eq!(only_tenant.sources.len(), 1);
    assert_eq!(only_tenant.hash, tenant.hash);

    // Nothing at all: no effective file.
    assert!(
        effective(
            MergeMode::WholeFile,
            &Layers {
                tenant: None,
                base: None,
                root: None
            }
        )
        .is_none()
    );

    // A root application file is not part of a whole-file resolution.
    let root = file("application.yml", "root: 1\n");
    let with_root = effective(
        MergeMode::WholeFile,
        &Layers {
            tenant: None,
            base: Some(base.layer()),
            root: Some(root.layer()),
        },
    )
    .unwrap();
    assert_eq!(with_root, only_base);
}

#[test]
fn spring_merge_maps_merge_lists_replace() {
    let base = file("svc/app.yml", BASE);
    let tenant = file("svc/app-sit1.yml", TENANT);
    let merged = effective(
        MergeMode::SpringMerge,
        &Layers {
            tenant: Some(tenant.layer()),
            base: Some(base.layer()),
            root: None,
        },
    )
    .unwrap();
    insta::assert_snapshot!("effective_spring_merge", dump(&merged));

    let value = |path: &str| {
        merged
            .settings
            .iter()
            .find(|v| v.path.as_str() == path)
            .map(|v| (v.value.text.as_str(), v.origin.rank))
    };
    // Maps merge: the tenant's port, base's host.
    assert_eq!(value("server.port"), Some(("9090", 1)));
    assert_eq!(value("server.host"), Some(("base.example", 2)));
    // A list is replaced as a unit: the tenant's single item, none of base's.
    assert_eq!(value("list[0]"), Some(("9", 1)));
    assert_eq!(value("list[1]"), None);
    assert_eq!(value("list[2]"), None);
    // Items of a list of maps are replaced as a unit too: base's `users[0].role` does not survive.
    assert_eq!(value("users[0].name"), Some(("z", 1)));
    assert_eq!(value("users[0].role"), None);
    // A map replaces a scalar, and a scalar replaces a map.
    assert_eq!(value("scalar"), None);
    assert_eq!(value("scalar.now"), Some(("a-map", 1)));
    assert_eq!(value("mapish"), Some(("flat", 1)));
    assert_eq!(value("mapish.k"), None);
    // Keys only one side has are kept.
    assert_eq!(value("only_base"), Some(("yes", 2)));
    assert_eq!(value("only_tenant"), Some(("true", 1)));
}

#[test]
fn a_list_replaced_by_a_map_and_a_map_by_a_list() {
    let base = file("a.yml", "x:\n  - 1\n  - 2\ny:\n  k: v\n");
    let tenant = file("a-sit1.yml", "x:\n  k: v\ny:\n  - 7\n");
    let merged = effective(
        MergeMode::SpringMerge,
        &Layers {
            tenant: Some(tenant.layer()),
            base: Some(base.layer()),
            root: None,
        },
    )
    .unwrap();
    let paths: Vec<&str> = merged.settings.iter().map(|v| v.path.as_str()).collect();
    assert_eq!(paths, ["x.k", "y[0]"]);
}

#[test]
fn nested_lists_are_replaced_by_the_outermost_list_the_higher_source_defines() {
    let base = file(
        "a.yml",
        "m:\n  - items:\n      - a\n      - b\n    keep: 1\n  - other: 2\nsib:\n  l: [1, 2]\n  n: 3\n",
    );
    let tenant = file("a-sit1.yml", "m:\n  - items:\n      - z\nsib:\n  l: [9]\n");
    let merged = effective(
        MergeMode::SpringMerge,
        &Layers {
            tenant: Some(tenant.layer()),
            base: Some(base.layer()),
            root: None,
        },
    )
    .unwrap();
    let rendered: Vec<String> = merged
        .settings
        .iter()
        .map(|v| format!("{}={}#{}", v.path, v.value.text, v.origin.rank))
        .collect();
    // `m` is one list: all of base's items are gone. `sib.l` is replaced, `sib.n` stays.
    assert_eq!(rendered, ["m[0].items[0]=z#1", "sib.l[0]=9#1", "sib.n=3#2"]);
}

#[test]
fn root_application_is_lowest_layer() {
    let root = file(
        "application.yml",
        "logging:\n  level: INFO\nserver:\n  port: 1\n  keepalive: 5\nfeatures:\n  - a\n  - b\n",
    );
    let base = file("svc/app.yml", "server:\n  port: 8080\nfeatures:\n  - c\n");
    let tenant = file("svc/app-sit1.yml", "logging:\n  level: DEBUG\n");
    let merged = effective(
        MergeMode::SpringMerge,
        &Layers {
            tenant: Some(tenant.layer()),
            base: Some(base.layer()),
            root: Some(root.layer()),
        },
    )
    .unwrap();
    insta::assert_snapshot!("effective_spring_merge_with_root", dump(&merged));
    let rank_of = |path: &str| {
        merged
            .settings
            .iter()
            .find(|v| v.path.as_str() == path)
            .map(|v| v.origin.rank)
    };
    assert_eq!(rank_of("logging.level"), Some(1));
    assert_eq!(rank_of("server.port"), Some(2));
    // Only the root file has this one.
    assert_eq!(rank_of("server.keepalive"), Some(3));
    // Base's list replaces the root's.
    assert_eq!(rank_of("features[0]"), Some(2));
    assert_eq!(rank_of("features[1]"), None);
    let ranks: Vec<u32> = merged.sources.iter().map(|s| s.rank).collect();
    assert_eq!(ranks, [1, 2, 3]);

    // The root file alone, with no base and no tenant file, is what is left, at rank 1 (ranks count the layers present).
    let alone = effective(
        MergeMode::SpringMerge,
        &Layers {
            tenant: None,
            base: None,
            root: Some(root.layer()),
        },
    )
    .unwrap();
    assert!(
        alone
            .settings
            .iter()
            .all(|v| v.origin.rank == 1 && v.origin.file == root.path)
    );
}

#[test]
fn every_value_has_origin() {
    let root = file("application.yml", "a: 1\nb: 2\nc:\n  d: 3\n");
    let base = file("svc/app.yml", "b: 20\nc:\n  e: 4\nl: [1,\n  2]\n");
    let tenant = file("svc/app-sit1.yml", "c:\n  d: 30\nl:\n  - 5\n");
    let layers = [&tenant, &base, &root];
    for mode in [MergeMode::SpringMerge, MergeMode::WholeFile] {
        let merged = effective(
            mode,
            &Layers {
                tenant: Some(tenant.layer()),
                base: Some(base.layer()),
                root: Some(root.layer()),
            },
        )
        .unwrap();
        assert!(!merged.settings.is_empty());
        for v in &merged.settings {
            // The origin names a source of the result, at its rank, and the lines are the lines of that setting in that file.
            let source = merged.sources.iter().find(|s| s.path == v.origin.file).unwrap();
            assert_eq!(source.rank, v.origin.rank);
            let from = layers.iter().find(|f| f.path == v.origin.file).unwrap();
            let original = from.flat.settings.iter().find(|s| s.path == v.path).unwrap();
            assert_eq!(original.lines, v.origin.lines, "{}", v.path);
            assert_eq!(original.value, v.value, "{}", v.path);
        }
    }
}

#[test]
fn documents_of_a_file_merge_in_order_and_later_wins() {
    let tenant = file("a-sit1.yml", "x: 1\n---\nx: 2\ny: 3\n");
    let merged = effective(
        MergeMode::SpringMerge,
        &Layers {
            tenant: Some(tenant.layer()),
            base: None,
            root: None,
        },
    )
    .unwrap();
    // No `#N/` in the merged paths: the documents are one property source (plan decision A10).
    let rendered: Vec<String> = merged
        .settings
        .iter()
        .map(|v| format!("{}={}", v.path, v.value.text))
        .collect();
    assert_eq!(rendered, ["x=2", "y=3"]);
    // The origin of `x` is the line where the second document writes it.
    let x = &merged.settings[0];
    assert_eq!(x.origin.lines.start(), 3);
}

#[test]
fn the_result_is_sorted_by_path_and_repeatable() {
    let base = file("a.yml", "zeta: 1\nalpha: 2\nmid:\n  b: 1\n  a: 2\n");
    let tenant = file("a-sit1.yml", "beta: 3\nalpha: 4\n");
    let layers = Layers {
        tenant: Some(tenant.layer()),
        base: Some(base.layer()),
        root: None,
    };
    let first = effective(MergeMode::SpringMerge, &layers).unwrap();
    assert_eq!(first, effective(MergeMode::SpringMerge, &layers).unwrap());
    let paths: Vec<&str> = first.settings.iter().map(|v| v.path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
}

#[test]
fn spring_merge_hash_follows_the_merged_settings_not_the_files() {
    let base = file("a.yml", "a: 1\nb: 2\n");
    // The same settings written in another order, with other spacing and comments: another file, the same effective config.
    let reordered = file("a.yml", "# note\nb:   2\na: 1\n");
    let changed = file("a.yml", "a: 1\nb: 3\n");
    let hash = |f: &File| {
        effective(
            MergeMode::SpringMerge,
            &Layers {
                tenant: None,
                base: Some(f.layer()),
                root: None,
            },
        )
        .unwrap()
        .hash
    };
    assert_ne!(base.hash, reordered.hash);
    assert_eq!(hash(&base), hash(&reordered));
    assert_ne!(hash(&base), hash(&changed));

    // A tenant file that restates base's values exactly leaves the effective config, and its hash, as it was.
    let restating = file("a-sit1.yml", "a: 1\n");
    let with_tenant = effective(
        MergeMode::SpringMerge,
        &Layers {
            tenant: Some(restating.layer()),
            base: Some(base.layer()),
            root: None,
        },
    )
    .unwrap();
    assert_eq!(with_tenant.hash, hash(&base));
    // A value that differs in type only is a different config.
    let typed = file("a.yml", "a: '1'\nb: 2\n");
    assert_ne!(hash(&typed), hash(&base));
}
