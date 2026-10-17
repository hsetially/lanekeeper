//! Which branch of the tenant repository is what (T4, D22, D84).
//!
//! | Branch | Class | Meaning |
//! |---|---|---|
//! | `templates/common-docs` | [`BranchClass::CommonDocs`] | the docs source, never deployed |
//! | anything else under `templates/` | [`BranchClass::Template`] | a Core template, never deployed (D22), whatever the admin patterns say |
//! | `sit` followed by digits, or a name an admin pattern includes | [`BranchClass::Deployable`] | a swimlane candidate; its name is the tenant id (D84) |
//! | everything else, and anything an admin pattern excludes | [`BranchClass::Other`] | not deployed |
//!
//! The admin's patterns are the `branch_include` and `branch_exclude` of the tenant repository's row of `path_mappings`: globs
//! matched against the whole branch name. **Exclude wins** over the built-in `sit<digits>` rule and over an include, so an admin can
//! take one branch out; a template or the docs branch cannot be brought in. A branch name that is not a [`TenantId`] (a slash, an
//! upper-case letter, a dot) is never deployable, because the tenant id *is* the branch name and has no room for them.
//!
//! The mirror classifies the same names for `GitReader::branches`, more loosely: every branch without a slash is `Deployable` there.
//! The registry is the authority for the swimlane candidates, so it classifies again with the admin's patterns; the two agree on
//! `sit<digits>`, on every template, and on the docs branch (`tests/branches.rs` holds them to it).

use domain::{BranchKind, TenantId};
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::db::Db;
use crate::error::RegistryError;

/// The docs branch of the tenant repository.
pub const COMMON_DOCS_BRANCH: &str = "templates/common-docs";
const TEMPLATE_PREFIX: &str = "templates/";
/// The most patterns an admin may give in each list, and the longest one in bytes. The columns are text arrays with no limit of their
/// own, so the registry holds the line.
pub const MAX_PATTERNS: usize = 1_000;
pub const MAX_PATTERN_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchClass {
    /// A swimlane candidate.
    Deployable,
    /// `templates/*` except the docs branch.
    Template,
    /// `templates/common-docs`.
    CommonDocs,
    /// Not deployed: a name no rule includes, one an admin excluded, one that cannot be a tenant id.
    Other,
}

impl BranchClass {
    /// The kind `GitReader::branches` uses for the same branch of the tenant repository.
    pub fn kind(self) -> BranchKind {
        match self {
            Self::Deployable => BranchKind::Deployable,
            Self::Template => BranchKind::Template,
            Self::CommonDocs => BranchKind::CommonDocs,
            Self::Other => BranchKind::Other,
        }
    }
}

/// Why the admin's patterns were refused. Names the problem, never the pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BranchError {
    #[error("too many branch patterns")]
    TooManyPatterns,
    #[error("a branch pattern is too long")]
    PatternTooLong,
    #[error("a branch pattern is not a valid glob")]
    InvalidPattern,
}

impl From<BranchError> for RegistryError {
    fn from(_: BranchError) -> Self {
        // A pattern the admin stored that does not compile is a stored value this code did not accept.
        Self::Corrupt
    }
}

fn compile(patterns: &[String]) -> Result<GlobSet, BranchError> {
    if patterns.len() > MAX_PATTERNS {
        return Err(BranchError::TooManyPatterns);
    }
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        if pattern.len() > MAX_PATTERN_BYTES {
            return Err(BranchError::PatternTooLong);
        }
        set.add(Glob::new(pattern).map_err(|_| BranchError::InvalidPattern)?);
    }
    set.build().map_err(|_| BranchError::InvalidPattern)
}

/// `sit` followed by one or more digits, and nothing else.
fn is_sit(name: &str) -> bool {
    name.strip_prefix("sit")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// The classifier of tenant-repository branches. Cheap to clone: the two glob sets are shared.
#[derive(Debug, Clone)]
pub struct BranchClassifier {
    include: GlobSet,
    exclude: GlobSet,
}

impl BranchClassifier {
    /// The built-in rule only: no admin patterns.
    pub fn builtin() -> Self {
        Self {
            include: GlobSet::empty(),
            exclude: GlobSet::empty(),
        }
    }

    /// The built-in rule plus the admin's patterns.
    ///
    /// # Errors
    ///
    /// A list over [`MAX_PATTERNS`], a pattern over [`MAX_PATTERN_BYTES`] or one that is not a glob.
    pub fn new(include: &[String], exclude: &[String]) -> Result<Self, BranchError> {
        Ok(Self {
            include: compile(include)?,
            exclude: compile(exclude)?,
        })
    }

    /// The classifier of the admin's current patterns (the tenant repository's row of `path_mappings`; no row is the built-in rule).
    pub async fn load(db: &Db) -> Result<Self, RegistryError> {
        let row: Option<(Vec<String>, Vec<String>)> = db
            .run(
                "branch_patterns",
                sqlx::query_as(
                    "SELECT branch_include, branch_exclude FROM path_mappings WHERE repo = 'tenant' LIMIT 1",
                )
                .fetch_optional(db.pool()),
            )
            .await?;
        match row {
            Some((include, exclude)) => Ok(Self::new(&include, &exclude)?),
            None => Ok(Self::builtin()),
        }
    }

    pub fn classify(&self, name: &str) -> BranchClass {
        if name == COMMON_DOCS_BRANCH {
            BranchClass::CommonDocs
        } else if name.starts_with(TEMPLATE_PREFIX) {
            BranchClass::Template
        } else if TenantId::parse(name).is_err() || self.exclude.is_match(name) {
            BranchClass::Other
        } else if is_sit(name) || self.include.is_match(name) {
            BranchClass::Deployable
        } else {
            BranchClass::Other
        }
    }

    pub fn is_deployable(&self, name: &str) -> bool {
        self.classify(name) == BranchClass::Deployable
    }

    /// The tenant a deployable branch is, or `None` for any other branch.
    pub fn tenant_of(&self, name: &str) -> Option<TenantId> {
        if self.is_deployable(name) {
            TenantId::parse(name).ok()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn lists(include: &[&str], exclude: &[&str]) -> BranchClassifier {
        let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        BranchClassifier::new(&own(include), &own(exclude)).unwrap()
    }

    #[test]
    fn sit_and_digits_is_deployable_and_nothing_else_of_that_shape() {
        let c = BranchClassifier::builtin();
        for name in ["sit1", "sit2", "sit42", "sit007"] {
            assert_eq!(c.classify(name), BranchClass::Deployable, "{name}");
        }
        for name in [
            "sit", "sitb", "sit1a", "sit-1", "sit_1", "sit1.0", "SIT1", "xsit1", "uat1", "main",
        ] {
            assert_eq!(c.classify(name), BranchClass::Other, "{name}");
        }
    }

    #[test]
    fn templates_and_the_docs_branch_are_never_deployable() {
        let c = BranchClassifier::builtin();
        assert_eq!(c.classify("templates/symx-template"), BranchClass::Template);
        assert_eq!(c.classify("templates/core"), BranchClass::Template);
        assert_eq!(c.classify("templates/"), BranchClass::Template);
        assert_eq!(c.classify("templates/sit1"), BranchClass::Template);
        assert_eq!(c.classify(COMMON_DOCS_BRANCH), BranchClass::CommonDocs);
        // `template/x` (singular) is not under the prefix.
        assert_eq!(c.classify("template/x"), BranchClass::Other);
        // An admin pattern can never bring a template in (D22).
        let all = lists(&["*", "templates/*"], &[]);
        assert_eq!(all.classify("templates/core"), BranchClass::Template);
        assert_eq!(all.classify(COMMON_DOCS_BRANCH), BranchClass::CommonDocs);
    }

    #[test]
    fn an_admin_include_adds_names_and_an_exclude_takes_them_away() {
        let c = lists(&["uat*", "prod-eu"], &["sit9", "uat-old*"]);
        assert_eq!(c.classify("uat1"), BranchClass::Deployable);
        assert_eq!(c.classify("uat"), BranchClass::Deployable);
        assert_eq!(c.classify("prod-eu"), BranchClass::Deployable);
        assert_eq!(c.classify("prod-us"), BranchClass::Other);
        assert_eq!(
            c.classify("sit1"),
            BranchClass::Deployable,
            "the built-in rule stays"
        );
        // Exclude wins over the built-in rule and over an include.
        assert_eq!(c.classify("sit9"), BranchClass::Other);
        assert_eq!(c.classify("uat-old1"), BranchClass::Other);
        assert_eq!(c.classify("uat-new1"), BranchClass::Deployable);
    }

    #[test]
    fn a_name_that_cannot_be_a_tenant_id_is_never_deployable() {
        // The tenant id is the branch name, so a name the id cannot hold cannot be deployed, whatever an admin includes.
        let c = lists(&["*"], &[]);
        for name in [
            "feature/x",
            "Upper",
            "a.b",
            "has space",
            "",
            "-lead",
            "x".repeat(64).as_str(),
        ] {
            assert_eq!(c.classify(name), BranchClass::Other, "{name:?}");
        }
        assert_eq!(c.classify("anything-else"), BranchClass::Deployable);
    }

    #[test]
    fn the_tenant_of_a_deployable_branch_is_its_name() {
        let c = lists(&["uat*"], &[]);
        assert_eq!(
            c.tenant_of("sit3").map(|t| t.as_str().to_owned()),
            Some("sit3".to_owned())
        );
        assert_eq!(
            c.tenant_of("uat1").map(|t| t.as_str().to_owned()),
            Some("uat1".to_owned())
        );
        assert!(c.tenant_of("templates/core").is_none());
        assert!(c.tenant_of("main").is_none());
    }

    #[test]
    fn bad_patterns_are_refused_without_naming_them() {
        let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            BranchClassifier::new(&own(&["["]), &[]).unwrap_err(),
            BranchError::InvalidPattern
        );
        assert_eq!(
            BranchClassifier::new(&[], &own(&["{a"])).unwrap_err(),
            BranchError::InvalidPattern
        );
        assert_eq!(
            BranchClassifier::new(&["a".repeat(MAX_PATTERN_BYTES + 1)], &[]).unwrap_err(),
            BranchError::PatternTooLong
        );
        let many = vec!["a*".to_owned(); MAX_PATTERNS + 1];
        assert_eq!(
            BranchClassifier::new(&many, &[]).unwrap_err(),
            BranchError::TooManyPatterns
        );
        assert_eq!(
            BranchClassifier::new(&[], &many).unwrap_err(),
            BranchError::TooManyPatterns
        );
        for e in [
            BranchError::TooManyPatterns,
            BranchError::PatternTooLong,
            BranchError::InvalidPattern,
        ] {
            assert_eq!(RegistryError::from(e), RegistryError::Corrupt);
            assert!(!e.to_string().contains('['));
        }
    }

    #[test]
    fn the_class_maps_to_the_kind_the_mirror_uses() {
        assert_eq!(BranchClass::Deployable.kind(), BranchKind::Deployable);
        assert_eq!(BranchClass::Template.kind(), BranchKind::Template);
        assert_eq!(BranchClass::CommonDocs.kind(), BranchKind::CommonDocs);
        assert_eq!(BranchClass::Other.kind(), BranchKind::Other);
    }

    proptest! {
        /// Classification is total (it never panics, whatever the text) and exclusive in the way that matters: a template or the docs
        /// branch is never deployable, and a deployable branch is always a valid tenant id.
        #[test]
        fn classification_is_total_and_a_template_is_never_deployable(
            name in "\\PC{0,80}",
            include in proptest::collection::vec("[a-z*?]{1,8}", 0..4),
            exclude in proptest::collection::vec("[a-z*?]{1,8}", 0..4),
        ) {
            let c = BranchClassifier::new(&include, &exclude).unwrap();
            let class = c.classify(&name);
            if name.starts_with("templates/") {
                prop_assert_ne!(class, BranchClass::Deployable);
            }
            if class == BranchClass::Deployable {
                prop_assert!(TenantId::parse(&name).is_ok());
                prop_assert!(c.tenant_of(&name).is_some());
            } else {
                prop_assert!(c.tenant_of(&name).is_none());
            }
        }

        /// `sit<digits>` is deployable unless an admin excluded it, and an exclusion that matches wins.
        #[test]
        fn sit_digits_is_deployable_unless_excluded(n in 0u32..100_000) {
            let name = format!("sit{n}");
            prop_assert_eq!(BranchClassifier::builtin().classify(&name), BranchClass::Deployable);
            let excluded = BranchClassifier::new(&[], &[name.clone()]).unwrap();
            prop_assert_eq!(excluded.classify(&name), BranchClass::Other);
        }
    }
}
