//! The paths of a sentinel's records, mapped to the paths of its swimlane (A17, rule 10, S11, S25).
//!
//! A sentinel reports a path **relative to its own `export_root`** (the root it watches on the NFS VM), and a swimlane's [`NfsPath`] is
//! relative to the swimlane's config root (D82). Two admin values say how the two relate: `sentinels.export_root`, and
//! `swimlanes.nfs_export` (the export the swimlane's agent mounts, as its `hello` or the admin set it). A17, as the plan approved it:
//!
//! - when `nfs_export` is **under** `export_root` (or equal to it), the part of `nfs_export` below `export_root` is the swimlane's prefix
//!   in a sentinel's paths: a record is the swimlane's when its path is **strictly below** that prefix, and the path it is stored under
//!   is the part below it;
//! - otherwise (no `nfs_export` yet, or one that is not under `export_root`) a sentinel's path is **already relative to the config
//!   root**, and is kept as it is;
//! - a record outside the prefix, or on the prefix itself (a directory, not a file of the swimlane), is **dropped and counted**, never
//!   stored: it is another swimlane's, or nobody's.
//!
//! The values of both roots are text from an admin or, for `nfs_export`, from an agent that filled in what the admin left empty, so they
//! are untrusted too: each must be an absolute path that [`NfsPath::parse_prefix`] accepts (no `..`, no control character, no backslash,
//! bounded), and one that is not **cannot be mapped**: every record of that sentinel is dropped rather than guessed at. Prefixes are
//! compared component by component, never as text, so a prefix `sit1` does not take `sit10/x`.
//!
//! [`NfsPath`]s of a record have been through the same parser at the edge (`proto::convert`) before they reach the sink; the mapping
//! parses the raw text again only for the fuzz target and the tests ([`Mapper::map_str`]), which is how a hostile path is shown to be
//! dropped or cleaned and never to come out with a traversal in it.

use domain::NfsPath;

/// How the paths of one sentinel's records become the paths of its swimlane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapper {
    /// The part of a record's path that is the swimlane's root. The root itself when a sentinel's paths are already relative to it.
    prefix: NfsPath,
}

/// The components of an absolute path (`/export/sit1`), or `None` when it is not a clean one: not absolute, or a path that
/// [`NfsPath::parse_prefix`] refuses (a `..`, a control character, a backslash, too long). `/` is the empty path.
fn absolute(path: &str) -> Option<NfsPath> {
    let rest = path.strip_prefix('/')?;
    NfsPath::parse_prefix(rest).ok()
}

impl Mapper {
    /// The mapping for a sentinel whose root is `export_root` and whose swimlane's export is `nfs_export`, or `None` when it cannot be
    /// established (a value that is not a clean absolute path): then no record of the sentinel is the swimlane's.
    pub fn new(export_root: &str, nfs_export: Option<&str>) -> Option<Self> {
        let root = absolute(export_root)?;
        let Some(export) = nfs_export else {
            return Some(Self {
                prefix: NfsPath::root(),
            });
        };
        let export = absolute(export)?;
        if !export.starts_with(&root) {
            // Not under the sentinel's root: its paths are already the swimlane's.
            return Some(Self {
                prefix: NfsPath::root(),
            });
        }
        let below: Vec<&str> = export.components().skip(root.components().count()).collect();
        // The components came out of a parsed path, so joining them is clean; a refusal here would be a bug and drops everything.
        let prefix = NfsPath::parse_prefix(&below.join("/")).ok()?;
        Some(Self { prefix })
    }

    /// The path a record is stored under, or `None` when the record is not the swimlane's.
    pub fn map(&self, path: &NfsPath) -> Option<NfsPath> {
        if self.prefix.is_root() {
            return Some(path.clone());
        }
        if !path.starts_with(&self.prefix) || path == &self.prefix {
            return None;
        }
        let below: Vec<&str> = path.components().skip(self.prefix.components().count()).collect();
        NfsPath::parse(&below.join("/")).ok()
    }

    /// [`Mapper::map`] of raw text: parsed as an `NfsPath` first, so a path the edge would have refused is dropped here too.
    pub fn map_str(&self, raw: &str) -> Option<NfsPath> {
        self.map(&NfsPath::parse(raw).ok()?)
    }

    /// The swimlane's prefix in a sentinel's paths (the root when there is none).
    pub fn prefix(&self) -> &NfsPath {
        &self.prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapper(root: &str, export: Option<&str>) -> Mapper {
        Mapper::new(root, export).expect("a mapping")
    }

    fn mapped(m: &Mapper, raw: &str) -> Option<String> {
        m.map_str(raw).map(|p| p.as_str().to_owned())
    }

    #[test]
    fn an_export_below_the_root_strips_its_prefix() {
        let m = mapper("/export", Some("/export/sit1/ns/csp-configuration"));
        assert_eq!(m.prefix().as_str(), "sit1/ns/csp-configuration");
        assert_eq!(
            mapped(&m, "sit1/ns/csp-configuration/svc/app.yml").as_deref(),
            Some("svc/app.yml")
        );
        assert_eq!(
            mapped(&m, "sit1/ns/csp-configuration/a.yml").as_deref(),
            Some("a.yml")
        );
    }

    #[test]
    fn an_export_equal_to_the_root_keeps_the_paths_as_they_are() {
        let m = mapper("/export", Some("/export"));
        assert!(m.prefix().is_root());
        assert_eq!(mapped(&m, "svc/app.yml").as_deref(), Some("svc/app.yml"));
        // Trailing slashes and dots are the same place.
        let m = mapper("/export/", Some("/export/./"));
        assert!(m.prefix().is_root());
    }

    #[test]
    fn an_export_that_is_not_under_the_root_means_the_paths_are_already_relative_to_the_config_root() {
        for export in [Some("/other/place"), Some("/exportx/sit1"), Some("/"), None] {
            let m = mapper("/export", export);
            assert!(m.prefix().is_root(), "{export:?}");
            assert_eq!(
                mapped(&m, "svc/app.yml").as_deref(),
                Some("svc/app.yml"),
                "{export:?}"
            );
        }
    }

    #[test]
    fn a_root_of_slash_makes_the_whole_export_the_prefix() {
        let m = mapper("/", Some("/export/sit1"));
        assert_eq!(m.prefix().as_str(), "export/sit1");
        assert_eq!(mapped(&m, "export/sit1/a.yml").as_deref(), Some("a.yml"));
        assert_eq!(mapped(&m, "a.yml"), None);
    }

    #[test]
    fn a_record_outside_the_prefix_or_on_it_is_dropped() {
        let m = mapper("/export", Some("/export/sit1/csp"));
        for raw in [
            "sit2/csp/a.yml",
            "sit1/other/a.yml",
            "sit1/csp",
            "sit1",
            "a.yml",
            "sit1/cspx/a.yml",
        ] {
            assert_eq!(mapped(&m, raw), None, "{raw}");
        }
    }

    #[test]
    fn the_prefix_is_compared_by_component_and_not_as_text() {
        let m = mapper("/export", Some("/export/sit1"));
        assert_eq!(mapped(&m, "sit10/x.yml"), None, "sit10 is not under sit1");
        assert_eq!(mapped(&m, "sit1x/x.yml"), None);
        assert_eq!(mapped(&m, "sit1/x.yml").as_deref(), Some("x.yml"));
        // And the same for the roots: /exportx is not under /export.
        let m = mapper("/export", Some("/exportx/sit1"));
        assert!(m.prefix().is_root());
    }

    #[test]
    fn hostile_paths_are_dropped_or_cleaned_and_never_survive_with_a_traversal() {
        let m = mapper("/export", Some("/export/sit1"));
        for raw in [
            "sit1/../sit2/a.yml",
            "../sit1/a.yml",
            "sit1/a/../../b.yml",
            "sit1/..",
            "/sit1/a.yml",
            "sit1\\a.yml",
            "sit1/a\0.yml",
            "sit1/a\n.yml",
            "sit1/a\u{7f}.yml",
            "",
            "/",
            ".",
            "sit1/",
        ] {
            assert_eq!(mapped(&m, raw), None, "{raw:?}");
        }
        // Dots and doubled slashes are cleaned by the parser, so they cannot hide a prefix or a name.
        assert_eq!(mapped(&m, "sit1//./a.yml").as_deref(), Some("a.yml"));
        assert_eq!(mapped(&m, "./sit1/a.yml").as_deref(), Some("a.yml"));
        // Too long: a component over 255 bytes, a path over 1,024, raw text over the parser's bound.
        assert_eq!(mapped(&m, &format!("sit1/{}", "a".repeat(256))), None);
        assert_eq!(mapped(&m, &format!("sit1/{}", "a/".repeat(600))), None);
        assert_eq!(mapped(&m, &format!("sit1/{}", "a".repeat(9000))), None);
    }

    #[test]
    fn a_name_that_looks_like_traversal_but_is_a_name_is_kept() {
        let m = mapper("/export", Some("/export/sit1"));
        assert_eq!(mapped(&m, "sit1/..a.yml").as_deref(), Some("..a.yml"));
        assert_eq!(mapped(&m, "sit1/a..b/c.yml").as_deref(), Some("a..b/c.yml"));
        assert_eq!(mapped(&m, "sit1/.hidden.yml").as_deref(), Some(".hidden.yml"));
    }

    #[test]
    fn roots_that_are_not_clean_absolute_paths_cannot_be_mapped() {
        for (root, export) in [
            ("export", Some("/export/sit1")),
            ("", Some("/export/sit1")),
            ("/export/../etc", None),
            ("/export", Some("/export/../../etc")),
            ("/export", Some("export/sit1")),
            ("/export", Some("")),
            ("/export", Some("/export/sit1\0")),
            ("/export", Some("/export\\sit1")),
            ("/exp\nort", None),
        ] {
            assert_eq!(Mapper::new(root, export), None, "{root:?} {export:?}");
        }
    }

    #[test]
    fn an_export_with_a_traversal_does_not_become_a_prefix_that_matches_other_trees() {
        // If `..` were resolved, /export/a/../b would be /export/b and a record of tree b would be taken for this swimlane.
        assert_eq!(Mapper::new("/export", Some("/export/a/../b")), None);
    }

    #[test]
    fn the_stored_path_is_always_a_clean_nfs_path_that_parses_back_to_itself() {
        let m = mapper("/export", Some("/export/sit1/csp"));
        for raw in ["sit1/csp/a/b.yml", "sit1//csp/./a/b.yml", "sit1/csp/a//b.yml"] {
            let p = m.map_str(raw).expect("mapped");
            assert_eq!(NfsPath::parse(p.as_str()).as_ref(), Ok(&p));
            assert_eq!(p.as_str(), "a/b.yml");
        }
    }
}
