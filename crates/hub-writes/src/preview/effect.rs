//! `preview_effect` (D46): what an upload or a delete would do to what the services read, before anybody confirms.
//!
//! The facts come from the engine (rule 7): `effect_preview` compares the effective config before and after, and this module
//! gathers what it needs and puts its answer on the wire.
//!
//! - **Gathering.** The tenants, the path mapping and the application names are the swimlane's own (plain, bounded reads); the files
//!   are the layers of the logical file as the registry has them now: the tenant file, the base file and, for a property source,
//!   the root `application.*`. A swimlane has a set of tenants (Q34): a tenant file is judged for its own tenant, a base file for
//!   the first tenant of the set, as the effective hashes of the registry are (A30).
//! - **The new content** is put into the form of the file it replaces first (D17), so the preview is of the bytes that would be
//!   written.
//! - **Masking.** A value the secret scan flags is replaced by its marker in the answer, whichever side of the change holds it
//!   and whichever file it comes from (S13, S14b). A file that cannot be scanned is masked whole: the safe answer to "might
//!   this be a secret" is yes.
//! - **The token** is issued last, bound to what was asked, by `super::token`.
//!
//! Everything CPU-bound runs on the blocking pool (rule 4).

use std::collections::BTreeMap;

use bytes::Bytes;
use domain::{
    AppName, ChangePreview, ContentSource, FileClass, LineRange, MergeMode, NfsPath, PathMappingRule,
    PreviewChange, PreviewRequest, PreviewScope, PreviewWarning, PreviewedChange, RepoPath, Role,
    SettingPath, SettingValue, SwimlaneId, TenantId, TenantSet, Timestamp, User, WriteCtx,
};
use engine::{
    DenyMatcher, EffectChange, EffectInput, EffectScope, EffectWarning, FileFacts, FlatSetting, Layer,
    Layers, Limits, Newline, ScanConfig, SecretMatch, Syntax, classify, classify_path, decode_text,
    effect_preview, effective, file_role, flatten, merge_mode, normalised_hash, parse, secret_scan,
};
use ports::{ReadError, WriteError, content_hash};

use super::token::{Binding, PreviewAction, issue};
use crate::error::{Internal, Res};
use crate::fidelity::{Basis, folder_eol_style, prepare_content};
use crate::pipeline::authorize::authorize;
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::pipeline::validate::Old;
use crate::service::Inner;

const TENANTS: &str =
    "SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = $1 ORDER BY tenant_id LIMIT $2";

const ROOTS: &str = "SELECT repo, repo_root FROM path_mappings WHERE repo IN ('base', 'tenant') LIMIT 2";

/// The application names the swimlane's Deployments declare (D88), which decide which files are property sources (D82).
const APP_NAMES: &str = "SELECT DISTINCT env_values ->> 'SPRING_APPLICATION_NAME' FROM services \
     WHERE swimlane_id = $1 AND removed_at IS NULL AND env_values ? 'SPRING_APPLICATION_NAME' LIMIT $2";

/// The most application names read for one preview.
const MAX_APP_NAMES: i64 = 5_000;

/// What the swimlane's own configuration says about paths. The pull-request path reads it the same way (T6).
pub(crate) struct Facts {
    pub rule: PathMappingRule,
    pub tenants: TenantSet,
    pub apps: Vec<AppName>,
}

pub(crate) async fn facts(hub: &Inner, swimlane: &SwimlaneId) -> Res<Facts> {
    let tenants: Vec<String> = hub
        .within(
            "preview_tenants",
            sqlx::query_scalar(TENANTS)
                .bind(swimlane.as_str())
                .bind(i64::try_from(TenantSet::MAX_TENANTS).unwrap_or(128))
                .fetch_all(&hub.pool),
        )
        .await?
        .map_err(|e| Internal::db("preview_tenants", &e))?;
    let roots: Vec<(String, String)> = hub
        .within("preview_roots", sqlx::query_as(ROOTS).fetch_all(&hub.pool))
        .await?
        .map_err(|e| Internal::db("preview_roots", &e))?;
    let apps: Vec<Option<String>> = hub
        .within(
            "preview_apps",
            sqlx::query_scalar(APP_NAMES)
                .bind(swimlane.as_str())
                .bind(MAX_APP_NAMES)
                .fetch_all(&hub.pool),
        )
        .await?
        .map_err(|e| Internal::db("preview_apps", &e))?;

    // The configured roots; the default for a root nobody configured.
    let default = PathMappingRule::default();
    let (mut base, mut tenant) = (default.base_root().clone(), default.tenant_root().clone());
    for (repo, root) in roots {
        let Ok(root) = RepoPath::parse(&root) else {
            continue;
        };
        match repo.as_str() {
            "base" => base = root,
            "tenant" => tenant = root,
            _ => {}
        }
    }
    Ok(Facts {
        rule: PathMappingRule::new(base, tenant),
        // A tenant id the table holds that is not a valid id is left out, not guessed at.
        tenants: TenantSet::new(tenants.iter().filter_map(|t| TenantId::parse(t).ok()))
            .map_err(|_| Internal::new("preview_tenants", "too many tenants"))?,
        apps: apps
            .into_iter()
            .flatten()
            .filter_map(|a| AppName::parse(&a).ok())
            .collect(),
    })
}

/// The layer files of a logical file, and which of them the target is.
struct Plan {
    facts: FileFacts,
    mode: MergeMode,
    /// The tenant file of the tenant the preview is for, when the swimlane has one.
    tenant: Option<NfsPath>,
    base: NfsPath,
    /// The root `application.*` file a property source merges over.
    root: Option<NfsPath>,
}

fn plan(f: &Facts, target: &NfsPath) -> Result<Plan, WriteError> {
    let facts = classify_path(&f.rule, target, &f.tenants, |_| true).map_err(|_| WriteError::BadRequest)?;
    let mode = merge_mode(file_role(target, &f.tenants, &f.apps));
    let base = facts.logical.as_path().clone();
    let view = facts.tenant.clone().or_else(|| f.tenants.iter().next().cloned());
    let tenant = view.and_then(|t| {
        let repo = RepoPath::parse(&format!("{}/{}", f.rule.tenant_root().as_str(), base.as_str())).ok()?;
        f.rule.tenant_to_nfs(&repo, &t).ok()
    });
    // The root layer is the lowest layer of a property source. The root file itself has none below it.
    let root = (mode == MergeMode::SpringMerge)
        .then(|| {
            let name = base.file_name();
            let ext = name.rsplit_once('.').map(|(_, e)| e)?;
            NfsPath::parse(&format!("application.{ext}")).ok()
        })
        .flatten()
        .filter(|r| *r != base);
    Ok(Plan {
        facts,
        mode,
        tenant,
        base,
        root,
    })
}

/// What the registry has at each path now. `None`: the file is not there (or may not be read).
async fn read_layers(
    hub: &Inner,
    user: &User,
    swimlane: &SwimlaneId,
    paths: &[&NfsPath],
) -> Result<BTreeMap<NfsPath, Option<Bytes>>, WriteError> {
    let mut out = BTreeMap::new();
    for path in paths {
        if out.contains_key(*path) {
            continue;
        }
        let read = hub
            .within(
                "preview_layer",
                hub.registry.content(user, swimlane, path, ContentSource::Nfs),
            )
            .await?;
        let bytes = match read {
            Ok(fc) if fc.withheld => None,
            Ok(fc) => fc.bytes,
            Err(ReadError::NotFound) => None,
            Err(_) => return Err(WriteError::Unavailable),
        };
        out.insert((*path).clone(), bytes);
    }
    Ok(out)
}

/// A layer file, read: its class, hash, flattened settings and the secret scan of its text.
struct Loaded {
    path: NfsPath,
    hash: domain::ContentHash,
    settings: Vec<FlatSetting>,
    scan: Scan,
}

/// The secret scan of one file.
enum Scan {
    Clean,
    Matches(Vec<SecretMatch>),
    /// The file could not be scanned: everything in it is masked.
    Unscannable,
}

impl Scan {
    /// The marker for a value written on `lines` of the file, if the scan flags it.
    fn flags(&self, lines: LineRange) -> Option<String> {
        match self {
            Self::Clean => None,
            Self::Unscannable => Some("[REDACTED: the file could not be scanned]".to_owned()),
            Self::Matches(found) => found
                .iter()
                .find(|m| m.line >= lines.start() && m.line <= lines.end())
                .map(SecretMatch::marker),
        }
    }
}

fn load(path: &NfsPath, bytes: &Bytes, deny: &DenyMatcher, limits: &Limits) -> Loaded {
    let class = classify(path, Some(bytes), deny);
    let mut settings = Vec::new();
    let mut scan = Scan::Clean;
    if !matches!(class, FileClass::Binary | FileClass::Denied) {
        match decode_text(bytes, limits) {
            Ok(decoded) => {
                scan = match secret_scan(&decoded.text, &ScanConfig::default(), limits) {
                    Ok(found) if found.is_empty() => Scan::Clean,
                    Ok(found) => Scan::Matches(found),
                    Err(_) => Scan::Unscannable,
                };
                if class == FileClass::Structured {
                    if let Some(syntax) = Syntax::of_path(path) {
                        // A file that does not parse has no settings to compare; the upload that replaces it is judged on its own.
                        if let Ok(parsed) = parse(&decoded.text, syntax, limits) {
                            if let Ok(flat) = flatten(&parsed, limits) {
                                settings = flat.settings;
                            }
                        }
                    }
                }
            }
            Err(_) => scan = Scan::Unscannable,
        }
    }
    Loaded {
        path: path.clone(),
        hash: normalised_hash(bytes, class),
        settings,
        scan,
    }
}

/// Everything the blocking part needs.
struct Work {
    plan: Plan,
    tenants: TenantSet,
    /// The layer files as they are now.
    now: BTreeMap<NfsPath, Option<Bytes>>,
    target: NfsPath,
    change: PreviewChange,
    folder_style: Option<Newline>,
    folder_exists: bool,
}

/// What `compute` found.
struct Computed {
    scope: PreviewScope,
    warnings: Vec<PreviewWarning>,
    changes: Vec<PreviewedChange>,
    changes_total: usize,
    effective_changed: bool,
}

fn layer_of(loaded: &Loaded) -> Layer<'_> {
    Layer {
        path: &loaded.path,
        hash: loaded.hash,
        settings: &loaded.settings,
    }
}

fn compute(work: &Work, deny: &DenyMatcher, limits: &Limits) -> Result<Computed, WriteError> {
    let Work {
        plan,
        tenants,
        now,
        target,
        change,
        folder_style,
        folder_exists,
    } = work;
    let target_is_tenant = plan.facts.tenant.is_some();

    // The layers as they are now.
    let mut loaded: BTreeMap<NfsPath, Loaded> = BTreeMap::new();
    for (path, bytes) in now {
        if let Some(bytes) = bytes {
            loaded.insert(path.clone(), load(path, bytes, deny, limits));
        }
    }

    // The target's new content, in the form of the file it replaces.
    let new_loaded: Option<Loaded> = match change {
        PreviewChange::Delete { .. } => None,
        PreviewChange::Upload { content, .. } => {
            let class = classify(target, Some(content), deny);
            let basis = match now.get(target).and_then(Option::as_ref) {
                Some(old) => Basis::of(&Old::Known(old.clone()), None),
                None => Basis::of(&Old::Absent, *folder_style),
            };
            let prepared =
                prepare_content(content, class, basis, None, limits).map_err(|_| WriteError::BadRequest)?;
            if prepared.len() > limits.input_bytes {
                return Err(WriteError::BadRequest);
            }
            Some(load(target, &prepared, deny, limits))
        }
    };

    let slot = |path: &Option<NfsPath>| path.as_ref().and_then(|p| loaded.get(p)).map(layer_of);
    let layers = Layers {
        tenant: slot(&plan.tenant),
        base: loaded.get(&plan.base).map(layer_of),
        root: slot(&plan.root),
    };
    let input = EffectInput {
        mode: plan.mode,
        target: &plan.facts,
        tenants,
        layers,
        target_folder_exists: *folder_exists,
        change: match &new_loaded {
            Some(l) => EffectChange::Write(layer_of(l)),
            None => EffectChange::Delete,
        },
    };
    let preview = effect_preview(&input, limits);

    // After the change, for the masks: which file each side of a change is from.
    let mut after = layers;
    let replaced = new_loaded.as_ref().map(layer_of);
    if target_is_tenant {
        after.tenant = replaced;
    } else {
        after.base = replaced;
    }
    let before_at = origins(effective(plan.mode, &layers));
    let after_at = origins(effective(plan.mode, &after));
    let scan_before = |file: &NfsPath| loaded.get(file).map(|l| &l.scan);
    let scan_after = |file: &NfsPath| match &new_loaded {
        Some(l) if file == target => Some(&l.scan),
        _ => loaded.get(file).map(|l| &l.scan),
    };

    let changes = preview
        .changes
        .into_iter()
        .map(|mut c| {
            let left = mask(&mut c.left, &c.path, &before_at, &scan_before);
            let right = mask(&mut c.right, &c.path, &after_at, &scan_after);
            PreviewedChange {
                change: c,
                flagged: left || right,
            }
        })
        .collect();

    Ok(Computed {
        scope: match preview.scope {
            EffectScope::EveryService => PreviewScope::EveryService,
            EffectScope::Application(app) => PreviewScope::Application { app },
            EffectScope::Folder(path) => PreviewScope::Folder { path },
        },
        warnings: preview.warnings.into_iter().map(warning).collect(),
        changes,
        changes_total: preview.changes_total,
        effective_changed: preview.effective_changed,
    })
}

type Origins = BTreeMap<SettingPath, (NfsPath, LineRange)>;

/// Where each effective setting is written: the file and the lines.
fn origins(effective: Option<engine::Effective>) -> Origins {
    effective
        .map(|e| {
            e.settings
                .into_iter()
                .map(|v| (v.path, (v.origin.file, v.origin.lines)))
                .collect()
        })
        .unwrap_or_default()
}

/// Replace the value of one side of a change by a marker if the scan flags the place it is written (S13, S14b). A value whose
/// place is not known is masked too: a preview never guesses that a value is harmless.
fn mask<'a>(
    side: &mut Option<SettingValue>,
    path: &SettingPath,
    at: &Origins,
    scans: &dyn Fn(&NfsPath) -> Option<&'a Scan>,
) -> bool {
    let Some(value) = side else { return false };
    let marker = match at.get(path) {
        Some((file, lines)) => scans(file).and_then(|scan| scan.flags(*lines)),
        None => Some("[REDACTED: the source of the value is not known]".to_owned()),
    };
    match marker {
        Some(marker) => {
            value.text = marker;
            true
        }
        None => false,
    }
}

fn warning(w: EffectWarning) -> PreviewWarning {
    match w {
        EffectWarning::FallsBackToBase => PreviewWarning::FallsBackToBase,
        EffectWarning::NoFileLeft => PreviewWarning::NoFileLeft,
        EffectWarning::ForksFromBase => PreviewWarning::ForksFromBase,
        EffectWarning::NeverRead { suffix } => PreviewWarning::NeverRead { suffix },
        EffectWarning::NeedsNotifyOrRestart => PreviewWarning::NeedsNotifyOrRestart,
        EffectWarning::NewFolderNeedsConfigServerRestart => PreviewWarning::NewFolderNeedsConfigServerRestart,
        EffectWarning::ChannelsYmlNeedsConfigServerRestart => {
            PreviewWarning::ChannelsYmlNeedsConfigServerRestart
        }
    }
}

/// Does the folder of `path` exist, and what line endings do its files have? The root always exists. A registry that does not
/// answer is no reason to warn about a new folder.
async fn folder_facts(
    hub: &Inner,
    user: &User,
    swimlane: &SwimlaneId,
    path: &NfsPath,
) -> (bool, Option<Newline>) {
    let parent = path.parent();
    let Ok(page) = domain::Page::new(domain::Page::MAX_LIMIT, None) else {
        return (true, None);
    };
    match hub
        .within("preview_folder", hub.registry.tree(user, swimlane, &parent, page))
        .await
    {
        Ok(Ok(listed)) => (
            parent.is_root() || !listed.items.is_empty(),
            folder_eol_style(&listed.items),
        ),
        _ => (true, None),
    }
}

/// `WriteService::preview_effect`.
pub(crate) async fn preview_effect(
    hub: &Inner,
    ctx: &WriteCtx,
    r: PreviewRequest,
) -> Result<ChangePreview, WriteError> {
    let user = authorize(hub, ctx, Role::Editor).await?;
    if matches!(check_lock(hub, &r.swimlane).await?, LockState::UnknownSwimlane) {
        return Err(WriteError::NotFound);
    }
    // Content over the limit, and a path that may not be written, get no preview.
    if let PreviewChange::Upload { content, .. } = &r.change {
        if content.len() > hub.cfg.max_content_bytes {
            return Err(WriteError::BadRequest);
        }
    }
    if hub.deny.is_denied(&r.path) {
        return Err(WriteError::BadRequest);
    }

    let swimlane_facts = facts(hub, &r.swimlane).await?;
    let plan = plan(&swimlane_facts, &r.path)?;
    let mut wanted: Vec<&NfsPath> = vec![&plan.base, &r.path];
    wanted.extend(plan.tenant.as_ref());
    wanted.extend(plan.root.as_ref());
    let now = read_layers(hub, &user, &r.swimlane, &wanted).await?;
    let (folder_exists, folder_style) = folder_facts(hub, &user, &r.swimlane, &r.path).await;

    // The token is bound to what was asked, not to what was computed.
    let (action, expected, content_digest) = match &r.change {
        PreviewChange::Upload { expected, content } => {
            (PreviewAction::Upload, *expected, Some(content_hash(content)))
        }
        PreviewChange::Delete { expected } => (
            PreviewAction::Delete,
            domain::Expected::Hash { hash: *expected },
            None,
        ),
    };
    let work = Work {
        plan,
        tenants: swimlane_facts.tenants,
        now,
        target: r.path.clone(),
        change: r.change.clone(),
        folder_style,
        folder_exists,
    };
    let computed = hub
        .blocking("preview", {
            let (deny, limits) = (hub.deny.clone(), hub.limits.clone());
            move || compute(&work, &deny, &limits)
        })
        .await??;

    let key = hub
        .preview_key
        .get(hub.cfg.db_timeout)
        .await
        .map_err(|_| WriteError::Unavailable)?;
    let binding = Binding {
        user: &user.id,
        swimlane: &r.swimlane,
        path: &r.path,
        action,
        expected: &expected,
        content: content_digest.as_ref(),
    };
    let (preview_id, expires_ms) = issue(&key, &binding, hub.now_ms()).ok_or(WriteError::Unavailable)?;
    Ok(ChangePreview {
        preview_id,
        expires_at: Timestamp::from_unix_millis(expires_ms),
        scope: computed.scope,
        warnings: computed.warnings,
        changes: computed.changes,
        changes_total: u32::try_from(computed.changes_total).unwrap_or(u32::MAX),
        effective_changed: computed.effective_changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scan_flags_only_the_lines_it_found() {
        let found = Scan::Matches(vec![SecretMatch {
            line: 4,
            column: 1,
            length: 20,
            rule: engine::SecretRule::AwsAccessKey,
        }]);
        let lines = |a, b| LineRange::new(a, b).unwrap();
        assert!(found.flags(lines(3, 3)).is_none());
        assert!(found.flags(lines(5, 6)).is_none());
        assert_eq!(
            found.flags(lines(4, 4)).unwrap(),
            "[REDACTED: 20 chars, aws_access_key]"
        );
        assert!(
            found.flags(lines(2, 9)).is_some(),
            "a value spanning the line is flagged"
        );
        assert!(Scan::Clean.flags(lines(1, 9)).is_none());
        assert!(Scan::Unscannable.flags(lines(1, 1)).is_some());
    }
}
