//! Which files are property sources and which are resources (D82).
//!
//! A property source is `application.*`, `application-<profile>.*`, `<app>.*` or `<app>-<tenant>.*`, where `.*` is
//! `.properties`, `.yml` or `.yaml` (lower case: file names on NFS are case-sensitive), in any folder (every folder but a
//! channel's is a search location). Everything else is a resource. `application_names` come from the Deployments'
//! `SPRING_APPLICATION_NAME` (D88).

use domain::{AppName, FileRole, LogicalFile, MergeMode, NfsPath, TenantId, TenantSet};

/// The extensions of a property source.
pub(crate) const PROPERTY_EXTENSIONS: [&str; 3] = ["properties", "yml", "yaml"];

/// A file name split into stem and extension at the last dot, the way the path mapping does: a leading dot, a trailing dot
/// and a name with no dot have no extension.
pub(crate) fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// True when `name` is the name of a property source. `known_tenant` tells whether a suffix is a tenant of the swimlane;
/// `is_app` whether a name is an application.
fn is_property_source(
    name: &str,
    known_tenant: impl Fn(&str) -> bool,
    is_app: impl Fn(&str) -> bool,
) -> bool {
    let (stem, Some(ext)) = split_name(name) else {
        return false;
    };
    if !PROPERTY_EXTENSIONS.contains(&ext) {
        return false;
    }
    if stem == "application"
        || stem
            .strip_prefix("application-")
            .is_some_and(|rest| !rest.is_empty())
    {
        return true;
    }
    if is_app(stem) {
        return true;
    }
    // `<app>-<tenant>`: the app name may itself hold hyphens, so try every split.
    stem.match_indices('-')
        .any(|(i, _)| is_app(&stem[..i]) && known_tenant(&stem[i + 1..]))
}

/// The role of an NFS path: a property source or a resource. `tenants` are the swimlane's tenants (so that
/// `<app>-<tenant>.yml` is a property source and `<app>-sit9.yml` of an unknown tenant is not).
pub fn file_role(path: &NfsPath, tenants: &TenantSet, application_names: &[AppName]) -> FileRole {
    let known = |suffix: &str| TenantId::parse(suffix).is_ok_and(|t| tenants.contains(&t));
    let app = |name: &str| application_names.iter().any(|a| a.as_str() == name);
    if is_property_source(path.file_name(), known, app) {
        FileRole::PropertySource
    } else {
        FileRole::Resource
    }
}

/// The role of a logical file, which has no tenant suffix: `<app>.yml` stands for `<app>-<tenant>.yml` too.
pub fn logical_file_role(file: &LogicalFile, application_names: &[AppName]) -> FileRole {
    let app = |name: &str| application_names.iter().any(|a| a.as_str() == name);
    if is_property_source(file.as_path().file_name(), |_| false, app) {
        FileRole::PropertySource
    } else {
        FileRole::Resource
    }
}

/// How a file of this role merges: a property source merges setting by setting, a resource is chosen whole.
pub const fn merge_mode(role: FileRole) -> MergeMode {
    match role {
        FileRole::PropertySource => MergeMode::SpringMerge,
        FileRole::Resource => MergeMode::WholeFile,
    }
}
