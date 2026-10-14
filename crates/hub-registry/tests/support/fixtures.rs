//! The synthetic fixtures (`xtask::fixtures`, rule 12): real-shaped NFS trees with CRLF files, images, channel folders, tenant
//! suffixes and a 6,000-line YAML file. The small scale (4 swimlanes of about 120 files) is generated once per test binary;
//! the target scale (40 swimlanes, about 80,000 files) is read from `target/fixtures/target` for the benchmarks.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bytes::Bytes;
use domain::NfsPath;

pub struct Fixtures {
    _dir: Option<tempfile::TempDir>,
    pub root: PathBuf,
    manifest: serde_json::Value,
}

/// The small-scale fixtures. They need `git` on the PATH, as the generator does.
pub fn small() -> &'static Fixtures {
    static CELL: OnceLock<Fixtures> = OnceLock::new();
    CELL.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("small");
        let manifest = xtask::fixtures::generate(&xtask::fixtures::Config {
            scale: xtask::fixtures::Scale::Small,
            seed: xtask::fixtures::DEFAULT_SEED,
            out: root.clone(),
        })
        .unwrap_or_else(|e| panic!("cannot generate the small fixtures (is git on the PATH?): {e}"));
        Fixtures {
            _dir: Some(dir),
            root,
            manifest: serde_json::to_value(&manifest).unwrap(),
        }
    })
}

/// The target-scale fixtures in `target/fixtures/target`, generated when missing (as `just agent-fixtures` does).
pub fn target() -> &'static Fixtures {
    static CELL: OnceLock<Fixtures> = OnceLock::new();
    CELL.get_or_init(|| {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
                PathBuf::from,
            )
            .join("fixtures/target");
        let manifest_path = root.join("manifest.json");
        let manifest = if manifest_path.exists() {
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap()
        } else {
            let m = xtask::fixtures::generate(&xtask::fixtures::Config {
                scale: xtask::fixtures::Scale::Target,
                seed: xtask::fixtures::DEFAULT_SEED,
                out: root.clone(),
            })
            .unwrap_or_else(|e| panic!("cannot generate the target fixtures (is git on the PATH?): {e}"));
            serde_json::to_value(&m).unwrap()
        };
        Fixtures {
            _dir: None,
            root,
            manifest,
        }
    })
}

impl Fixtures {
    pub fn swimlanes(&self) -> Vec<String> {
        self.manifest["swimlanes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .collect()
    }

    pub fn tenants(&self, swimlane: &str) -> Vec<String> {
        self.manifest["swimlanes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == swimlane)
            .map(|s| {
                s["tenants"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t.as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every file of a swimlane's NFS tree, sorted by path.
    pub fn files(&self, swimlane: &str) -> Vec<(NfsPath, Bytes)> {
        let base = self.root.join("nfs").join(swimlane);
        let mut out = Vec::new();
        walk(&base, &base, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<(NfsPath, Bytes)>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(base, &path, out);
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((
                NfsPath::parse(&rel).unwrap(),
                Bytes::from(std::fs::read(&path).unwrap()),
            ));
        }
    }
}
