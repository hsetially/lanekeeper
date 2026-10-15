//! Which services read a file (T10, D82, D85): the **consumers** of a change.
//!
//! The config-server answers `(application, profile = "<tenant>,default", [channel], file)`, so a file reaches a service in one of two ways,
//! and the rules below are the two ways, written once:
//!
//! | The file | Read by |
//! |---|---|
//! | a file at the root (any role) | every service (the root is a search location of every request, D83; the domain model: "root files map to every service") |
//! | a property source `application.*` or `application-<profile>.*`, in any folder | every service: they are in the property view of every application (D82) |
//! | a property source `<app>.*` or `<app>-<tenant>.*`, in any folder | the services whose `SPRING_APPLICATION_NAME` is `<app>` (the engine's own cut of the name, with every split at a hyphen tried) |
//! | any other file in a folder | the services whose application is named like the **first folder** of the path (the config-server searches `<root>/{application}`; the engine's `EffectScope::Application`), and the Deployments an admin has linked to the **deepest mapped folder** above the file (`folder_mappings`, confirmed rows only) |
//!
//! A folder mapping of the swimlane replaces the global rows for the same folder (the table's own rule: "a row with a swimlane overrides
//! it"), and a mapping that names a Deployment the swimlane does not run, or one that is gone, links nobody. A suggested mapping that no
//! person has confirmed is a guess and is not used: reading a guess as a fact would say a service is stale when nothing links it to the file.
//!
//! What this cannot know: a **resource file's folder says nothing about who asks for it** (every non-channel folder is a search location, so
//! any service that knows the file's name can ask for it). The first folder and the admin's links are what the registry has; a resource
//! that is read by a service neither names is not shown as pending for it. That is a limit of the model, written down in D105, and the
//! reason the admin's folder mapping exists.
//!
//! The functions here read no database and no clock: they are given the services and the mappings.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use domain::{AppName, FileRole, NfsPath, TenantSet};

use crate::tenants;

/// The services that consume files, as indexes into the list the caller keeps, and what the rules need to find them.
#[derive(Debug, Default)]
pub(crate) struct Directory {
    /// Every service that can be stale: all the indexes.
    every: Vec<usize>,
    /// Services by the application name they run.
    by_app: HashMap<String, Vec<usize>>,
    /// Services by the folder an admin linked them to.
    mapped: BTreeMap<String, Vec<usize>>,
}

/// One service the rules may name.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Reader<'a> {
    pub namespace: &'a str,
    pub name: &'a str,
    pub app: Option<&'a AppName>,
}

/// A row of `folder_mappings` that a person confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mapping {
    /// The row belongs to the swimlane, not to every swimlane.
    pub of_swimlane: bool,
    pub folder: String,
    pub namespace: String,
    pub deployment: String,
}

impl Directory {
    /// The directory for `services` (the index of each is its place in the list) and the confirmed `mappings` of the swimlane and of all.
    pub(crate) fn new(services: &[Reader<'_>], mappings: &[Mapping]) -> Self {
        let mut directory = Self {
            every: (0..services.len()).collect(),
            ..Self::default()
        };
        let mut by_key: HashMap<(&str, &str), usize> = HashMap::with_capacity(services.len());
        for (i, s) in services.iter().enumerate() {
            by_key.insert((s.namespace, s.name), i);
            if let Some(app) = s.app {
                directory
                    .by_app
                    .entry(app.as_str().to_owned())
                    .or_default()
                    .push(i);
            }
        }
        // The swimlane's own rows for a folder replace the global ones for it.
        let own: std::collections::BTreeSet<&str> = mappings
            .iter()
            .filter(|m| m.of_swimlane)
            .map(|m| m.folder.as_str())
            .collect();
        for m in mappings {
            if !m.of_swimlane && own.contains(m.folder.as_str()) {
                continue;
            }
            if let Some(&i) = by_key.get(&(m.namespace.as_str(), m.deployment.as_str())) {
                let list = directory.mapped.entry(m.folder.clone()).or_default();
                if !list.contains(&i) {
                    list.push(i);
                }
            }
        }
        directory
    }

    /// The services that read the file at `path`, whose role is `role`, in a swimlane that deploys `tenants`. Sorted, without repeats.
    pub(crate) fn readers(&self, path: &NfsPath, role: FileRole, tenants: &TenantSet) -> Cow<'_, [usize]> {
        if path.parent().is_root() {
            return Cow::Borrowed(&self.every);
        }
        match role {
            FileRole::PropertySource => self.property_readers(path, tenants),
            FileRole::Resource => self.folder_readers(path),
        }
    }

    fn property_readers(&self, path: &NfsPath, tenants: &TenantSet) -> Cow<'_, [usize]> {
        let name = path.file_name();
        let stem = name.rfind('.').filter(|&i| i > 0).map_or(name, |i| &name[..i]);
        if stem == "application"
            || stem
                .strip_prefix("application-")
                .is_some_and(|rest| !rest.is_empty())
        {
            return Cow::Borrowed(&self.every);
        }
        let mut found: Vec<usize> = Vec::new();
        let mut add = |app: &str| {
            if let Some(list) = self.by_app.get(app) {
                found.extend(list);
            }
        };
        // `<app>`, and every cut at a hyphen into `<app>-<tenant>`: an application name can hold hyphens itself.
        add(stem);
        for (i, _) in stem.match_indices('-') {
            let known = domain::TenantId::parse(&stem[i + 1..]).is_ok_and(|t| tenants.contains(&t));
            if known {
                add(&stem[..i]);
            }
        }
        found.sort_unstable();
        found.dedup();
        Cow::Owned(found)
    }

    fn folder_readers(&self, path: &NfsPath) -> Cow<'_, [usize]> {
        let mut found: Vec<usize> = Vec::new();
        if let Some(first) = path.components().next() {
            if let Some(list) = self.by_app.get(first) {
                found.extend(list);
            }
        }
        // The deepest folder an admin linked, from the file's own folder up.
        let mut dir = path.parent();
        while !dir.is_root() {
            if let Some(list) = self.mapped.get(dir.as_str()) {
                found.extend(list);
                break;
            }
            dir = dir.parent();
        }
        found.sort_unstable();
        found.dedup();
        Cow::Owned(found)
    }
}

/// Whether the swimlane reads a tenant copy at all (a copy for a tenant it does not deploy is read by nobody), from the suffix of the file.
pub(crate) fn is_read(path: &NfsPath, logical: &domain::LogicalFile, tenants: &TenantSet) -> bool {
    tenants::is_read(tenants::tenant_suffix(path, logical).as_ref(), tenants)
}

#[cfg(test)]
mod tests {
    use domain::TenantId;
    use engine::file_role;

    use super::*;

    fn app(s: &str) -> AppName {
        AppName::parse(s).unwrap()
    }

    fn path(s: &str) -> NfsPath {
        NfsPath::parse(s).unwrap()
    }

    fn tenants(names: &[&str]) -> TenantSet {
        TenantSet::new(names.iter().map(|n| TenantId::parse(n).unwrap())).unwrap()
    }

    fn mapping(of_swimlane: bool, folder: &str, deployment: &str) -> Mapping {
        Mapping {
            of_swimlane,
            folder: folder.to_owned(),
            namespace: "csp".to_owned(),
            deployment: deployment.to_owned(),
        }
    }

    /// Four services: 0 `api` (app `tx-api`), 1 `web` (app `portal`), 2 `batch` (no app name), 3 `core` (app `tx-infinity-core`).
    fn world(mappings: &[Mapping]) -> (Directory, Vec<AppName>) {
        let apps = [app("tx-api"), app("portal"), app("tx-infinity-core")];
        let services = [
            Reader {
                namespace: "csp",
                name: "api",
                app: Some(&apps[0]),
            },
            Reader {
                namespace: "csp",
                name: "web",
                app: Some(&apps[1]),
            },
            Reader {
                namespace: "csp",
                name: "batch",
                app: None,
            },
            Reader {
                namespace: "csp",
                name: "core",
                app: Some(&apps[2]),
            },
        ];
        (Directory::new(&services, mappings), apps.to_vec())
    }

    fn readers(d: &Directory, apps: &[AppName], tenants: &TenantSet, p: &str) -> Vec<usize> {
        let path = path(p);
        let role = file_role(&path, tenants, apps);
        d.readers(&path, role, tenants).into_owned()
    }

    #[test]
    fn a_root_file_is_read_by_every_service_whatever_its_role() {
        let (d, apps) = world(&[]);
        let t = tenants(&["sit1"]);
        assert_eq!(
            readers(&d, &apps, &t, "application.yml"),
            [0, 1, 2, 3],
            "a property source"
        );
        assert_eq!(readers(&d, &apps, &t, "channels.yml"), [0, 1, 2, 3]);
        assert_eq!(readers(&d, &apps, &t, "logo.png"), [0, 1, 2, 3], "a resource");
    }

    #[test]
    fn application_property_sources_are_read_by_every_service_in_any_folder() {
        let (d, apps) = world(&[]);
        let t = tenants(&["sit1"]);
        assert_eq!(
            readers(&d, &apps, &t, "somewhere/deep/application.yml"),
            [0, 1, 2, 3]
        );
        assert_eq!(
            readers(&d, &apps, &t, "somewhere/application-sit1.properties"),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn an_application_property_source_is_read_by_the_services_of_that_application_only() {
        let (d, apps) = world(&[]);
        let t = tenants(&["sit1"]);
        assert_eq!(readers(&d, &apps, &t, "anywhere/tx-api.yml"), [0]);
        assert_eq!(
            readers(&d, &apps, &t, "anywhere/tx-api-sit1.yml"),
            [0],
            "the tenant's copy"
        );
        // An application name with hyphens of its own: every cut at a hyphen is tried.
        assert_eq!(readers(&d, &apps, &t, "x/tx-infinity-core-sit1.yml"), [3]);
        assert_eq!(readers(&d, &apps, &t, "x/tx-infinity-core.yaml"), [3]);
        // `<app>-<suffix>` of a tenant the swimlane does not deploy is not a property source of it, and the file is a resource of its folder.
        assert_eq!(readers(&d, &apps, &t, "x/tx-api-sit9.yml"), Vec::<usize>::new());
    }

    #[test]
    fn a_resource_is_read_by_the_application_of_its_first_folder_and_by_the_services_linked_to_the_deepest_folder_above_it()
     {
        let (d, apps) = world(&[
            mapping(false, "reports", "batch"),
            mapping(false, "reports/monthly", "core"),
            mapping(false, "reports/monthly", "gone"),
        ]);
        let t = tenants(&["sit1"]);
        assert_eq!(
            readers(&d, &apps, &t, "tx-api/layout.xsl"),
            [0],
            "the application's own folder"
        );
        assert_eq!(readers(&d, &apps, &t, "portal/img/a.png"), [1], "below it too");
        assert_eq!(
            readers(&d, &apps, &t, "reports/summary.xsl"),
            [2],
            "a linked folder"
        );
        assert_eq!(
            readers(&d, &apps, &t, "reports/monthly/a.xsl"),
            [3],
            "the deepest link decides: the folder above does not also read it"
        );
        assert_eq!(
            readers(&d, &apps, &t, "nobody/asks.txt"),
            Vec::<usize>::new(),
            "no application, no link"
        );
    }

    #[test]
    fn a_swimlanes_own_mapping_replaces_the_global_one_for_its_folder() {
        let (d, apps) = world(&[
            mapping(false, "reports", "batch"),
            mapping(true, "reports", "core"),
            mapping(false, "other", "batch"),
        ]);
        let t = tenants(&["sit1"]);
        assert_eq!(readers(&d, &apps, &t, "reports/a.xsl"), [3]);
        assert_eq!(
            readers(&d, &apps, &t, "other/a.xsl"),
            [2],
            "the global row stands where the swimlane says nothing"
        );
    }

    #[test]
    fn the_application_of_a_folder_and_the_link_to_it_are_both_readers() {
        let (d, apps) = world(&[mapping(false, "tx-api/jobs", "batch")]);
        let t = tenants(&["sit1"]);
        assert_eq!(readers(&d, &apps, &t, "tx-api/jobs/run.xsl"), [0, 2]);
    }

    #[test]
    fn the_rule_that_finds_the_readers_is_the_one_of_the_role_the_engine_gives() {
        // The engine's `file_role` decides property source or resource, and a file is found by the rule of the role it is given.
        let (d, apps) = world(&[mapping(false, "tx-api", "batch")]);
        let t = tenants(&["sit1"]);
        for (p, role, expected) in [
            // Property sources are found by name, whatever folder they are in.
            ("tx-api/tx-api.yml", FileRole::PropertySource, vec![0]),
            ("elsewhere/tx-api-sit1.yml", FileRole::PropertySource, vec![0]),
            ("portal/portal-sit1.properties", FileRole::PropertySource, vec![1]),
            (
                "a/b/application-sit2.yml",
                FileRole::PropertySource,
                vec![0, 1, 2, 3],
            ),
            // A yml that is no application's is a resource of its folder: the application of the folder and the Deployment linked to it.
            ("tx-api/other.yml", FileRole::Resource, vec![0, 2]),
            // A suffix of a tenant that is not deployed does not make an application's file a property source.
            ("tx-api/tx-api-sit9.yml", FileRole::Resource, vec![0, 2]),
        ] {
            let path = path(p);
            assert_eq!(file_role(&path, &t, &apps), role, "{p}: the engine's role");
            assert_eq!(d.readers(&path, role, &t).into_owned(), expected, "{p}");
        }
    }

    #[test]
    fn a_tenant_copy_nobody_deploys_is_not_read() {
        use domain::LogicalFile;
        let t = tenants(&["sit1", "sit2"]);
        let logical = LogicalFile::parse("a/core.yml").unwrap();
        assert!(is_read(&path("a/core.yml"), &logical, &t));
        assert!(is_read(&path("a/core-sit2.yml"), &logical, &t));
        assert!(!is_read(&path("a/core-sit3.yml"), &logical, &t));
        assert!(!is_read(
            &path("a/core-sit1.yml"),
            &logical,
            &TenantSet::default()
        ));
    }

    #[test]
    fn no_service_gives_no_reader() {
        let d = Directory::new(&[], &[mapping(false, "a", "b")]);
        let t = tenants(&["sit1"]);
        assert!(d.readers(&path("a/x.yml"), FileRole::Resource, &t).is_empty());
        assert!(d.readers(&path("x.yml"), FileRole::Resource, &t).is_empty());
    }
}
