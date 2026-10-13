//! Severity of a change or a finding (T9, D76, `docs/domain-model.md` "Severity").
//!
//! `severity(context, rules)` says how serious a change or a finding is: low, medium, high or critical, and which rules said
//! so. It is a fact like every other (D33): the same context and the same rules always give the same answer, and nothing in it
//! is an opinion of an AI layer.
//!
//! # Rules
//!
//! A [`SeverityRule`] has an id, a severity, an `enabled` flag and a [`SeverityMatch`]. A match lists conditions; a rule
//! matches a context when **all** the conditions it states hold (a condition left empty or `None` is not stated, and a rule
//! that states none matches everything). Within a list, **any** entry may hold.
//!
//! | Condition | Holds when |
//! |---|---|
//! | `finding_kinds` | the context is a finding of one of these kinds |
//! | `change_kinds` | the change has a setting change of one of these kinds |
//! | `path_glob` | the file's path matches the glob |
//! | `setting_glob` | one of the changed settings matches the glob |
//! | `drift_states` | the file's drift state is one of these |
//! | `file_classes` | the file's class is one of these |
//! | `root_file` | the file is (or is not) directly in the root of the config tree: its path has no `/` |
//!
//! A condition about something the context does not have (no path, no drift state) does not hold. Globs match the whole path
//! from the root, case-sensitively (file names on NFS are), and `*` crosses `/`, as every glob of the engine does; `**` is
//! the same as `*` and reads better in `security/**`.
//!
//! The first six conditions are what the API's `SeverityMatch` offers an admin, minus `drift_states`, `file_classes` and
//! `root_file`, which only the default rules use (plan decision A5).
//!
//! # Result
//!
//! The severity is the **highest** severity of the enabled rules that match, or low when none does, and `rule_ids` lists
//! every one of them, sorted. Taking the highest is what makes the rule list safe to extend: adding a rule can only add ids
//! and raise the severity, never lower it (`adding_a_rule_never_lowers_severity_prop`). It is also why the API's `priority`
//! does not enter here: with a maximum, order cannot matter, and the result does not depend on the order the rules are given
//! in either.
//!
//! # Bounds
//!
//! At most [`SeverityRules::MAX_RULES`] rules and [`SeverityRules::MAX_GLOB_BYTES`] bytes per glob. The globs of all rules are
//! compiled once, in [`SeverityRules::new`], into two sets, so a context is matched against all of them in one pass over its
//! path and one pass over its setting paths, whatever the number of rules. Errors name the problem and never the rule.

use std::fmt;

use domain::{ChangeKind, DriftState, FileClass, FindingKind, NfsPath, SettingPath, Severity, ShortText};
use globset::{Candidate, Glob, GlobSet, GlobSetBuilder};

use crate::error::SeverityError;

/// What `severity` is asked about: a change to a file, or a finding about one. Every field is optional information; a field
/// left at its default is "not known", and a rule that needs it does not match.
#[derive(Debug, Clone, Default)]
pub struct ChangeContext<'a> {
    /// The check that raised the finding, for a finding.
    pub finding_kind: Option<FindingKind>,
    /// The kinds of the setting changes in a change (`semantic_diff`), in any order and with repeats.
    pub change_kinds: &'a [ChangeKind],
    /// The file the change or the finding is about. For C3 it is the logical file.
    pub path: Option<&'a NfsPath>,
    /// The settings that changed, for `setting_glob`.
    pub setting_paths: &'a [SettingPath],
    /// The file's drift state.
    pub drift_state: Option<DriftState>,
    /// The file's class.
    pub file_class: Option<FileClass>,
}

impl ChangeContext<'_> {
    /// The file is directly in the root of the tree, so that it is read by every service (`application.yml`).
    fn is_root_file(&self) -> Option<bool> {
        self.path.map(|p| !p.is_root() && !p.as_str().contains('/'))
    }
}

/// The conditions of a rule. See the module documentation for what each one means.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeverityMatch {
    pub finding_kinds: Vec<FindingKind>,
    pub change_kinds: Vec<ChangeKind>,
    pub path_glob: Option<String>,
    pub setting_glob: Option<String>,
    pub drift_states: Vec<DriftState>,
    pub file_classes: Vec<FileClass>,
    pub root_file: Option<bool>,
}

/// One rule: when its conditions hold, the context is at least this serious.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeverityRule {
    /// Unique among the rules given together. The default rules' ids start with `default.`.
    pub id: ShortText,
    pub severity: Severity,
    /// A disabled rule is checked for errors and then ignored.
    pub enabled: bool,
    pub matcher: SeverityMatch,
}

/// The answer: the severity and the rules that gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeverityOutcome {
    pub severity: Severity,
    /// Every enabled rule that matched, sorted by id.
    pub rule_ids: Vec<ShortText>,
}

impl SeverityOutcome {
    /// No rule matched: low, with no ids.
    pub const fn none() -> Self {
        Self {
            severity: Severity::Low,
            rule_ids: Vec::new(),
        }
    }
}

/// An enabled rule, ready to be matched.
#[derive(Clone)]
struct Compiled {
    id: ShortText,
    severity: Severity,
    finding_kinds: Vec<FindingKind>,
    change_kinds: Vec<ChangeKind>,
    drift_states: Vec<DriftState>,
    file_classes: Vec<FileClass>,
    root_file: Option<bool>,
    has_path_glob: bool,
    has_setting_glob: bool,
}

/// A set of rules, validated and compiled once and used for many contexts.
#[derive(Clone)]
pub struct SeverityRules {
    /// The enabled rules, sorted by id.
    rules: Vec<Compiled>,
    /// All path globs; `path_owner[i]` is the index in `rules` of the rule glob `i` belongs to.
    paths: Option<GlobSet>,
    path_owner: Vec<usize>,
    settings: Option<GlobSet>,
    setting_owner: Vec<usize>,
}

impl SeverityRules {
    /// The most rules `new` takes.
    pub const MAX_RULES: usize = 1_000;
    /// The longest glob `new` takes, in bytes. It is the length the API allows.
    pub const MAX_GLOB_BYTES: usize = 256;

    /// Compiles the rules, in any order. The result does not depend on the order.
    ///
    /// # Errors
    ///
    /// `TooManyRules`, `DuplicateId`, `GlobTooLong`, or `InvalidGlob` for a glob that does not compile or a set that is too
    /// complex to. The error does not contain the rule. Rules that are disabled are checked as well.
    pub fn new(mut rules: Vec<SeverityRule>) -> Result<Self, SeverityError> {
        if rules.len() > Self::MAX_RULES {
            return Err(SeverityError::TooManyRules);
        }
        rules.sort_by(|a, b| a.id.cmp(&b.id));
        if rules.windows(2).any(|w| w[0].id == w[1].id) {
            return Err(SeverityError::DuplicateId);
        }

        let mut compiled = Vec::with_capacity(rules.len());
        let (mut paths, mut path_owner) = (GlobSetBuilder::new(), Vec::new());
        let (mut settings, mut setting_owner) = (GlobSetBuilder::new(), Vec::new());
        for rule in rules {
            let path_glob = rule.matcher.path_glob.as_deref().map(glob).transpose()?;
            let setting_glob = rule.matcher.setting_glob.as_deref().map(glob).transpose()?;
            if !rule.enabled {
                continue;
            }
            let at = compiled.len();
            let (has_path_glob, has_setting_glob) = (path_glob.is_some(), setting_glob.is_some());
            if let Some(g) = path_glob {
                paths.add(g);
                path_owner.push(at);
            }
            if let Some(g) = setting_glob {
                settings.add(g);
                setting_owner.push(at);
            }
            let m = rule.matcher;
            compiled.push(Compiled {
                id: rule.id,
                severity: rule.severity,
                finding_kinds: m.finding_kinds,
                change_kinds: m.change_kinds,
                drift_states: m.drift_states,
                file_classes: m.file_classes,
                root_file: m.root_file,
                has_path_glob,
                has_setting_glob,
            });
        }
        let build = |builder: GlobSetBuilder, owners: &[usize]| {
            if owners.is_empty() {
                Ok(None)
            } else {
                builder.build().map(Some).map_err(|_| SeverityError::InvalidGlob)
            }
        };
        Ok(Self {
            paths: build(paths, &path_owner)?,
            settings: build(settings, &setting_owner)?,
            rules: compiled,
            path_owner,
            setting_owner,
        })
    }

    /// The default rules and the admin's.
    ///
    /// # Errors
    ///
    /// As `new`. An admin rule with the id of a default rule is a `DuplicateId`.
    pub fn with_defaults(admin: Vec<SeverityRule>) -> Result<Self, SeverityError> {
        let mut all = default_rules();
        all.extend(admin);
        Self::new(all)
    }

    /// How many rules are in effect (the enabled ones).
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

impl fmt::Debug for SeverityRules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The globs are admin configuration; they are not printed.
        f.debug_struct("SeverityRules")
            .field("rules", &self.rules.len())
            .finish_non_exhaustive()
    }
}

fn glob(text: &str) -> Result<Glob, SeverityError> {
    if text.len() > SeverityRules::MAX_GLOB_BYTES {
        return Err(SeverityError::GlobTooLong);
    }
    Glob::new(text).map_err(|_| SeverityError::InvalidGlob)
}

/// A condition that lists values holds when it lists none, or the context has a value that it lists.
fn holds<T: PartialEq>(listed: &[T], actual: Option<&T>) -> bool {
    listed.is_empty() || actual.is_some_and(|a| listed.contains(a))
}

/// The severity of a change or a finding, and the rules that gave it. See the module documentation.
pub fn severity(context: &ChangeContext<'_>, rules: &SeverityRules) -> SeverityOutcome {
    // Which rules' path glob and setting glob hold for this context, found with one pass each over all the rules' globs.
    let mut path_hit = vec![false; rules.rules.len()];
    if let (Some(set), Some(path)) = (&rules.paths, context.path) {
        let mut hits = Vec::new();
        set.matches_candidate_into(&Candidate::new(path.as_str()), &mut hits);
        for glob_at in hits {
            if let Some(hit) = rules.path_owner.get(glob_at).and_then(|&r| path_hit.get_mut(r)) {
                *hit = true;
            }
        }
    }
    let mut setting_hit = vec![false; rules.rules.len()];
    if let Some(set) = &rules.settings {
        let mut hits = Vec::new();
        let mut open = rules.setting_owner.len();
        for setting in context.setting_paths {
            if open == 0 {
                break;
            }
            hits.clear();
            set.matches_candidate_into(&Candidate::new(setting.as_str()), &mut hits);
            for &glob_at in &hits {
                if let Some(hit) = rules
                    .setting_owner
                    .get(glob_at)
                    .and_then(|&r| setting_hit.get_mut(r))
                {
                    if !*hit {
                        *hit = true;
                        open -= 1;
                    }
                }
            }
        }
    }

    let root = context.is_root_file();
    let mut out = SeverityOutcome::none();
    for (at, rule) in rules.rules.iter().enumerate() {
        let matches = holds(&rule.finding_kinds, context.finding_kind.as_ref())
            && (rule.change_kinds.is_empty()
                || context.change_kinds.iter().any(|k| rule.change_kinds.contains(k)))
            && holds(&rule.drift_states, context.drift_state.as_ref())
            && holds(&rule.file_classes, context.file_class.as_ref())
            && rule.root_file.is_none_or(|want| root == Some(want))
            && (!rule.has_path_glob || path_hit.get(at).copied().unwrap_or(false))
            && (!rule.has_setting_glob || setting_hit.get(at).copied().unwrap_or(false));
        if matches {
            out.severity = out.severity.max(rule.severity);
            out.rule_ids.push(rule.id.clone());
        }
    }
    out
}

fn rule(id: &str, severity: Severity, matcher: SeverityMatch) -> Option<SeverityRule> {
    Some(SeverityRule {
        id: ShortText::parse(id).ok()?,
        severity,
        enabled: true,
        matcher,
    })
}

/// The default rules of `docs/domain-model.md`, "Severity (D76)", one rule per line of the list (a line with two kinds of
/// file gets two rules where one condition cannot say both):
///
/// - critical: any change under `security/**`; an overwritten NFS change (C7); a denied file;
/// - high: a root file changing (a change of settings, or a drift state other than in sync); keys removed; a value changing
///   type; a conflict; an environment host mismatch (C6);
/// - medium: any other out-of-band change (the file is ahead on NFS, untracked, or of unknown state); missed base changes
///   (C1);
/// - low: redundant or uneven tenant files (C2, C3); duplicate keys (C8).
///
/// A context that none of them matches is low. The rules do not include the C9 to C11 checks, which the list does not rank.
pub fn default_rules() -> Vec<SeverityRule> {
    use ChangeKind as C;
    use DriftState as D;
    use FindingKind as F;
    let changes = vec![C::Added, C::Removed, C::Changed, C::TypeChanged];
    let rules = [
        rule(
            "default.security_path",
            Severity::Critical,
            SeverityMatch {
                path_glob: Some("security/**".to_owned()),
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.overwritten_nfs_change",
            Severity::Critical,
            SeverityMatch {
                finding_kinds: vec![F::OverwrittenNfsChange],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.denied_file",
            Severity::Critical,
            SeverityMatch {
                file_classes: vec![FileClass::Denied],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.root_file_change",
            Severity::High,
            SeverityMatch {
                root_file: Some(true),
                change_kinds: changes,
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.root_file_drift",
            Severity::High,
            SeverityMatch {
                root_file: Some(true),
                drift_states: vec![D::GitAhead, D::NfsAhead, D::Conflict, D::Unknown, D::Untracked],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.removed_keys",
            Severity::High,
            SeverityMatch {
                change_kinds: vec![C::Removed],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.type_change",
            Severity::High,
            SeverityMatch {
                change_kinds: vec![C::TypeChanged],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.conflict",
            Severity::High,
            SeverityMatch {
                drift_states: vec![D::Conflict],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.environment_host_mismatch",
            Severity::High,
            SeverityMatch {
                finding_kinds: vec![F::EnvironmentHostMismatch],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.out_of_band",
            Severity::Medium,
            SeverityMatch {
                drift_states: vec![D::NfsAhead, D::Unknown, D::Untracked],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.missed_base_changes",
            Severity::Medium,
            SeverityMatch {
                finding_kinds: vec![F::MissedBaseChanges],
                ..SeverityMatch::default()
            },
        ),
        rule(
            "default.tenant_file_hygiene",
            Severity::Low,
            SeverityMatch {
                finding_kinds: vec![F::RedundantTenantFile, F::UnevenTenantFiles, F::DuplicateKeys],
                ..SeverityMatch::default()
            },
        ),
    ];
    // Every id above is short and has no control character, so none is dropped; a test counts them.
    rules.into_iter().flatten().collect()
}
