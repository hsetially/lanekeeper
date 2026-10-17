//! C10, unresolved placeholders (T10, D82, D87, A45): what is computed, what is cached by content, and what is read.
//!
//! A resource file's `${KEY}` is filled by the property view of the application that renders it and the tenant, or by an environment variable
//! of a Deployment that reads the file (D87). The engine's check ([`engine::c10_unresolved_placeholders`]) takes the file's text, the tenants'
//! views and the environment names; this module is the wiring, and the part that makes it cheap:
//!
//! - **The result of a file is a function of four things**: the file's path, its content (the blob), the key sets of the views it is
//!   rendered with, and the names of the environment its readers have. Each is named by a digest (`Key`), and a result is cached under it
//!   (`C10Cache`, weighed in bytes), including the common result "nothing is unresolved". A file that did not change, in a swimlane whose
//!   property sources and Deployments did not change, is never read again: a one-file change reads one blob and evaluates one file.
//! - **A property source's keys are cached by its blob**, the settings' *paths* and lines and never a value: C10 asks only whether a key
//!   exists, so nothing a value could be (a secret, D95) is ever held here.
//! - **Nothing is read before it is needed.** The views of an application are built only when one of its files has to be evaluated, the
//!   environment names of a set of readers likewise, and a resource's bytes in batches of at most `structure_batch_bytes`.
//! - **Denied files are never read.** A denied file is not a resource anyone can render (D79) and its blob is not fetched.
//!
//! The digests over-approximate what a result depends on (they name the files that *could* be property sources of an application, by name,
//! wherever they are), so a miss may happen that was not needed, and a hit is never wrong: a cached result is the result of the same inputs.
//! `tests/structure_checks.rs` holds a cold computation to a warm one after every kind of change.
//!
//! A swimlane with no tenant (nobody has adopted it) has no C10: the config-server is asked per tenant (D82), so there is no view to look a
//! key up in. A view that is over the engine's limits is skipped, with a count: an empty view would report every key as unresolved.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{
    AppName, ChannelName, ContentHash, FileClass, FileRole, LineRange, NfsPath, SettingPath, SettingValue,
    SwimlaneId, TenantId, TenantSet, ValueType,
};
use engine::{
    CheckResult, DenyMatcher, EnvNames, FlatSetting, Limits, PlaceholderFile, PropertyFile, PropertyView,
    Syntax, TenantView, c10_unresolved_placeholders, decode_text, index_settings, property_view,
    search_locations,
};
use moka::sync::Cache;
use ports::{BlobStore, MAX_GET_MANY};
use tracing::warn;

use super::Digest32;
use super::inputs::{self, FileRow, ServiceRow};
use crate::config::RegistryConfig;
use crate::db::Db;
use crate::denied::is_denied;
use crate::error::RegistryError;
use crate::pickup::consumers::{Directory, is_read};
use crate::read::resource::{application_of, property_names};

/// What a cached result is named by: the file, its content, and the digests of the views and of the environment it was evaluated with.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    path: NfsPath,
    blob: ContentHash,
    views: [u8; 32],
    env: [u8; 32],
}

/// The keys of a property source: the paths and lines of its settings, or `None` for a blob that does not parse. Never a value.
type Keys = Arc<Option<Vec<(SettingPath, LineRange)>>>;

/// The caches of C10, shared by every swimlane: two files of two swimlanes with the same path, content, views and environment have one
/// result. Bounded by `structure_cache_bytes` (the weight of an entry is what it holds, with a floor for the map).
pub(crate) struct C10Cache {
    results: Cache<Key, Option<Arc<CheckResult>>>,
    keys: Cache<ContentHash, Keys>,
}

impl std::fmt::Debug for C10Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("C10Cache").finish_non_exhaustive()
    }
}

fn weight(bytes: usize) -> u32 {
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

impl C10Cache {
    pub(crate) fn new(config: &RegistryConfig) -> Self {
        // Two thirds of the budget for results (one per resource file), the rest for the property sources' keys.
        let total = config.structure_cache_bytes;
        Self {
            results: Cache::builder()
                .max_capacity(total / 3 * 2)
                .weigher(|k: &Key, v: &Option<Arc<CheckResult>>| {
                    weight(
                        160 + k.path.as_str().len()
                            + v.as_ref()
                                .map_or(0, |r| 256 + r.message.as_str().len() + 64 * r.locations.len()),
                    )
                })
                .build(),
            keys: Cache::builder()
                .max_capacity(total / 3)
                .weigher(|_, v: &Keys| {
                    weight(
                        64 + v
                            .as_ref()
                            .as_ref()
                            .map_or(0, |keys| keys.iter().map(|(p, _)| 48 + p.as_str().len()).sum()),
                    )
                })
                .build(),
        }
    }

    /// Drop everything: for the test that compares a cold computation with a warm one, and for a caller that wants to start over.
    pub(crate) fn clear(&self) {
        self.results.invalidate_all();
        self.keys.invalidate_all();
    }
}

/// What C10 needs from the rest of the computation.
pub(crate) struct Ctx<'a> {
    pub db: &'a Db,
    pub blobs: &'a dyn BlobStore,
    pub cache: &'a C10Cache,
    pub limits: &'a Limits,
    pub deny: &'a DenyMatcher,
    pub blob_timeout: Duration,
    pub batch_bytes: u64,
}

/// What C10 is computed from, for one swimlane.
pub(crate) struct Input<'a> {
    pub swimlane: &'a SwimlaneId,
    pub files: &'a [FileRow],
    pub tenants: &'a TenantSet,
    /// The application names of the swimlane's Deployments, sorted, without repeats.
    pub apps: &'a [AppName],
    pub services: &'a [ServiceRow],
    pub directory: &'a Directory,
    pub channels: &'a [ChannelName],
}

/// What a C10 computation did, in counts (S10: no name, no path).
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Findings, sorted by path.
    pub results: Vec<CheckResult>,
    /// Resource files that C10 looked at.
    pub files: usize,
    /// Files whose result was evaluated now, and files whose result came from the cache.
    pub evaluated: usize,
    pub cached: usize,
    /// Blobs read from the blob store (resources that were evaluated, property sources that were not cached).
    pub blobs_read: usize,
}

/// A resource file that C10 looks at, and what its result depends on besides its content.
struct Candidate {
    /// Index in `Input::files`.
    at: usize,
    app: AppName,
    readers: Readers,
}

/// The services that read a file: every service (a root file, or a file that no folder or mapping names a reader of: any service may ask
/// for it by name), or these.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Readers {
    Every,
    Some(Vec<usize>),
}

/// A file whose result has to be evaluated.
struct Miss {
    candidate: usize,
    key: Key,
}

/// The digest of an application's views: the property sources that could be in its view (by name, in any folder), the channels (they decide
/// which folders are searched) and the tenants. A change to any of them is a new digest.
fn views_digest(app: &AppName, input: &Input<'_>, by_name: &BTreeMap<&str, Vec<usize>>) -> [u8; 32] {
    let mut d = Digest32::new("c10-views");
    d.field(app.as_str().as_bytes());
    for tenant in input.tenants.iter() {
        d.field(tenant.as_str().as_bytes());
    }
    d.field(b"channels");
    for channel in input.channels {
        d.field(channel.as_str().as_bytes());
    }
    d.field(b"sources");
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    for tenant in input.tenants.iter() {
        for name in property_names(app, tenant) {
            for &at in by_name.get(name.as_str()).map_or(&[][..], Vec::as_slice) {
                seen.insert(at);
            }
        }
    }
    for at in seen {
        let file = &input.files[at];
        d.field(file.path.as_str().as_bytes());
        d.field(file.blob.as_bytes());
    }
    d.finish()
}

/// The digest of the environment names a set of readers has: the digests of their names, as a set (two Deployments with the same names are
/// one, and the order does not matter).
fn env_digest(readers: &Readers, services: &[ServiceRow]) -> [u8; 32] {
    let mut names: BTreeSet<&str> = BTreeSet::new();
    match readers {
        Readers::Every => names.extend(services.iter().map(|s| s.names.as_str())),
        Readers::Some(list) => names.extend(
            list.iter()
                .filter_map(|&i| services.get(i))
                .map(|s| s.names.as_str()),
        ),
    }
    let mut d = Digest32::new("c10-env");
    for n in names {
        d.field(n.as_bytes());
    }
    d.finish()
}

/// The settings' keys of a property source's bytes, read as `syntax`: the paths and lines, never a value.
pub(crate) fn keys_of(
    bytes: &[u8],
    syntax: Syntax,
    limits: &Limits,
) -> Option<Vec<(SettingPath, LineRange)>> {
    let index = index_settings(bytes, syntax, limits).ok()?;
    Some(index.rows.into_iter().map(|r| (r.path, r.lines)).collect())
}

/// Settings with no value, for a view that is only asked whether a key is there.
fn bare(keys: &[(SettingPath, LineRange)]) -> Vec<FlatSetting> {
    keys.iter()
        .map(|(path, lines)| FlatSetting {
            path: path.clone(),
            value: SettingValue {
                text: String::new(),
                ty: ValueType::String,
                flag: None,
            },
            lines: *lines,
            via_alias: false,
        })
        .collect()
}

/// A blob batch: hashes with the size the registry knows for each, cut so that a batch holds at most `batch_bytes` (and the store's limit
/// of hashes). A blob over the budget goes alone.
fn batches(wanted: &[(ContentHash, u64)], batch_bytes: u64) -> Vec<Vec<ContentHash>> {
    let mut out: Vec<Vec<ContentHash>> = Vec::new();
    let mut current: Vec<ContentHash> = Vec::new();
    let mut bytes = 0u64;
    for (hash, size) in wanted {
        if !current.is_empty() && (bytes.saturating_add(*size) > batch_bytes || current.len() >= MAX_GET_MANY)
        {
            out.push(std::mem::take(&mut current));
            bytes = 0;
        }
        current.push(*hash);
        bytes = bytes.saturating_add(*size);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Fetch a batch of blobs under the blob timeout.
async fn fetch(ctx: &Ctx<'_>, batch: &[ContentHash]) -> Result<HashMap<ContentHash, Bytes>, RegistryError> {
    let got = tokio::time::timeout(ctx.blob_timeout, ctx.blobs.get_many(batch))
        .await
        .map_err(|_| RegistryError::Timeout)??;
    Ok(got.into_iter().collect())
}

/// The views of an application, one for each tenant of the swimlane, built from the cached keys of the property sources (which are read
/// and cached first). `None` when a view is over the engine's limits.
async fn views(
    ctx: &Ctx<'_>,
    input: &Input<'_>,
    app: &AppName,
    by_name: &BTreeMap<&str, Vec<usize>>,
    paths: &[NfsPath],
    out: &mut Outcome,
) -> Result<Option<Vec<(TenantId, PropertyView)>>, RegistryError> {
    // The property sources this application can read, for every tenant.
    let mut sources: BTreeSet<usize> = BTreeSet::new();
    for tenant in input.tenants.iter() {
        for name in property_names(app, tenant) {
            sources.extend(by_name.get(name.as_str()).into_iter().flatten().copied());
        }
    }
    // The ones whose keys are not cached are read, in batches, and parsed.
    let missing: Vec<(ContentHash, u64)> = sources
        .iter()
        .map(|&at| (input.files[at].blob, input.files[at].size))
        .filter(|(blob, _)| ctx.cache.keys.get(blob).is_none())
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect();
    let syntax_of: BTreeMap<ContentHash, Syntax> = sources
        .iter()
        .filter_map(|&at| Some((input.files[at].blob, Syntax::of_path(&input.files[at].path)?)))
        .collect();
    for batch in batches(&missing, ctx.batch_bytes) {
        let got = fetch(ctx, &batch).await?;
        out.blobs_read += got.len();
        for blob in &batch {
            let keys = match (got.get(blob), syntax_of.get(blob)) {
                (Some(bytes), Some(syntax)) => keys_of(bytes, *syntax, ctx.limits),
                // A blob the store does not have is a source with no settings, as the pipeline treats it (a file whose content is missing).
                _ => None,
            };
            ctx.cache.keys.insert(*blob, Arc::new(keys));
        }
    }
    let mut built = Vec::with_capacity(input.tenants.len());
    for tenant in input.tenants.iter() {
        let locations = search_locations(app, Some(tenant), paths, input.channels);
        let names = property_names(app, tenant);
        let chosen: BTreeSet<usize> = names
            .iter()
            .flat_map(|name| by_name.get(name.as_str()).into_iter().flatten().copied())
            .collect();
        let settings: Vec<(usize, Vec<FlatSetting>)> = chosen
            .iter()
            .map(|&at| {
                let keys = ctx.cache.keys.get(&input.files[at].blob);
                let list = keys
                    .as_deref()
                    .and_then(Option::as_ref)
                    .map_or_else(Vec::new, |k| bare(k));
                (at, list)
            })
            .collect();
        let files: Vec<PropertyFile<'_>> = settings
            .iter()
            .map(|(at, list)| PropertyFile {
                path: &input.files[*at].path,
                settings: list,
            })
            .collect();
        let Ok(view) = property_view(app, Some(tenant), &locations, &files, ctx.limits) else {
            warn!("a property view is over its limits: its files are not checked for placeholders");
            return Ok(None);
        };
        built.push((tenant.clone(), view));
    }
    Ok(Some(built))
}

/// The environment names the readers have, as the engine takes them (names only).
async fn environment(ctx: &Ctx<'_>, input: &Input<'_>, readers: &Readers) -> Result<EnvNames, RegistryError> {
    let ids: Vec<i64> = match readers {
        Readers::Every => input.services.iter().map(|s| s.id).collect(),
        Readers::Some(list) => list
            .iter()
            .filter_map(|&i| input.services.get(i))
            .map(|s| s.id)
            .collect(),
    };
    let lists = inputs::env_names(ctx.db, input.swimlane, &ids).await?;
    Ok(EnvNames::new(lists.iter().flatten().map(String::as_str)))
}

/// One file's result, from its bytes and the views and environment it is evaluated with.
fn evaluate(
    path: &NfsPath,
    bytes: &Bytes,
    views: &[TenantView<'_>],
    env: &EnvNames,
    limits: &Limits,
) -> Option<Arc<CheckResult>> {
    // A file that is not text under its own declaration has no placeholders to read.
    let decoded = decode_text(bytes, limits).ok()?;
    let plain = PlaceholderFile {
        path,
        text: &decoded.text,
        settings: None,
    };
    let first = c10_unresolved_placeholders(&[plain], views, env, limits)
        .into_iter()
        .next()?;
    // A file that has a finding is read once more, as a structured file, so that the finding points at the settings that hold the
    // placeholder. The settings are transient: they are dropped here and never cached or logged.
    if let Some(syntax) = Syntax::of_path(path) {
        if let Ok(index) = index_settings(bytes, syntax, limits) {
            let flat: Vec<FlatSetting> = index
                .rows
                .into_iter()
                .map(|r| FlatSetting {
                    path: r.path,
                    value: r.value,
                    lines: r.lines,
                    via_alias: false,
                })
                .collect();
            let with = PlaceholderFile {
                path,
                text: &decoded.text,
                settings: Some(&flat),
            };
            if let Some(better) = c10_unresolved_placeholders(&[with], views, env, limits)
                .into_iter()
                .next()
            {
                return Some(Arc::new(better));
            }
        }
    }
    Some(Arc::new(first))
}

/// C10 for a swimlane.
pub(crate) async fn run(ctx: &Ctx<'_>, input: &Input<'_>) -> Result<Outcome, RegistryError> {
    let mut out = Outcome::default();
    if input.tenants.is_empty() {
        return Ok(out);
    }
    // The property sources by file name, for the views' digests and the views themselves.
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (at, file) in input.files.iter().enumerate() {
        if file.class == FileClass::Structured && !is_denied(ctx.deny, &file.path, file.denied, file.class) {
            by_name.entry(file.path.file_name()).or_default().push(at);
        }
    }

    // The resource files, who reads each, and the key of each one's result.
    let mut candidates: Vec<Candidate> = Vec::new();
    for (at, file) in input.files.iter().enumerate() {
        if file.role != FileRole::Resource
            || !matches!(file.class, FileClass::Text | FileClass::Structured)
            || is_denied(ctx.deny, &file.path, file.denied, file.class)
            || !is_read(&file.path, &file.logical, input.tenants)
        {
            continue;
        }
        let Some(app) = application_of(&file.logical, input.apps) else {
            continue;
        };
        let readers = match input.directory.readers(&file.path, file.role, input.tenants) {
            Cow::Borrowed(_) => Readers::Every,
            Cow::Owned(list) if list.is_empty() => Readers::Every,
            Cow::Owned(list) => Readers::Some(list),
        };
        candidates.push(Candidate { at, app, readers });
    }
    out.files = candidates.len();

    let mut view_digests: HashMap<AppName, [u8; 32]> = HashMap::new();
    let mut env_digests: HashMap<Readers, [u8; 32]> = HashMap::new();
    let mut results: Vec<Option<Arc<CheckResult>>> = vec![None; candidates.len()];
    let mut misses: Vec<Miss> = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        let file = &input.files[c.at];
        let views_fp = *view_digests
            .entry(c.app.clone())
            .or_insert_with(|| views_digest(&c.app, input, &by_name));
        let env_fp = *env_digests
            .entry(c.readers.clone())
            .or_insert_with(|| env_digest(&c.readers, input.services));
        let key = Key {
            path: file.path.clone(),
            blob: file.blob,
            views: views_fp,
            env: env_fp,
        };
        match ctx.cache.results.get(&key) {
            Some(cached) => {
                out.cached += 1;
                results[i] = cached;
            }
            None => misses.push(Miss { candidate: i, key }),
        }
    }

    if !misses.is_empty() {
        evaluate_misses(ctx, input, &candidates, &misses, &by_name, &mut results, &mut out).await?;
    }

    let mut found: Vec<CheckResult> = results.into_iter().flatten().map(|r| (*r).clone()).collect();
    found.sort_by(|a, b| (&a.path, &a.message).cmp(&(&b.path, &b.message)));
    out.results = found;
    Ok(out)
}

/// Evaluate the files whose result is not cached: the views and the environments they need, then their bytes in batches.
async fn evaluate_misses(
    ctx: &Ctx<'_>,
    input: &Input<'_>,
    candidates: &[Candidate],
    misses: &[Miss],
    by_name: &BTreeMap<&str, Vec<usize>>,
    results: &mut [Option<Arc<CheckResult>>],
    out: &mut Outcome,
) -> Result<(), RegistryError> {
    let paths: Vec<NfsPath> = input.files.iter().map(|f| f.path.clone()).collect();
    // The views of every application that has a miss, and the environment of every set of readers that has one.
    let mut app_views: HashMap<AppName, Option<Vec<(TenantId, PropertyView)>>> = HashMap::new();
    let mut envs: HashMap<Readers, EnvNames> = HashMap::new();
    for miss in misses {
        let c = &candidates[miss.candidate];
        if !app_views.contains_key(&c.app) {
            let built = views(ctx, input, &c.app, by_name, &paths, out).await?;
            app_views.insert(c.app.clone(), built);
        }
        if !envs.contains_key(&c.readers) {
            let env = environment(ctx, input, &c.readers).await?;
            envs.insert(c.readers.clone(), env);
        }
    }
    // The bytes of the misses, in batches.
    let wanted: Vec<(ContentHash, u64)> = misses
        .iter()
        .map(|m| {
            let f = &input.files[candidates[m.candidate].at];
            (f.blob, f.size)
        })
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect();
    let mut by_blob: HashMap<ContentHash, Vec<usize>> = HashMap::new();
    for (i, miss) in misses.iter().enumerate() {
        by_blob.entry(miss.key.blob).or_default().push(i);
    }
    for batch in batches(&wanted, ctx.batch_bytes) {
        let got = fetch(ctx, &batch).await?;
        out.blobs_read += got.len();
        for blob in &batch {
            for &i in by_blob.get(blob).map_or(&[][..], Vec::as_slice) {
                let miss = &misses[i];
                let c = &candidates[miss.candidate];
                let file = &input.files[c.at];
                let result = match (got.get(blob), app_views.get(&c.app), envs.get(&c.readers)) {
                    (Some(bytes), Some(Some(built)), Some(env)) => {
                        let tenant_views: Vec<TenantView<'_>> = built
                            .iter()
                            .map(|(tenant, view)| TenantView { tenant, view })
                            .collect();
                        out.evaluated += 1;
                        let result = evaluate(&file.path, bytes, &tenant_views, env, ctx.limits);
                        ctx.cache.results.insert(miss.key.clone(), result.clone());
                        result
                    }
                    // A view over its limits, or a blob the store does not have: nothing is said about the file, and nothing is cached,
                    // so the next computation tries again.
                    _ => None,
                };
                results[miss.candidate] = result;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(n: u8) -> ContentHash {
        ContentHash::from_bytes([n; 32])
    }

    #[test]
    fn blobs_are_read_in_batches_that_hold_at_most_the_byte_budget_and_the_stores_limit() {
        let wanted: Vec<(ContentHash, u64)> = (0..10).map(|i| (hash(i), 4)).collect();
        let b = batches(&wanted, 10);
        assert_eq!(
            b.iter().map(Vec::len).collect::<Vec<_>>(),
            [2, 2, 2, 2, 2],
            "two blobs of 4 fit in 10, three do not"
        );
        // A blob over the budget goes alone, and is not dropped.
        let big = vec![(hash(1), 100), (hash(2), 1), (hash(3), 1)];
        let b = batches(&big, 10);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), [1, 2]);
        // The store's limit of hashes holds whatever the sizes.
        let many: Vec<(ContentHash, u64)> = (0..=MAX_GET_MANY)
            .map(|i| (hash(u8::try_from(i % 251).unwrap_or(0)), 0))
            .collect();
        let b = batches(&many, 1 << 40);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].len(), MAX_GET_MANY);
        assert!(batches(&[], 10).is_empty());
    }

    #[test]
    fn the_digests_follow_what_a_result_depends_on_and_nothing_else() {
        let svc = |id: i64, names: &str| ServiceRow {
            id,
            namespace: "ns".to_owned(),
            name: format!("svc{id}"),
            app: None,
            names: names.to_owned(),
        };
        let services = vec![svc(1, "aaa"), svc(2, "bbb"), svc(3, "aaa")];
        let every = env_digest(&Readers::Every, &services);
        // Two Deployments with the same names are one set of names; the ids and the order do not matter.
        assert_eq!(
            env_digest(&Readers::Some(vec![0, 1]), &services),
            env_digest(&Readers::Some(vec![2, 1]), &services)
        );
        assert_eq!(env_digest(&Readers::Some(vec![0, 1]), &services), every);
        assert_ne!(env_digest(&Readers::Some(vec![0]), &services), every);
        let other = vec![svc(1, "aaa"), svc(2, "ccc")];
        assert_ne!(env_digest(&Readers::Every, &other), every);
        // A byte moved from one field to the next is another digest.
        let mut a = Digest32::new("t");
        a.field(b"ab");
        a.field(b"c");
        let mut b = Digest32::new("t");
        b.field(b"a");
        b.field(b"bc");
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn the_keys_of_a_property_source_hold_paths_and_lines_and_never_a_value() {
        let keys = keys_of(
            b"server:\n  port: 8080\n  secret: hunter2-not-for-logs\n",
            Syntax::Yaml,
            &Limits::default(),
        )
        .unwrap();
        let names: Vec<&str> = keys.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["server.port", "server.secret"]);
        let bare = bare(&keys);
        assert!(bare.iter().all(|s| s.value.text.is_empty()), "no value is kept");
        assert!(format!("{keys:?}").find("hunter2").is_none());
        assert!(keys_of(b"a: [unclosed", Syntax::Yaml, &Limits::default()).is_none());
    }

    #[test]
    fn a_file_that_has_a_finding_points_at_the_settings_that_hold_the_placeholder() {
        let path = NfsPath::parse("svc/app.yml").unwrap();
        let bytes = Bytes::from_static(b"name: x\nurl: ${MISSING_KEY}/api\nother: y\n");
        let tenant = TenantId::parse("sit1").unwrap();
        let view = PropertyView::default();
        let views = [TenantView {
            tenant: &tenant,
            view: &view,
        }];
        let found = evaluate(&path, &bytes, &views, &EnvNames::default(), &Limits::default()).unwrap();
        assert_eq!(found.locations.len(), 1);
        assert_eq!(found.locations[0].path.as_str(), "url");
        assert_eq!(found.locations[0].lines.start(), 2);
        // The environment fills it: no result.
        let env = EnvNames::new(["MISSING_KEY"]);
        assert!(evaluate(&path, &bytes, &views, &env, &Limits::default()).is_none());
    }
}
