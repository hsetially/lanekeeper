//! Effective config of one file (T4, D82): what a service gets from the layers a file can come from.
//!
//! A logical file can exist as a tenant file (`app-sit1.yml`), as a base file (`app.yml`), and, for a Spring property
//! source, as a root file (`application.yml`) below both. `effective` takes those layers, already flattened, and gives the
//! settings a service would see, each with the layer it came from.
//!
//! - **`whole_file`**: the tenant file replaces the base file. Nothing is merged, and a root file is not part of it.
//! - **`spring_merge`**: the layers are property sources. Maps merge key by key. A list is replaced as a unit by the
//!   highest layer that defines any item of it (the binder's rule: `a[0]` in a higher source drops every `a[n]` below), a
//!   scalar replaces a map and a map replaces a scalar. The documents of one file merge in order and the later one wins,
//!   so a merged path has no `#<n>/` prefix (plan decision A10).
//!
//! The function reads no file and renders nothing: it works on the flattened settings, so the caller (which has parsed each
//! blob once) pays for no parse. Rendering of `${KEY}` placeholders is `render` (T10, `resolution`); the mode of a file follows
//! from its role (`file_role`, `merge_mode`), not from a pattern.
//!
//! # Hashes
//!
//! `Effective::hash` is the identity of the result. For `whole_file` it is the hash of the chosen file, as the caller
//! gives it (`normalised_hash`: line endings do not count). For `spring_merge` it is a hash of the merged settings, so two
//! different files, or two different layerings, that give the same settings have the same hash.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use domain::{ContentHash, LineRange, MergeMode, NfsPath, SettingPath, SettingValue, ValueFlag, ValueType};
use sha2::{Digest, Sha256};

use crate::parse::{FlatSetting, PathSegment, decode_setting_path, encode_setting_path};

/// One file that can supply the effective config: where it is, its hash, and its settings.
#[derive(Debug, Clone, Copy)]
pub struct Layer<'a> {
    pub path: &'a NfsPath,
    /// The hash of the file's content with line endings normalised (`normalised_hash`).
    pub hash: ContentHash,
    /// The flattened settings (`flatten`). Empty for a text or binary file.
    pub settings: &'a [FlatSetting],
}

/// The layers of one logical file, highest precedence first: tenant, base, root.
#[derive(Debug, Clone, Copy)]
pub struct Layers<'a> {
    /// The tenant file, `<name>-<tenant>.<ext>`.
    pub tenant: Option<Layer<'a>>,
    /// The base file, `<name>.<ext>`.
    pub base: Option<Layer<'a>>,
    /// The root `application.*` file, the lowest layer of a property source. Ignored by `whole_file`.
    pub root: Option<Layer<'a>>,
}

/// A file the effective config is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: NfsPath,
    /// 1 is the highest precedence. Ranks count the layers that are present: a base file alone has rank 1.
    pub rank: u32,
    pub hash: ContentHash,
}

/// Where an effective value is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub file: NfsPath,
    pub rank: u32,
    /// The lines of the setting in that file.
    pub lines: LineRange,
}

/// One setting of the effective config.
#[derive(Clone, PartialEq, Eq)]
pub struct EffectiveValue {
    pub path: SettingPath,
    pub value: SettingValue,
    pub origin: Origin,
}

impl std::fmt::Debug for EffectiveValue {
    // A value may be a secret: `Debug` shows its size, never its text (S13, S21).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectiveValue")
            .field("path", &self.path)
            .field("value_bytes", &self.value.text.len())
            .field("origin", &self.origin)
            .finish()
    }
}

/// The result of `effective`.
#[derive(Clone, PartialEq, Eq)]
pub struct Effective {
    pub mode: MergeMode,
    /// The layers that supply something, highest first. `whole_file`: the one that is served.
    pub sources: Vec<Source>,
    /// The settings, sorted by path, each path once.
    pub settings: Vec<EffectiveValue>,
    pub hash: ContentHash,
}

impl std::fmt::Debug for Effective {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Effective")
            .field("mode", &self.mode)
            .field("sources", &self.sources)
            .field("settings", &self.settings.len())
            .field("hash", &self.hash)
            .finish()
    }
}

/// The effective config of a logical file, or `None` when no layer exists.
pub fn effective(mode: MergeMode, layers: &Layers<'_>) -> Option<Effective> {
    match mode {
        MergeMode::WholeFile => whole_file(layers),
        MergeMode::SpringMerge => Some(spring_merge(layers)),
    }
}

fn whole_file(layers: &Layers<'_>) -> Option<Effective> {
    let winner = layers.tenant.or(layers.base)?;
    let source = Source {
        path: winner.path.clone(),
        rank: 1,
        hash: winner.hash,
    };
    let mut settings: Vec<EffectiveValue> = winner
        .settings
        .iter()
        .map(|s| EffectiveValue {
            path: s.path.clone(),
            value: s.value.clone(),
            origin: Origin {
                file: winner.path.clone(),
                rank: 1,
                lines: s.lines,
            },
        })
        .collect();
    settings.sort_by(|a, b| a.path.cmp(&b.path));
    Some(Effective {
        mode: MergeMode::WholeFile,
        sources: vec![source],
        settings,
        hash: winner.hash,
    })
}

/// A setting path as the steps the merge compares. The document of a multi-document file is dropped.
type Key = Vec<PathSegment>;

/// A setting held by the merge, and the layer it is from (an index into the present layers).
struct Held<'a> {
    setting: &'a FlatSetting,
    layer: usize,
    /// The path to report: the setting's own, or, if it had a document prefix, the path without it.
    path: SettingPath,
}

fn key_of(path: &SettingPath) -> (Key, SettingPath) {
    match decode_setting_path(path) {
        Some(decoded) => {
            let reported = if decoded.document.is_some() {
                SettingPath::parse(&encode_setting_path(None, &decoded.segments))
                    .unwrap_or_else(|_| path.clone())
            } else {
                path.clone()
            };
            (decoded.segments, reported)
        }
        // A path the writer could not have produced: compared as one opaque key, so it merges with nothing but itself.
        None => (vec![PathSegment::Key(path.as_str().to_owned())], path.clone()),
    }
}

fn spring_merge(layers: &Layers<'_>) -> Effective {
    let present: Vec<Layer<'_>> = [layers.tenant, layers.base, layers.root]
        .into_iter()
        .flatten()
        .collect();

    let mut held: BTreeMap<Key, Held<'_>> = BTreeMap::new();
    // Lowest layer first: each higher layer is applied over what the lower ones left.
    for (index, layer) in present.iter().enumerate().rev() {
        let keyed: Vec<(Key, SettingPath, &FlatSetting)> = layer
            .settings
            .iter()
            .map(|s| {
                let (key, reported) = key_of(&s.path);
                (key, reported, s)
            })
            .collect();

        // What this layer replaces. The lower settings are removed before the layer's own are added, so the layer never
        // removes itself.
        //
        // - `wipe`: the first index in a path says where a list is, and the list replaces everything the lower layers have
        //   below that path, items of a list or keys of a map (the binder's rule: `a[0]` above drops every `a[n]` below);
        // - `no_lists`: a key replaces the items of a list that the lower layers have at the same place (a map over a list),
        //   and leaves the other keys there alone (maps merge);
        // - a setting itself replaces a map or a list below it (a scalar over a map) and a scalar above it (a map over a
        //   scalar).
        let mut wipe: BTreeSet<Key> = BTreeSet::new();
        let mut no_lists: BTreeSet<Key> = BTreeSet::new();
        for (key, _, _) in &keyed {
            let first_index = key.iter().position(|seg| matches!(seg, PathSegment::Index(_)));
            if let Some(at) = first_index {
                if !wipe.contains(&key[..at]) {
                    wipe.insert(key[..at].to_vec());
                }
            }
            for at in 0..first_index.unwrap_or(key.len()) {
                if !no_lists.contains(&key[..at]) {
                    no_lists.insert(key[..at].to_vec());
                }
            }
            remove_below(&mut held, key);
            for n in 0..key.len() {
                held.remove(&key[..n]);
            }
        }
        for list in &wipe {
            remove_below(&mut held, list);
        }
        for map in &no_lists {
            remove_list_items(&mut held, map);
        }
        for (key, reported, setting) in keyed {
            held.insert(
                key,
                Held {
                    setting,
                    layer: index,
                    path: reported,
                },
            );
        }
    }

    let rank_of = |layer: usize| u32::try_from(layer + 1).unwrap_or(u32::MAX);
    let mut settings: Vec<EffectiveValue> = held
        .into_values()
        .map(|h| EffectiveValue {
            path: h.path,
            value: h.setting.value.clone(),
            origin: Origin {
                file: present[h.layer].path.clone(),
                rank: rank_of(h.layer),
                lines: h.setting.lines,
            },
        })
        .collect();
    settings.sort_by(|a, b| a.path.cmp(&b.path));

    let used: BTreeSet<u32> = settings.iter().map(|v| v.origin.rank).collect();
    let sources = present
        .iter()
        .enumerate()
        .filter(|(i, _)| used.contains(&rank_of(*i)))
        .map(|(i, layer)| Source {
            path: layer.path.clone(),
            rank: rank_of(i),
            hash: layer.hash,
        })
        .collect();
    let hash = settings_hash(&settings);
    Effective {
        mode: MergeMode::SpringMerge,
        sources,
        settings,
        hash,
    }
}

/// Removes every setting strictly below `key` (the keys that start with it and are longer).
fn remove_below(held: &mut BTreeMap<Key, Held<'_>>, key: &Key) {
    let doomed: Vec<Key> = held
        .range::<[PathSegment], _>((Bound::Excluded(key.as_slice()), Bound::Unbounded))
        .map(|(k, _)| k)
        .take_while(|k| k.starts_with(key))
        .cloned()
        .collect();
    for k in doomed {
        held.remove(&k);
    }
}

/// Removes the items of a list that is at `at`: the settings below it whose next step is an index.
fn remove_list_items(held: &mut BTreeMap<Key, Held<'_>>, at: &Key) {
    let doomed: Vec<Key> = held
        .range::<[PathSegment], _>((Bound::Included(at.as_slice()), Bound::Unbounded))
        .map(|(k, _)| k)
        .take_while(|k| k.starts_with(at))
        .filter(|k| matches!(k.get(at.len()), Some(PathSegment::Index(_))))
        .cloned()
        .collect();
    for k in doomed {
        held.remove(&k);
    }
}

fn type_byte(ty: ValueType) -> u8 {
    match ty {
        ValueType::String => 0,
        ValueType::Int => 1,
        ValueType::Float => 2,
        ValueType::Bool => 3,
        ValueType::Null => 4,
        ValueType::Other => 5,
    }
}

fn flag_byte(flag: Option<ValueFlag>) -> u8 {
    match flag {
        None => 0,
        Some(ValueFlag::YamlBooleanLike) => 1,
    }
}

/// The hash of merged settings: SHA-256 over a tag, the number of settings, and for each setting in path order
/// `u32_le(path length) || path || u32_le(text length) || text || type byte || flag byte`. Where a setting came from is not
/// part of it: the same settings from other files are the same effective config.
pub(crate) fn settings_hash(settings: &[EffectiveValue]) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(b"lanekeeper/effective-settings/v1\n");
    hasher.update(len32(settings.len()).to_le_bytes());
    for v in settings {
        hasher.update(len32(v.path.as_str().len()).to_le_bytes());
        hasher.update(v.path.as_str().as_bytes());
        hasher.update(len32(v.value.text.len()).to_le_bytes());
        hasher.update(v.value.text.as_bytes());
        hasher.update([type_byte(v.value.ty), flag_byte(v.value.flag)]);
    }
    ContentHash::from_bytes(hasher.finalize().into())
}

/// A length as the four bytes the encodings use. Lengths are bounded by `Limits` long before they reach `u32::MAX`.
pub(crate) fn len32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
