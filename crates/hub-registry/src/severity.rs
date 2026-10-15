//! Severity (T9, D76, D103, rule 10): the rules the registry rates changes and findings with, where they come from, and what is rated.
//!
//! The engine decides what a severity is ([`engine::severity`]: the highest of the enabled rules that match, low when none does). This
//! module is the registry's side of it: the rules an admin has stored are loaded from `severity_rules` on top of the engine's defaults
//! ([`SeverityRuleSource`]), one set of them is in force for a computation ([`Loaded`]), and the pipeline asks it what a file's change is
//! worth ([`file_severity`]). A finding is rated by the pipeline with the same set (`pipeline::findings_sync`), and the notifier's
//! priority is the severity of those findings, so the three always agree.
//!
//! # What an admin's rule can say
//!
//! A row is `(id, severity, enabled, matcher)`. The matcher is the contract's `SeverityMatch` (`api/openapi.yaml`): `finding_kinds`,
//! `change_kinds`, `path_glob` and `setting_glob`, and nothing else. It is **data**: it is parsed into closed types, an unknown key or a
//! kind that does not exist makes the row unusable, a glob is compiled by the engine (linear time, bounded in length and count) and never
//! run as anything else, the words of a row are never part of a statement, a log line or a decision other than the one the matcher
//! states. A row that cannot be used is **skipped and counted**, the others apply: one bad row must not leave a swimlane without
//! severities or stop a batch (the same rule as the host rules of the pipeline). Nothing about a skipped row is logged but the count.
//!
//! - The engine's defaults always apply (D76). An admin's rows are added to them; the engine takes the highest severity, so a row can
//!   raise what a default says and never lower it (the engine's choice, plan 04 A16), and a row cannot switch a default off.
//! - Rows with `is_default` are not read: they stand for the defaults, which the engine has, and a row with the id of a default is
//!   skipped, so a seeded copy cannot collide with the original.
//! - `scope` is not read. The contract's rule has no scope: a rule applies to every context it matches, and its own conditions
//!   (`finding_kinds` only match a finding, `change_kinds` only a change) are what restricts it.
//! - `description` and `position` are not read: neither can change what a rule rates (with a maximum, the order of the rules cannot).
//!
//! # The fingerprint
//!
//! [`Loaded::fingerprint`] names the set that is in force: the engine's defaults and the admin's rules in the form the engine is given,
//! each with everything that can change a result and nothing else (not the words of a description, not the place in the list, not the
//! order of the kinds in a condition). It is an input of every swimlane's rules fingerprint (`pipeline::rules`), so a change of the rows
//! makes the pipeline compute the whole swimlane again, an upgrade that changes a default does too, and a change that rates nothing
//! differently costs nothing.
//!
//! # What is rated
//!
//! [`file_severity`]: a file whose drift state says that something is pending (anything but `in_sync` and `intentional_divergence`) is
//! rated with its path, its drift state and its class. A file that is in sync, or whose divergence a person has accepted, has no
//! severity: there is no change to rate (it is NULL, and the tree shows a folder's as the worst below it). The registry does not
//! run a semantic diff on the hot path, so `change_kinds` and `setting_glob` have nothing to match there: the defaults that need them
//! (keys removed, a value changing type) and an admin's conditions on settings are for the contexts that have a diff (D103).
//!
//! # Reading the rules
//!
//! One bounded statement per use, and the compiled set is kept while the rows read are the same ones, so a batch pays one query and no
//! compilation. A source that cannot read the table fails the batch that asked for it (the pipeline) or uses the last set it read, or the
//! defaults (a notification must still go out): [`SeverityRuleSource::current_or_last`].

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use domain::{ChangeKind, ContentHash, DriftState, FileClass, FindingKind, NfsPath, Severity, ShortText};
use engine::{ChangeContext, SeverityMatch, SeverityRule, SeverityRules, default_rules, severity};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tracing::{info, warn};

use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::error::RegistryError;
use crate::metrics::RegistryMetrics;

/// The most kinds one condition lists. The contract allows 16 and 8; this only bounds what is read.
const MAX_KINDS: usize = 32;

/// The admin's rules, in the order the contract gives them (`priority`, then the id), at most one more than the engine can take.
pub const RULES_SQL: &str = "\
SELECT id, severity, enabled, matcher FROM severity_rules WHERE NOT is_default ORDER BY position, id LIMIT $1";

/// A row of [`RULES_SQL`].
type Row = (String, String, bool, Value);

/// The most admin rules the engine can take next to its defaults.
fn capacity() -> usize {
    SeverityRules::MAX_RULES.saturating_sub(default_rules().len())
}

/// The contract's `SeverityMatch`, as stored. Closed: a key the contract does not have is an error, not something that is ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatcherJson {
    #[serde(default)]
    finding_kinds: Vec<FindingKind>,
    #[serde(default)]
    change_kinds: Vec<ChangeKind>,
    #[serde(default)]
    path_glob: Option<String>,
    #[serde(default)]
    setting_glob: Option<String>,
}

/// A row as an engine rule, or `None` when it is not one. Never says why: the words of a row are not repeated anywhere.
fn rule_of(id: &str, severity: &str, enabled: bool, matcher: Value) -> Option<SeverityRule> {
    if id.is_empty() {
        return None;
    }
    let id = ShortText::parse(id).ok()?;
    let severity = severity.parse::<Severity>().ok()?;
    // An object and nothing else: serde would also read a struct from a list, where `[]` is a rule that states no condition.
    if !matcher.is_object() {
        return None;
    }
    let m: MatcherJson = serde_json::from_value(matcher).ok()?;
    if m.finding_kinds.len() > MAX_KINDS || m.change_kinds.len() > MAX_KINDS {
        return None;
    }
    Some(SeverityRule {
        id,
        severity,
        enabled,
        matcher: SeverityMatch {
            finding_kinds: m.finding_kinds,
            change_kinds: m.change_kinds,
            path_glob: m.path_glob,
            setting_glob: m.setting_glob,
            ..SeverityMatch::default()
        },
    })
}

fn change_name(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "added",
        ChangeKind::Removed => "removed",
        ChangeKind::Changed => "changed",
        ChangeKind::TypeChanged => "type_changed",
    }
}

/// A field of a hash that cannot be confused with its neighbours: a tag and a value, each with its length.
fn put(hash: &mut Sha256, tag: &str, value: &[u8]) {
    hash.update((tag.len() as u64).to_be_bytes());
    hash.update(tag.as_bytes());
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

/// A condition that lists values: the set of them, so that their order and repeats do not matter.
fn put_set(hash: &mut Sha256, tag: &str, names: impl Iterator<Item = &'static str>) {
    let set: BTreeSet<&str> = names.collect();
    put(hash, tag, &(set.len() as u64).to_be_bytes());
    for name in set {
        put(hash, tag, name.as_bytes());
    }
}

/// The name of a set of rules: everything that can change what they rate, and nothing that cannot (see the module documentation).
pub fn fingerprint(rules: &[SeverityRule]) -> ContentHash {
    let mut sorted: Vec<&SeverityRule> = rules.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut hash = Sha256::new();
    put(&mut hash, "severity-rules", &[1]);
    put(&mut hash, "count", &(sorted.len() as u64).to_be_bytes());
    for rule in sorted {
        let m = &rule.matcher;
        put(&mut hash, "id", rule.id.as_str().as_bytes());
        put(&mut hash, "severity", rule.severity.as_str().as_bytes());
        put(&mut hash, "enabled", &[u8::from(rule.enabled)]);
        put_set(&mut hash, "finding", m.finding_kinds.iter().map(|k| k.as_str()));
        put_set(
            &mut hash,
            "change",
            m.change_kinds.iter().map(|k| change_name(*k)),
        );
        put_set(&mut hash, "state", m.drift_states.iter().map(|s| s.as_str()));
        put_set(&mut hash, "class", m.file_classes.iter().map(|c| c.as_str()));
        // A glob that is absent and one that is empty are different rules.
        put(
            &mut hash,
            "path",
            m.path_glob.as_deref().map_or(&[0][..], |g| g.as_bytes()),
        );
        put(&mut hash, "path-set", &[u8::from(m.path_glob.is_some())]);
        put(
            &mut hash,
            "setting",
            m.setting_glob.as_deref().map_or(&[0][..], |g| g.as_bytes()),
        );
        put(&mut hash, "setting-set", &[u8::from(m.setting_glob.is_some())]);
        put(
            &mut hash,
            "root",
            &[match m.root_file {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            }],
        );
    }
    ContentHash::from_bytes(hash.finalize().into())
}

/// A set of rules in force: compiled once, with the name of the set.
pub struct Loaded {
    /// The engine's defaults and the admin's rules, compiled.
    pub rules: Arc<SeverityRules>,
    pub fingerprint: ContentHash,
    /// Admin rules in effect, and rows that were not applied.
    pub admin: usize,
    pub skipped: usize,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("admin", &self.admin)
            .field("skipped", &self.skipped)
            .finish_non_exhaustive()
    }
}

impl Loaded {
    /// The engine's defaults and `admin`, or `None` when the engine refuses the set.
    fn of(admin: &[SeverityRule], skipped: usize) -> Option<Self> {
        let mut all = default_rules();
        all.extend(admin.iter().cloned());
        let rules = SeverityRules::new(all.clone()).ok()?;
        Some(Self {
            rules: Arc::new(rules),
            fingerprint: fingerprint(&all),
            admin: admin.len(),
            skipped,
        })
    }

    /// The set of the rows `rows`: the rows that are rules, each checked alone so that one bad glob costs one row. If the engine refuses
    /// the set as a whole (too complex together), the defaults stand and every row counts as skipped.
    fn from_rows(rows: Vec<Row>, defaults: &Self) -> Self {
        let default_ids: BTreeSet<ShortText> = default_rules().into_iter().map(|r| r.id).collect();
        let room = capacity();
        let mut skipped = rows.len().saturating_sub(room);
        let mut admin = Vec::new();
        for (id, severity, enabled, matcher) in rows.into_iter().take(room) {
            match rule_of(&id, &severity, enabled, matcher) {
                Some(rule)
                    if !default_ids.contains(&rule.id) && SeverityRules::new(vec![rule.clone()]).is_ok() =>
                {
                    admin.push(rule);
                }
                _ => skipped += 1,
            }
        }
        let given = admin.len();
        Self::of(&admin, skipped).unwrap_or_else(|| Self {
            rules: Arc::clone(&defaults.rules),
            fingerprint: defaults.fingerprint,
            admin: 0,
            skipped: skipped + given,
        })
    }
}

/// What the cache remembers: the hash of the rows read, and the set compiled from them.
struct Cached {
    key: ContentHash,
    loaded: Arc<Loaded>,
}

struct Inner {
    db: Db,
    metrics: RegistryMetrics,
    defaults: Arc<Loaded>,
    cache: Mutex<Option<Cached>>,
}

/// Where a computation gets the rules it rates with. Cloning shares the compiled set.
#[derive(Clone)]
pub struct SeverityRuleSource {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SeverityRuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeverityRuleSource").finish_non_exhaustive()
    }
}

/// The hash of the rows as they were read: all of each row, so that any change is a miss, and a row the engine cannot use is a key too
/// (so it is counted once and not at every batch).
fn key_of(rows: &[Row]) -> ContentHash {
    let mut hash = Sha256::new();
    put(&mut hash, "rows", &(rows.len() as u64).to_be_bytes());
    for (id, severity, enabled, matcher) in rows {
        put(&mut hash, "id", id.as_bytes());
        put(&mut hash, "severity", severity.as_bytes());
        put(&mut hash, "enabled", &[u8::from(*enabled)]);
        put(&mut hash, "matcher", matcher.to_string().as_bytes());
    }
    ContentHash::from_bytes(hash.finalize().into())
}

impl SeverityRuleSource {
    /// # Errors
    ///
    /// The engine refuses its own default rules, or the configuration is out of range.
    pub fn new(pool: PgPool, config: &RegistryConfig, metrics: RegistryMetrics) -> Result<Self, ConfigError> {
        let defaults = Loaded::of(&[], 0).ok_or(ConfigError::Invalid("severity_rules"))?;
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(pool, config.query_timeout),
                metrics,
                defaults: Arc::new(defaults),
                cache: Mutex::new(None),
            }),
        })
    }

    /// The engine's defaults alone.
    pub fn defaults(&self) -> Arc<Loaded> {
        Arc::clone(&self.inner.defaults)
    }

    /// The set in force now: the rows are read (one bounded statement), and the compiled set of the last read is used again if the rows
    /// are the same.
    ///
    /// # Errors
    ///
    /// The table could not be read in time. Nothing is guessed: the caller decides what to do without rules.
    pub async fn current(&self) -> Result<Arc<Loaded>, RegistryError> {
        let limit = i64::try_from(capacity() + 1).unwrap_or(i64::MAX);
        let rows: Vec<Row> = self
            .inner
            .db
            .run(
                "severity_rules",
                sqlx::query_as(RULES_SQL)
                    .bind(limit)
                    .fetch_all(self.inner.db.pool()),
            )
            .await?;
        let key = key_of(&rows);
        if let Some(hit) = self.cached(&key) {
            return Ok(hit);
        }
        let loaded = Arc::new(Loaded::from_rows(rows, &self.inner.defaults));
        self.inner.metrics.severity_rules(loaded.admin, loaded.skipped);
        if loaded.skipped > 0 {
            warn!(
                admin = loaded.admin,
                skipped = loaded.skipped,
                "rows of severity_rules were not applied: the engine cannot use them"
            );
        } else {
            info!(admin = loaded.admin, "severity rules compiled");
        }
        if let Ok(mut cache) = self.inner.cache.lock() {
            *cache = Some(Cached {
                key,
                loaded: Arc::clone(&loaded),
            });
        }
        Ok(loaded)
    }

    /// [`current`](Self::current), or when the table cannot be read the last set that was read, or the defaults. For what must go on
    /// without a database answer (a notification), never for what writes a state.
    pub async fn current_or_last(&self) -> Arc<Loaded> {
        match self.current().await {
            Ok(loaded) => loaded,
            Err(error) => {
                warn!(
                    ?error,
                    "the severity rules could not be read: the last set read, or the defaults, are used"
                );
                self.last()
            }
        }
    }

    fn cached(&self, key: &ContentHash) -> Option<Arc<Loaded>> {
        let cache = self.inner.cache.lock().ok()?;
        cache
            .as_ref()
            .filter(|c| c.key == *key)
            .map(|c| Arc::clone(&c.loaded))
    }

    fn last(&self) -> Arc<Loaded> {
        self.inner
            .cache
            .lock()
            .ok()
            .and_then(|c| c.as_ref().map(|c| Arc::clone(&c.loaded)))
            .unwrap_or_else(|| Arc::clone(&self.inner.defaults))
    }
}

/// What a file's pending change is worth under `rules`, or `None` when nothing is pending: the file is in sync, or a person has accepted
/// its divergence (D20). Rated with its path, its drift state and its class (see the module documentation).
pub fn file_severity(
    rules: &SeverityRules,
    path: &NfsPath,
    state: DriftState,
    class: FileClass,
) -> Option<Severity> {
    if matches!(state, DriftState::InSync | DriftState::IntentionalDivergence) {
        return None;
    }
    Some(
        severity(
            &ChangeContext {
                path: Some(path),
                drift_state: Some(state),
                file_class: Some(class),
                ..ChangeContext::default()
            },
            rules,
        )
        .severity,
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rules() -> SeverityRules {
        SeverityRules::new(default_rules()).unwrap()
    }

    fn path(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    fn row(id: &str, severity: &str, enabled: bool, matcher: Value) -> Row {
        (id.to_owned(), severity.to_owned(), enabled, matcher)
    }

    fn defaults() -> Loaded {
        Loaded::of(&[], 0).unwrap()
    }

    #[test]
    fn a_row_with_the_contracts_matcher_is_a_rule() {
        let r = rule_of(
            "payments",
            "critical",
            true,
            json!({
                "finding_kinds": ["drift", "orphan_file"],
                "change_kinds": ["removed"],
                "path_glob": "payments/**",
                "setting_glob": null
            }),
        )
        .unwrap();
        assert_eq!(r.id.as_str(), "payments");
        assert_eq!(r.severity, Severity::Critical);
        assert!(r.enabled);
        assert_eq!(
            r.matcher.finding_kinds,
            [FindingKind::Drift, FindingKind::OrphanFile]
        );
        assert_eq!(r.matcher.change_kinds, [ChangeKind::Removed]);
        assert_eq!(r.matcher.path_glob.as_deref(), Some("payments/**"));
        assert_eq!(r.matcher.setting_glob, None);
        // The conditions the contract does not offer an admin are not set from a row.
        assert!(r.matcher.drift_states.is_empty() && r.matcher.file_classes.is_empty());
        assert_eq!(r.matcher.root_file, None);

        let empty = rule_of("everything", "low", false, json!({})).unwrap();
        assert!(!empty.enabled);
        assert_eq!(
            empty.matcher,
            SeverityMatch::default(),
            "a rule that states nothing is the engine's: it matches all"
        );
    }

    #[test]
    fn a_row_that_is_not_a_rule_is_none_and_says_nothing_about_why() {
        let long = "x".repeat(300);
        for bad in [
            rule_of("a", "urgent", true, json!({})),
            rule_of("", "low", true, json!({})),
            rule_of("a\nb", "low", true, json!({})),
            rule_of(&long, "low", true, json!({})),
            rule_of("a", "low", true, json!(["payments/**"])),
            rule_of("a", "low", true, json!([])),
            rule_of("a", "low", true, json!([[], [], null, null])),
            rule_of("a", "low", true, json!("payments/**")),
            rule_of("a", "low", true, json!(null)),
            rule_of("a", "low", true, json!({"path_glob": 5})),
            rule_of("a", "low", true, json!({"path_glob": "a", "run": "x"})),
            rule_of("a", "low", true, json!({"drift_states": ["conflict"]})),
            rule_of("a", "low", true, json!({"root_file": true})),
            rule_of("a", "low", true, json!({"finding_kinds": ["nope"]})),
            rule_of("a", "low", true, json!({"change_kinds": ["renamed"]})),
            rule_of(
                "a",
                "low",
                true,
                json!({"finding_kinds": vec!["drift"; MAX_KINDS + 1]}),
            ),
        ] {
            assert!(bad.is_none(), "{bad:?}");
        }
    }

    #[test]
    fn rows_are_checked_one_by_one_and_a_bad_one_costs_only_itself() {
        let rows = vec![
            row("good", "high", true, json!({"path_glob": "payments/**"})),
            row(
                "bad-glob",
                "critical",
                true,
                json!({"path_glob": "payments/[unclosed"}),
            ),
            row(
                "too-long-glob",
                "critical",
                true,
                json!({"path_glob": "a".repeat(300)}),
            ),
            row("not-a-matcher", "critical", true, json!([])),
            row("default.conflict", "low", true, json!({})),
            row("off", "critical", false, json!({"path_glob": "app/**"})),
        ];
        let loaded = Loaded::from_rows(rows, &defaults());
        assert_eq!(loaded.admin, 2, "the good one and the one that is switched off");
        assert_eq!(loaded.skipped, 4);
        let high = |p: &str| {
            file_severity(
                &loaded.rules,
                &path(p),
                DriftState::GitAhead,
                FileClass::Structured,
            )
        };
        assert_eq!(high("payments/a.yml"), Some(Severity::High));
        assert_eq!(
            high("app/a.yml"),
            Some(Severity::Low),
            "a rule that is off rates nothing"
        );
        assert_ne!(loaded.fingerprint, defaults().fingerprint);
        assert_eq!(
            file_severity(
                &loaded.rules,
                &path("a.yml"),
                DriftState::Conflict,
                FileClass::Structured
            ),
            Some(Severity::High),
            "the default `conflict` rule is still the engine's: a row cannot take its id and say another thing"
        );
    }

    #[test]
    fn more_rows_than_the_engine_can_take_are_cut_and_counted() {
        let room = capacity();
        assert!(room > 100, "{room}");
        let rows: Vec<Row> = (0..room + 3)
            .map(|i| {
                row(
                    &format!("r{i:04}"),
                    "low",
                    true,
                    json!({"path_glob": format!("p{i}/**")}),
                )
            })
            .collect();
        let loaded = Loaded::from_rows(rows, &defaults());
        assert_eq!((loaded.admin, loaded.skipped), (room, 3));
        assert_eq!(loaded.rules.len(), room + default_rules().len());
    }

    #[test]
    fn the_fingerprint_follows_what_a_rule_rates_and_nothing_else() {
        let base = vec![
            rule_of(
                "a",
                "high",
                true,
                json!({"finding_kinds": ["drift", "orphan_file"], "path_glob": "x/**"}),
            )
            .unwrap(),
            rule_of("b", "low", true, json!({"change_kinds": ["added", "removed"]})).unwrap(),
        ];
        let f = |rules: &[SeverityRule]| fingerprint(rules);
        assert_eq!(f(&base), f(&base));

        // The order of the rules, of the kinds in a condition, and repeats do not change what is rated.
        let mut shuffled = base.clone();
        shuffled.reverse();
        shuffled[1].matcher.finding_kinds.reverse();
        shuffled[0].matcher.change_kinds.push(ChangeKind::Added);
        assert_eq!(f(&shuffled), f(&base));

        // Everything that changes a result does.
        let mut changed: Vec<Vec<SeverityRule>> = Vec::new();
        for edit in 0..11 {
            let mut r = base.clone();
            match edit {
                0 => r[0].severity = Severity::Critical,
                1 => r[0].enabled = false,
                2 => r[0].id = ShortText::parse("a2").unwrap(),
                3 => drop(r[0].matcher.finding_kinds.pop()),
                4 => r[0].matcher.path_glob = Some("y/**".to_owned()),
                5 => r[0].matcher.path_glob = None,
                6 => r[0].matcher.setting_glob = Some("*.password".to_owned()),
                7 => drop(r[1].matcher.change_kinds.pop()),
                8 => r[1].matcher.drift_states.push(DriftState::Conflict),
                9 => r[1].matcher.file_classes.push(FileClass::Denied),
                _ => r[1].matcher.root_file = Some(true),
            }
            changed.push(r);
        }
        changed.push(base[..1].to_vec());
        changed.push([base.clone(), vec![rule_of("c", "low", true, json!({})).unwrap()]].concat());
        for (i, other) in changed.iter().enumerate() {
            assert_ne!(f(other), f(&base), "edit {i}");
        }
        // A glob that is absent is not a glob that is empty.
        let mut empty_glob = base.clone();
        empty_glob[0].matcher.path_glob = Some(String::new());
        let mut no_glob = base.clone();
        no_glob[0].matcher.path_glob = None;
        assert_ne!(f(&empty_glob), f(&no_glob));
    }

    #[test]
    fn the_same_rows_are_the_same_set_and_the_statement_reads_no_words() {
        // Neither is read, so the same rows give the same set: this is what keeps a rewording from recomputing forty swimlanes.
        let a = Loaded::from_rows(
            vec![row("x", "high", true, json!({"path_glob": "p/**"}))],
            &defaults(),
        );
        let b = Loaded::from_rows(
            vec![row("x", "high", true, json!({"path_glob": "p/**"}))],
            &defaults(),
        );
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(
            key_of(&[row("x", "high", true, json!({"a": 1, "b": 2}))]),
            key_of(&[row("x", "high", true, json!({"b": 2, "a": 1}))])
        );
        assert_ne!(
            key_of(&[row("x", "high", true, json!({}))]),
            key_of(&[row("x", "high", false, json!({}))])
        );
        assert_ne!(key_of(&[]), key_of(&[row("x", "high", true, json!({}))]));
        assert!(RULES_SQL.contains("NOT is_default") && !RULES_SQL.contains("description"));
    }

    #[test]
    fn the_defaults_alone_rate_a_file_by_its_state_its_class_and_its_path() {
        use DriftState as D;
        use FileClass as C;
        use Severity::{Critical, High, Low, Medium};
        let r = rules();
        let sev = |p: &str, s, c| file_severity(&r, &path(p), s, c);
        // Nothing pending: no severity, whatever the path or the class.
        for state in [D::InSync, D::IntentionalDivergence] {
            assert_eq!(sev("security/a.yml", state, C::Structured), None, "{state:?}");
            assert_eq!(sev("application.yml", state, C::Structured), None);
            assert_eq!(sev("k.key", state, C::Denied), None);
        }
        assert_eq!(
            sev("app/a.yml", D::NfsAhead, C::Structured),
            Some(Medium),
            "an out-of-band change"
        );
        assert_eq!(sev("app/a.yml", D::Untracked, C::Text), Some(Medium));
        assert_eq!(sev("app/a.yml", D::Unknown, C::Structured), Some(Medium));
        assert_eq!(
            sev("app/a.yml", D::GitAhead, C::Structured),
            Some(Low),
            "waiting for its sync: no default rule"
        );
        assert_eq!(sev("app/a.yml", D::Conflict, C::Structured), Some(High));
        assert_eq!(
            sev("security/a.yml", D::GitAhead, C::Structured),
            Some(Critical),
            "anything under security/**"
        );
        assert_eq!(
            sev("keys/server.pem", D::NfsAhead, C::Denied),
            Some(Critical),
            "a denied file changing"
        );
        assert_eq!(
            sev("application.yml", D::GitAhead, C::Structured),
            Some(High),
            "a root file"
        );
        assert_eq!(
            sev("application-sit1.yml", D::NfsAhead, C::Structured),
            Some(High),
            "a root file of a tenant"
        );
        assert_eq!(
            sev("app/application.yml", D::NfsAhead, C::Structured),
            Some(Medium),
            "only the root is the root"
        );
    }

    #[test]
    fn an_admins_rule_can_raise_what_a_default_says_and_never_lower_it() {
        let admin = vec![
            rule_of("raise", "critical", true, json!({"path_glob": "payments/**"})).unwrap(),
            rule_of("lower", "low", true, json!({"path_glob": "security/**"})).unwrap(),
        ];
        let loaded = Loaded::of(&admin, 0).unwrap();
        let sev = |p: &str| {
            file_severity(
                &loaded.rules,
                &path(p),
                DriftState::NfsAhead,
                FileClass::Structured,
            )
        };
        assert_eq!(sev("payments/a.yml"), Some(Severity::Critical));
        assert_eq!(
            sev("security/a.yml"),
            Some(Severity::Critical),
            "the engine takes the highest (plan 04 A16)"
        );
    }

    mod property {
        use proptest::prelude::*;

        use super::*;

        /// Any JSON at all, nested a few levels.
        fn any_json() -> impl Strategy<Value = Value> {
            let leaf = prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::Bool),
                any::<i64>().prop_map(|n| json!(n)),
                "[ -~]{0,12}".prop_map(Value::String),
            ];
            leaf.prop_recursive(3, 24, 4, |inner| {
                prop_oneof![
                    prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                    prop::collection::btree_map("[a-z_]{1,14}", inner, 0..4)
                        .prop_map(|m| Value::Object(m.into_iter().collect())),
                ]
            })
        }

        /// A matcher that is nearly the contract's: its keys, with globs of the characters that matter and kinds that may not exist.
        fn near_matcher() -> impl Strategy<Value = Value> {
            (
                prop::option::of("[a-z*/\\[\\]{},?!]{0,10}"),
                prop::option::of("[a-z*.]{0,8}"),
                prop::collection::vec(prop::sample::select(vec!["drift", "orphan_file", "nope"]), 0..3),
                prop::collection::vec(prop::sample::select(vec!["added", "removed", "renamed"]), 0..3),
            )
                .prop_map(|(path, setting, findings, changes)| {
                    json!({"path_glob": path, "setting_glob": setting, "finding_kinds": findings, "change_kinds": changes})
                })
        }

        fn rows() -> impl Strategy<Value = Vec<Row>> {
            prop::collection::vec(
                (
                    "[a-z.\\-]{0,9}",
                    prop::sample::select(vec!["low", "medium", "high", "critical", "urgent"]),
                    any::<bool>(),
                    prop_oneof![any_json(), near_matcher()],
                ),
                0..8,
            )
            .prop_map(|v| {
                v.into_iter()
                    .map(|(id, severity, enabled, matcher)| (id, severity.to_owned(), enabled, matcher))
                    .collect()
            })
        }

        const CONTEXTS: [(&str, DriftState, FileClass); 8] = [
            ("app/a.yml", DriftState::NfsAhead, FileClass::Structured),
            ("app/a.yml", DriftState::GitAhead, FileClass::Text),
            ("security/a.yml", DriftState::Unknown, FileClass::Structured),
            ("application.yml", DriftState::Conflict, FileClass::Structured),
            ("keys/server.pem", DriftState::Untracked, FileClass::Denied),
            ("payments/rates.yml", DriftState::NfsAhead, FileClass::Structured),
            ("a.yml", DriftState::InSync, FileClass::Structured),
            ("a.yml", DriftState::IntentionalDivergence, FileClass::Structured),
        ];

        proptest! {
            /// Whatever a row holds, reading it is not a panic and gives a rule or nothing.
            #[test]
            fn no_json_makes_a_rule_panic(id in "[ -~]{0,20}", enabled in any::<bool>(), matcher in any_json()) {
                let _ = rule_of(&id, "high", enabled, matcher);
            }

            /// Whatever the rows are, the defaults stand: the rows can only raise what the defaults say, in every context, and what is not
            /// pending has no severity, with any rows. The count of rows in effect and skipped never exceeds the rows given.
            #[test]
            fn rows_never_lower_what_the_defaults_say(rows in rows()) {
                let defaults = defaults();
                let given = rows.len();
                let loaded = Loaded::from_rows(rows, &defaults);
                prop_assert!(loaded.admin + loaded.skipped <= given);
                for (p, state, class) in CONTEXTS {
                    let with = file_severity(&loaded.rules, &path(p), state, class);
                    let without = file_severity(&defaults.rules, &path(p), state, class);
                    prop_assert!(with >= without, "{p} {state:?}: {with:?} < {without:?}");
                    if matches!(state, DriftState::InSync | DriftState::IntentionalDivergence) {
                        prop_assert_eq!(with, None);
                    }
                }
            }

            /// The same rows are the same set: the fingerprint is a function of what the rows say.
            #[test]
            fn the_same_rows_have_the_same_fingerprint(rows in rows()) {
                let d = defaults();
                let a = Loaded::from_rows(rows.clone(), &d);
                let b = Loaded::from_rows(rows.clone(), &d);
                prop_assert_eq!(a.fingerprint, b.fingerprint);
                prop_assert_eq!(key_of(&rows), key_of(&rows));
            }
        }
    }
}
