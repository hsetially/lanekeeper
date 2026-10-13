//! Effect previews (T6, D46, D84, D85): what a change to a file does to what the services read, shown before anybody
//! confirms it.
//!
//! The input is the layers of one logical file as they are now, from the point of view of one tenant (`Layers`: tenant
//! file, base file, root file), and a change to one of them. The preview compares the effective config before and after
//! (`effective`, so both merge modes are right) and names what a person has to know:
//!
//! - deleting a tenant file makes the service fall back to the base file ([`EffectWarning::FallsBackToBase`]), or leaves
//!   it with no file at all ([`EffectWarning::NoFileLeft`]);
//! - a new tenant file in `whole_file` mode forks from base: base changes no longer reach the tenant
//!   ([`EffectWarning::ForksFromBase`]);
//! - a tenant file whose suffix is not one of the swimlane's tenants is never read ([`EffectWarning::NeverRead`]), and
//!   changes nothing for anybody;
//! - a root file is read by every service ([`EffectScope::EveryService`]); any other file by the application of its
//!   folder.
//!
//! The preview reads no file and no clock. When the change is to a tenant file, `layers` must be the view of the tenant the
//! file is for. When it is to a base file, `layers` is the view of whichever tenant the caller wants to preview.

use domain::{AppName, MergeMode, NfsPath, SettingChange, TenantId, TenantSet};

use crate::diff::semantic_diff;
use crate::effective::{Layer, Layers, effective};
use crate::limits::Limits;
use crate::paths::FileFacts;

/// A change to the file that `EffectInput::target` names.
#[derive(Debug, Clone, Copy)]
pub enum EffectChange<'a> {
    /// The file is deleted.
    Delete,
    /// The file is created, or replaced by this content (the layer's path is the file's).
    Write(Layer<'a>),
}

/// Everything `effect_preview` needs.
#[derive(Debug, Clone, Copy)]
pub struct EffectInput<'a> {
    pub mode: MergeMode,
    /// The file that changes: a tenant file when `target.tenant` is set, otherwise the base file of the logical file.
    pub target: &'a FileFacts,
    /// The tenants the swimlane deploys.
    pub tenants: &'a TenantSet,
    /// The layers of the logical file as they are now.
    pub layers: Layers<'a>,
    pub change: EffectChange<'a>,
}

/// Who reads the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectScope {
    /// A file in the root folder: every service asks for it (D82), so a change to it can affect all of them.
    EveryService,
    /// A file below the folder of an application.
    Application(AppName),
    /// A file below a folder whose name is not an application name.
    Folder(NfsPath),
}

/// A consequence that the person confirming the change should see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectWarning {
    /// A tenant file is deleted and the base file takes over (D46).
    FallsBackToBase,
    /// The only file there is gets deleted: the service finds nothing.
    NoFileLeft,
    /// A tenant file is created over a base file in `whole_file` mode: from now on it is a copy that falls behind (C1).
    ForksFromBase,
    /// The file's suffix is the id of a tenant that the swimlane does not deploy: no service of it reads the file.
    NeverRead { suffix: TenantId },
}

/// What a change does.
#[derive(Clone, PartialEq, Eq)]
pub struct EffectPreview {
    pub scope: EffectScope,
    pub warnings: Vec<EffectWarning>,
    /// The settings that would differ in the effective config, from now to after the change, sorted by path, at most
    /// `Limits::locations_per_finding`. Empty for a text or binary file: see `effective_changed`.
    pub changes: Vec<SettingChange>,
    /// All of them, listed or not.
    pub changes_total: usize,
    /// What the service gets would be different: the effective hash changes. True for a text or binary file that changes,
    /// and for a structured file that changes only in how it is written (order, comments).
    pub effective_changed: bool,
}

impl std::fmt::Debug for EffectPreview {
    // A change carries values, and a value may be a secret: only the number of changes is printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectPreview")
            .field("scope", &self.scope)
            .field("warnings", &self.warnings)
            .field("changes", &self.changes.len())
            .field("changes_total", &self.changes_total)
            .field("effective_changed", &self.effective_changed)
            .finish()
    }
}

/// What `change` does to the effective config of the logical file.
pub fn effect_preview(input: &EffectInput<'_>, limits: &Limits) -> EffectPreview {
    let scope = scope_of(input.target);
    let is_tenant_file = input.target.tenant.is_some();

    // A file for a tenant the swimlane does not deploy is read by nobody: nothing changes, and that is the news.
    if let Some(suffix) = input
        .target
        .tenant
        .as_ref()
        .filter(|t| !input.tenants.contains(t))
    {
        return EffectPreview {
            scope,
            warnings: vec![EffectWarning::NeverRead {
                suffix: suffix.clone(),
            }],
            changes: Vec::new(),
            changes_total: 0,
            effective_changed: false,
        };
    }

    let before = input.layers;
    let mut after = input.layers;
    let slot = if is_tenant_file {
        &mut after.tenant
    } else {
        &mut after.base
    };
    let existed = slot.is_some();
    *slot = match input.change {
        EffectChange::Delete => None,
        EffectChange::Write(layer) => Some(layer),
    };

    let mut warnings = Vec::new();
    match input.change {
        EffectChange::Delete => {
            // A root file is still a layer of a property source, so it is something to read.
            let root_remains = input.mode == MergeMode::SpringMerge && after.root.is_some();
            if after.tenant.is_none() && after.base.is_none() && !root_remains {
                warnings.push(EffectWarning::NoFileLeft);
            } else if is_tenant_file && after.base.is_some() {
                warnings.push(EffectWarning::FallsBackToBase);
            }
        }
        EffectChange::Write(_) => {
            if is_tenant_file && !existed && input.mode == MergeMode::WholeFile && before.base.is_some() {
                warnings.push(EffectWarning::ForksFromBase);
            }
        }
    }

    let (old, new) = (effective(input.mode, &before), effective(input.mode, &after));
    let old_settings = old.as_ref().map_or(&[][..], |e| e.settings.as_slice());
    let new_settings = new.as_ref().map_or(&[][..], |e| e.settings.as_slice());
    let mut changes = semantic_diff(old_settings, new_settings);
    let changes_total = changes.len();
    changes.truncate(limits.locations_per_finding);

    EffectPreview {
        scope,
        warnings,
        changes,
        changes_total,
        effective_changed: old.map(|e| e.hash) != new.map(|e| e.hash),
    }
}

/// Who reads the logical file of `target`.
fn scope_of(target: &FileFacts) -> EffectScope {
    let mut parts = target.logical.as_str().split('/');
    let (Some(first), Some(_)) = (parts.next(), parts.next()) else {
        return EffectScope::EveryService;
    };
    match AppName::parse(first) {
        Ok(app) => EffectScope::Application(app),
        Err(_) => match NfsPath::parse(first) {
            Ok(folder) => EffectScope::Folder(folder),
            // The first component of a valid path is a valid path; this keeps the function total all the same.
            Err(_) => EffectScope::EveryService,
        },
    }
}
