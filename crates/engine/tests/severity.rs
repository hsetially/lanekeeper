//! T9 (D76): severity of a change or a finding, from the default rules in `docs/domain-model.md` and the admin's own. The result
//! is the highest severity of the rules that match, with the ids of all of them, sorted. One test per default rule, and the
//! property that makes the rule list safe to extend: adding a rule never lowers a severity.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{ChangeKind, DriftState, FileClass, FindingKind, NfsPath, SettingPath, Severity, ShortText};
use engine::{
    ChangeContext, SeverityError, SeverityMatch, SeverityOutcome, SeverityRule, SeverityRules, default_rules,
    severity,
};
use proptest::prelude::*;

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

fn setting(s: &str) -> SettingPath {
    SettingPath::parse(s).unwrap()
}

fn id(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

fn defaults() -> SeverityRules {
    SeverityRules::with_defaults(Vec::new()).unwrap()
}

fn outcome(context: &ChangeContext<'_>) -> (Severity, Vec<String>) {
    let SeverityOutcome { severity, rule_ids } = severity(context, &defaults());
    (severity, rule_ids.iter().map(|i| i.as_str().to_owned()).collect())
}

fn finding(kind: FindingKind, path: &NfsPath) -> ChangeContext<'_> {
    ChangeContext {
        finding_kind: Some(kind),
        path: Some(path),
        ..ChangeContext::default()
    }
}

#[test]
fn default_rule_security_path_critical() {
    let p = nfs("security/security-roles.yml");
    let ctx = ChangeContext {
        change_kinds: &[ChangeKind::Changed],
        path: Some(&p),
        ..ChangeContext::default()
    };
    assert_eq!(
        outcome(&ctx),
        (Severity::Critical, vec!["default.security_path".to_owned()])
    );

    // Deeper than one level, and in a finding as well as in a change.
    let deep = nfs("security/keys/a/b.yml");
    assert_eq!(outcome(&finding(FindingKind::Drift, &deep)).0, Severity::Critical);

    // `security` anywhere but at the root is not the security folder, and neither is a name that merely starts with it.
    for other in ["app/security/x.yml", "security-extra/x.yml", "securityfile.yml"] {
        let p = nfs(other);
        let ctx = ChangeContext {
            change_kinds: &[ChangeKind::Changed],
            path: Some(&p),
            ..ChangeContext::default()
        };
        assert!(
            !outcome(&ctx).1.contains(&"default.security_path".to_owned()),
            "{other}"
        );
    }
}

#[test]
fn c7_critical() {
    let p = nfs("svc/a.yml");
    let ctx = finding(FindingKind::OverwrittenNfsChange, &p);
    assert_eq!(
        outcome(&ctx),
        (
            Severity::Critical,
            vec!["default.overwritten_nfs_change".to_owned()]
        )
    );
}

#[test]
fn denied_file_critical() {
    let p = nfs("svc/tls/server.pem");
    let ctx = ChangeContext {
        finding_kind: Some(FindingKind::Drift),
        path: Some(&p),
        drift_state: Some(DriftState::GitAhead),
        file_class: Some(FileClass::Denied),
        ..ChangeContext::default()
    };
    assert_eq!(
        outcome(&ctx),
        (Severity::Critical, vec!["default.denied_file".to_owned()])
    );
}

#[test]
fn root_file_high() {
    let p = nfs("application.yml");
    let ctx = ChangeContext {
        change_kinds: &[ChangeKind::Changed],
        path: Some(&p),
        ..ChangeContext::default()
    };
    assert_eq!(
        outcome(&ctx),
        (Severity::High, vec!["default.root_file_change".to_owned()])
    );

    // A root file whose drift state is a change is a root file changing too; one in sync is not.
    for (state, high) in [
        (DriftState::GitAhead, true),
        (DriftState::NfsAhead, true),
        (DriftState::Conflict, true),
        (DriftState::InSync, false),
        (DriftState::IntentionalDivergence, false),
    ] {
        let ctx = ChangeContext {
            finding_kind: Some(FindingKind::Drift),
            path: Some(&p),
            drift_state: Some(state),
            ..ChangeContext::default()
        };
        let (got, ids) = outcome(&ctx);
        assert_eq!(
            ids.contains(&"default.root_file_drift".to_owned()),
            high,
            "{state}"
        );
        assert_eq!(
            got == Severity::High,
            high || state == DriftState::Conflict,
            "{state}"
        );
    }

    // A finding that is not about a change does not make a root file high.
    let ctx = finding(FindingKind::RedundantTenantFile, &p);
    assert_eq!(outcome(&ctx).0, Severity::Low);

    // A file in a folder is not a root file.
    let p = nfs("svc/application.yml");
    let ctx = ChangeContext {
        change_kinds: &[ChangeKind::Changed],
        path: Some(&p),
        ..ChangeContext::default()
    };
    assert_eq!(outcome(&ctx), (Severity::Low, Vec::new()));

    // No path, no root file.
    assert_eq!(outcome(&ChangeContext::default()), (Severity::Low, Vec::new()));
}

#[test]
fn removed_keys_high() {
    let p = nfs("svc/a.yml");
    let ctx = ChangeContext {
        change_kinds: &[ChangeKind::Added, ChangeKind::Removed],
        path: Some(&p),
        ..ChangeContext::default()
    };
    assert_eq!(
        outcome(&ctx),
        (Severity::High, vec!["default.removed_keys".to_owned()])
    );
    // Added and changed keys alone are not enough.
    let ctx = ChangeContext {
        change_kinds: &[ChangeKind::Added, ChangeKind::Changed],
        path: Some(&p),
        ..ChangeContext::default()
    };
    assert_eq!(outcome(&ctx), (Severity::Low, Vec::new()));
}

#[test]
fn type_change_high() {
    let p = nfs("svc/a.yml");
    let ctx = ChangeContext {
        change_kinds: &[ChangeKind::TypeChanged],
        path: Some(&p),
        ..ChangeContext::default()
    };
    assert_eq!(
        outcome(&ctx),
        (Severity::High, vec!["default.type_change".to_owned()])
    );
}

#[test]
fn conflict_high() {
    let p = nfs("svc/a.yml");
    let ctx = ChangeContext {
        finding_kind: Some(FindingKind::Drift),
        path: Some(&p),
        drift_state: Some(DriftState::Conflict),
        ..ChangeContext::default()
    };
    assert_eq!(
        outcome(&ctx),
        (Severity::High, vec!["default.conflict".to_owned()])
    );
}

#[test]
fn c6_high() {
    let p = nfs("svc/a.yml");
    let ctx = finding(FindingKind::EnvironmentHostMismatch, &p);
    assert_eq!(
        outcome(&ctx),
        (
            Severity::High,
            vec!["default.environment_host_mismatch".to_owned()]
        )
    );
}

#[test]
fn other_oob_medium() {
    let p = nfs("svc/a.yml");
    for state in [DriftState::NfsAhead, DriftState::Unknown, DriftState::Untracked] {
        let ctx = ChangeContext {
            finding_kind: Some(FindingKind::Drift),
            path: Some(&p),
            drift_state: Some(state),
            ..ChangeContext::default()
        };
        assert_eq!(
            outcome(&ctx),
            (Severity::Medium, vec!["default.out_of_band".to_owned()]),
            "{state}"
        );
    }
    // Git ahead, in sync and a marked divergence are not out-of-band changes.
    for state in [
        DriftState::GitAhead,
        DriftState::InSync,
        DriftState::IntentionalDivergence,
    ] {
        let ctx = ChangeContext {
            finding_kind: Some(FindingKind::Drift),
            path: Some(&p),
            drift_state: Some(state),
            ..ChangeContext::default()
        };
        assert_eq!(outcome(&ctx), (Severity::Low, Vec::new()), "{state}");
    }
}

#[test]
fn c1_medium() {
    let p = nfs("svc/a-sit1.yml");
    let ctx = finding(FindingKind::MissedBaseChanges, &p);
    assert_eq!(
        outcome(&ctx),
        (Severity::Medium, vec!["default.missed_base_changes".to_owned()])
    );
}

#[test]
fn c2_c3_c8_low() {
    let p = nfs("svc/a.yml");
    for kind in [
        FindingKind::RedundantTenantFile,
        FindingKind::UnevenTenantFiles,
        FindingKind::DuplicateKeys,
    ] {
        assert_eq!(
            outcome(&finding(kind, &p)),
            (Severity::Low, vec!["default.tenant_file_hygiene".to_owned()]),
            "{kind}"
        );
    }
}

#[test]
fn the_highest_rule_wins_and_every_matching_rule_is_listed_sorted() {
    // A conflict in a root file that is denied: critical, with all three rules.
    let p = nfs("application.yml");
    let ctx = ChangeContext {
        finding_kind: Some(FindingKind::Drift),
        path: Some(&p),
        drift_state: Some(DriftState::Conflict),
        file_class: Some(FileClass::Denied),
        ..ChangeContext::default()
    };
    assert_eq!(
        outcome(&ctx),
        (
            Severity::Critical,
            vec![
                "default.conflict".to_owned(),
                "default.denied_file".to_owned(),
                "default.root_file_drift".to_owned()
            ]
        )
    );
}

#[test]
fn default_rules_have_unique_ids_and_compile() {
    let rules = default_rules();
    assert_eq!(rules.len(), 12);
    let mut ids: Vec<_> = rules.iter().map(|r| r.id.as_str().to_owned()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), rules.len());
    assert!(ids.iter().all(|i| i.starts_with("default.")));
    SeverityRules::new(rules).unwrap();
}

fn admin(id_text: &str, severity: Severity, matcher: SeverityMatch) -> SeverityRule {
    SeverityRule {
        id: id(id_text),
        severity,
        enabled: true,
        matcher,
    }
}

#[test]
fn admin_rules_add_to_the_defaults() {
    let rules = SeverityRules::with_defaults(vec![admin(
        "admin.payments",
        Severity::Critical,
        SeverityMatch {
            path_glob: Some("payments/**".to_owned()),
            finding_kinds: vec![FindingKind::Drift],
            ..SeverityMatch::default()
        },
    )])
    .unwrap();
    let p = nfs("payments/limits.yml");
    let ctx = ChangeContext {
        finding_kind: Some(FindingKind::Drift),
        path: Some(&p),
        drift_state: Some(DriftState::NfsAhead),
        ..ChangeContext::default()
    };
    let got = severity(&ctx, &rules);
    assert_eq!(got.severity, Severity::Critical);
    let ids: Vec<_> = got.rule_ids.iter().map(ShortText::as_str).collect();
    assert_eq!(ids, ["admin.payments", "default.out_of_band"]);

    // The same rule does not match another kind of finding, or another folder.
    let other = ChangeContext {
        finding_kind: Some(FindingKind::OrphanFile),
        ..ctx.clone()
    };
    assert_eq!(severity(&other, &rules).severity, Severity::Medium);
    let q = nfs("billing/limits.yml");
    let elsewhere = ChangeContext {
        path: Some(&q),
        ..ctx
    };
    assert_eq!(severity(&elsewhere, &rules).severity, Severity::Medium);
}

#[test]
fn a_rule_needs_all_its_conditions() {
    let rules = SeverityRules::new(vec![admin(
        "r",
        Severity::High,
        SeverityMatch {
            finding_kinds: vec![FindingKind::Drift],
            drift_states: vec![DriftState::NfsAhead],
            file_classes: vec![FileClass::Structured],
            change_kinds: vec![ChangeKind::Changed],
            path_glob: Some("svc/*.yml".to_owned()),
            setting_glob: Some("db.*".to_owned()),
            root_file: Some(false),
        },
    )])
    .unwrap();
    let p = nfs("svc/a.yml");
    let paths = [setting("db.url"), setting("other")];
    let full = ChangeContext {
        finding_kind: Some(FindingKind::Drift),
        change_kinds: &[ChangeKind::Changed],
        path: Some(&p),
        setting_paths: &paths,
        drift_state: Some(DriftState::NfsAhead),
        file_class: Some(FileClass::Structured),
    };
    assert_eq!(severity(&full, &rules).severity, Severity::High);

    let q = nfs("svc/a.json");
    let none: [SettingPath; 0] = [];
    let misses = [
        ChangeContext {
            finding_kind: Some(FindingKind::OrphanFile),
            ..full.clone()
        },
        ChangeContext {
            finding_kind: None,
            ..full.clone()
        },
        ChangeContext {
            change_kinds: &[ChangeKind::Added],
            ..full.clone()
        },
        ChangeContext {
            change_kinds: &[],
            ..full.clone()
        },
        ChangeContext {
            path: Some(&q),
            ..full.clone()
        },
        ChangeContext {
            path: None,
            ..full.clone()
        },
        ChangeContext {
            setting_paths: &none,
            ..full.clone()
        },
        ChangeContext {
            drift_state: Some(DriftState::GitAhead),
            ..full.clone()
        },
        ChangeContext {
            drift_state: None,
            ..full.clone()
        },
        ChangeContext {
            file_class: Some(FileClass::Text),
            ..full.clone()
        },
    ];
    for (n, ctx) in misses.iter().enumerate() {
        assert_eq!(severity(ctx, &rules), SeverityOutcome::none(), "case {n}");
    }
}

#[test]
fn a_setting_glob_matches_when_any_setting_of_the_change_matches() {
    let rules = SeverityRules::new(vec![admin(
        "r",
        Severity::High,
        SeverityMatch {
            setting_glob: Some("spring.datasource.*".to_owned()),
            ..SeverityMatch::default()
        },
    )])
    .unwrap();
    let hit = [setting("a.b"), setting("spring.datasource.url.x")];
    let miss = [setting("a.b"), setting("spring.profiles")];
    let of = |s: &[SettingPath]| {
        severity(
            &ChangeContext {
                setting_paths: s,
                ..ChangeContext::default()
            },
            &rules,
        )
    };
    assert_eq!(of(&hit).severity, Severity::High);
    assert_eq!(of(&miss), SeverityOutcome::none());
}

#[test]
fn a_rule_with_no_condition_matches_everything_and_a_disabled_rule_nothing() {
    let on = admin("on", Severity::Medium, SeverityMatch::default());
    let mut off = admin("off", Severity::Critical, SeverityMatch::default());
    off.enabled = false;
    let rules = SeverityRules::new(vec![on, off]).unwrap();
    let got = severity(&ChangeContext::default(), &rules);
    assert_eq!(got.severity, Severity::Medium);
    assert_eq!(got.rule_ids, [id("on")]);
}

#[test]
fn no_rule_means_low_and_no_ids() {
    let rules = SeverityRules::new(Vec::new()).unwrap();
    assert_eq!(
        severity(&ChangeContext::default(), &rules),
        SeverityOutcome::none()
    );
    assert_eq!(SeverityOutcome::none().severity, Severity::Low);
}

#[test]
fn glob_matching_is_case_sensitive_and_star_crosses_slashes() {
    let rules = SeverityRules::new(vec![admin(
        "r",
        Severity::High,
        SeverityMatch {
            path_glob: Some("svc/*.yml".to_owned()),
            ..SeverityMatch::default()
        },
    )])
    .unwrap();
    let at = |p: &str| {
        let p = nfs(p);
        severity(
            &ChangeContext {
                path: Some(&p),
                ..ChangeContext::default()
            },
            &rules,
        )
        .severity
    };
    assert_eq!(at("svc/a.yml"), Severity::High);
    assert_eq!(
        at("svc/deep/a.yml"),
        Severity::High,
        "`*` crosses `/`, as in every glob of the engine"
    );
    assert_eq!(
        at("SVC/a.yml"),
        Severity::Low,
        "file names on NFS are case-sensitive"
    );
}

#[test]
fn bad_rule_lists_are_refused_and_the_error_never_echoes_the_rule() {
    const MARKER: &str = "SECRET-MARKER-9d41";
    let with_glob = |g: String| {
        SeverityRules::new(vec![admin(
            "r",
            Severity::Low,
            SeverityMatch {
                path_glob: Some(g),
                ..SeverityMatch::default()
            },
        )])
    };
    let invalid = with_glob(format!("[{MARKER}")).unwrap_err();
    assert_eq!(invalid, SeverityError::InvalidGlob);
    let too_long = with_glob("a".repeat(SeverityRules::MAX_GLOB_BYTES + 1)).unwrap_err();
    assert_eq!(too_long, SeverityError::GlobTooLong);
    assert!(
        with_glob("a".repeat(SeverityRules::MAX_GLOB_BYTES)).is_ok(),
        "exactly at the limit passes"
    );

    let setting_invalid = SeverityRules::new(vec![admin(
        "r",
        Severity::Low,
        SeverityMatch {
            setting_glob: Some(format!("{{{MARKER}")),
            ..SeverityMatch::default()
        },
    )])
    .unwrap_err();
    assert_eq!(setting_invalid, SeverityError::InvalidGlob);

    let many: Vec<_> = (0..=SeverityRules::MAX_RULES)
        .map(|n| admin(&format!("r{n}"), Severity::Low, SeverityMatch::default()))
        .collect();
    assert_eq!(SeverityRules::new(many).unwrap_err(), SeverityError::TooManyRules);
    let at_limit: Vec<_> = (0..SeverityRules::MAX_RULES)
        .map(|n| admin(&format!("r{n}"), Severity::Low, SeverityMatch::default()))
        .collect();
    assert!(SeverityRules::new(at_limit).is_ok());

    let dup = SeverityRules::new(vec![
        admin(MARKER, Severity::Low, SeverityMatch::default()),
        admin(MARKER, Severity::High, SeverityMatch::default()),
    ])
    .unwrap_err();
    assert_eq!(dup, SeverityError::DuplicateId);

    for err in [invalid, too_long, setting_invalid, dup] {
        assert!(!err.to_string().contains(MARKER), "{err}");
        assert!(!format!("{err:?}").contains(MARKER), "{err:?}");
    }
    // The compiled rules do not print the globs either.
    let rules = with_glob(format!("{MARKER}/**")).unwrap();
    assert!(!format!("{rules:?}").contains(MARKER));
}

// ---- properties ----

fn pick<T: Copy + std::fmt::Debug + 'static>(all: &'static [T]) -> impl Strategy<Value = T> {
    proptest::sample::select(all.to_vec())
}

fn arb_matcher() -> impl Strategy<Value = SeverityMatch> {
    (
        proptest::collection::vec(pick(&FindingKind::ALL), 0..3),
        proptest::collection::vec(
            pick(&[
                ChangeKind::Added,
                ChangeKind::Removed,
                ChangeKind::Changed,
                ChangeKind::TypeChanged,
            ]),
            0..3,
        ),
        proptest::option::of(proptest::sample::select(vec![
            "security/**",
            "svc/*.yml",
            "*.yml",
            "application.*",
            "**",
        ])),
        proptest::option::of(proptest::sample::select(vec!["db.*", "a.b", "*"])),
        proptest::collection::vec(pick(&DriftState::ALL), 0..3),
        proptest::collection::vec(pick(&FileClass::ALL), 0..3),
        proptest::option::of(any::<bool>()),
    )
        .prop_map(
            |(
                finding_kinds,
                change_kinds,
                path_glob,
                setting_glob,
                drift_states,
                file_classes,
                root_file,
            )| {
                SeverityMatch {
                    finding_kinds,
                    change_kinds,
                    path_glob: path_glob.map(str::to_owned),
                    setting_glob: setting_glob.map(str::to_owned),
                    drift_states,
                    file_classes,
                    root_file,
                }
            },
        )
}

fn arb_rules(max: usize) -> impl Strategy<Value = Vec<SeverityRule>> {
    proptest::collection::vec((arb_matcher(), pick(&Severity::ALL), any::<bool>()), 0..max).prop_map(|v| {
        v.into_iter()
            .enumerate()
            .map(|(n, (matcher, severity, enabled))| SeverityRule {
                id: id(&format!("admin.{n:03}")),
                severity,
                enabled,
                matcher,
            })
            .collect()
    })
}

#[derive(Debug, Clone)]
struct OwnedContext {
    finding_kind: Option<FindingKind>,
    change_kinds: Vec<ChangeKind>,
    path: Option<NfsPath>,
    setting_paths: Vec<SettingPath>,
    drift_state: Option<DriftState>,
    file_class: Option<FileClass>,
}

impl OwnedContext {
    fn borrowed(&self) -> ChangeContext<'_> {
        ChangeContext {
            finding_kind: self.finding_kind,
            change_kinds: &self.change_kinds,
            path: self.path.as_ref(),
            setting_paths: &self.setting_paths,
            drift_state: self.drift_state,
            file_class: self.file_class,
        }
    }
}

fn arb_context() -> impl Strategy<Value = OwnedContext> {
    (
        proptest::option::of(pick(&FindingKind::ALL)),
        proptest::collection::vec(
            pick(&[
                ChangeKind::Added,
                ChangeKind::Removed,
                ChangeKind::Changed,
                ChangeKind::TypeChanged,
            ]),
            0..3,
        ),
        proptest::option::of(proptest::sample::select(vec![
            "security/a.yml",
            "svc/a.yml",
            "svc/deep/a.yml",
            "application.yml",
            "application-sit1.yml",
            "x.bin",
        ])),
        proptest::collection::vec(proptest::sample::select(vec!["db.url", "a.b", "other"]), 0..3),
        proptest::option::of(pick(&DriftState::ALL)),
        proptest::option::of(pick(&FileClass::ALL)),
    )
        .prop_map(
            |(finding_kind, change_kinds, path, settings, drift_state, file_class)| OwnedContext {
                finding_kind,
                change_kinds,
                path: path.map(nfs),
                setting_paths: settings.into_iter().map(setting).collect(),
                drift_state,
                file_class,
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Adding a rule never lowers a severity, and never removes a rule id from the result.
    #[test]
    fn adding_a_rule_never_lowers_severity_prop(
        base in arb_rules(6),
        extra in arb_rules(4),
        context in arb_context(),
    ) {
        let extra: Vec<_> = extra
            .into_iter()
            .enumerate()
            .map(|(n, mut r)| { r.id = id(&format!("extra.{n:03}")); r })
            .collect();
        let before = SeverityRules::with_defaults(base.clone()).unwrap();
        let mut all = base;
        all.extend(extra);
        let after = SeverityRules::with_defaults(all).unwrap();
        let ctx = context.borrowed();
        let (b, a) = (severity(&ctx, &before), severity(&ctx, &after));
        prop_assert!(a.severity >= b.severity, "{:?} -> {:?}", b, a);
        for rule in &b.rule_ids {
            prop_assert!(a.rule_ids.contains(rule));
        }
    }

    /// The result does not depend on the order the rules were given in, and its ids are sorted and unique.
    #[test]
    fn rule_ids_sorted_and_deterministic(rules in arb_rules(8), context in arb_context(), shift in 0_usize..8) {
        let mut rotated = rules.clone();
        if !rotated.is_empty() {
            let by = shift % rotated.len();
            rotated.rotate_left(by);
        }
        rotated.reverse();
        let ctx = context.borrowed();
        let a = severity(&ctx, &SeverityRules::with_defaults(rules.clone()).unwrap());
        let b = severity(&ctx, &SeverityRules::with_defaults(rotated).unwrap());
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(&a, &severity(&ctx, &SeverityRules::with_defaults(rules).unwrap()));
        let ids: Vec<_> = a.rule_ids.iter().map(ShortText::as_str).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        prop_assert_eq!(ids, sorted);
    }

    /// The severity is the highest of the matching enabled rules: with no match it is low, and with a match it is not.
    #[test]
    fn severity_is_the_maximum_of_the_matching_rules_prop(rules in arb_rules(8), context in arb_context()) {
        let compiled = SeverityRules::new(rules.clone()).unwrap();
        let got = severity(&context.borrowed(), &compiled);
        // Each rule alone, to see which match.
        let mut best = Severity::Low;
        let mut ids = Vec::new();
        for rule in rules.iter().filter(|r| r.enabled) {
            let alone = SeverityRules::new(vec![SeverityRule { id: rule.id.clone(), ..rule.clone() }]).unwrap();
            let one = severity(&context.borrowed(), &alone);
            if !one.rule_ids.is_empty() {
                best = best.max(rule.severity);
                ids.push(rule.id.clone());
            }
        }
        ids.sort();
        prop_assert_eq!(got.severity, best);
        prop_assert_eq!(got.rule_ids, ids);
    }
}
