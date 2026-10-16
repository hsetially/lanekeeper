//! What a draft says about the change it holds, as of the moment it is asked: how serious the change is (D76) and when each service
//! that reads the file would pick it up (D85).
//!
//! Both are facts, and the engine decides them (rule 7): `engine::severity` over the default rules and the admin's, and
//! `engine::pickup_state` for each Deployment that reads the file. This module supplies the inputs and puts the answers on the wire
//! (`domain::Draft::severity` and `domain::Draft::effects`). Neither is stored: the rules and the Deployments change after a draft
//! is made, and a draft that is read tomorrow should say what is true tomorrow. Neither writes or records anything.
//!
//! - **Severity.** The context is the change as the draft holds it: the file's path, and the kind and path of each setting it
//!   changes. A draft is a change that has not happened, so it has no drift state, and a denied file is never drafted, so the
//!   file's class decides nothing: both are left out of the context, and a rule that needs them does not hold. The answer is the
//!   highest severity of the rules that match, `None` when no rule does (the engine's "low" with no rule behind it is not a
//!   severity anyone chose). The admin's rules are the enabled `change` rules of `severity_rules` that are not defaults; their
//!   `matcher` is the API's `SeverityMatch` (finding kinds, change kinds, a path glob and a setting glob). A rule the hub cannot
//!   read, or a set the engine refuses, gives no answer at all, never a lower one than the rule might have said.
//! - **Effects.** The services are the ones that read the path (D82, the same rule as a notification's, `ops::pickup`). The change
//!   is judged as one made now that nobody has been told about and no pod has restarted for, so a resource file is
//!   `live_within_ttl`, a property source `needs_notify_or_restart`, and a new folder or `channels.yml`
//!   `needs_config_server_restart`. A service's client settings are its Deployment's allowlisted environment values (D88).

use domain::{
    ChangeKind, FindingKind, NfsPath, PickupState, ServiceRef, SettingChange, SettingPath, Severity,
    ShortText, SwimlaneId, Timestamp, User,
};
use engine::{
    ChangeContext, PickupChange, PickupInput, SeverityMatch, SeverityRule, SeverityRules, pickup_state,
    severity,
};
use serde::Deserialize;
use sqlx::Row;

use crate::error::{Internal, Res};
use crate::ops::pickup::{change_of, client_of, deployment_names, readers, undeclared_name};
use crate::preview::effect::{facts, folder_facts};
use crate::service::Inner;

/// The most services one draft lists: the API's bound for `Draft.effects`.
pub(crate) const MAX_EFFECTS: i64 = 1_000;

/// The admin's enabled rules for changes. The defaults are the engine's, so a stored default is not read twice (the engine refuses
/// a duplicate id).
pub(crate) const ADMIN_RULES: &str = "SELECT id, matcher, severity FROM severity_rules \
     WHERE scope = 'change' AND enabled AND NOT is_default ORDER BY id LIMIT $1";

/// The Deployments of the swimlane that read the path: `$2` is true when every service does, otherwise `$3` names the applications.
pub(crate) const SERVICES: &str = "SELECT namespace, name, env_values FROM services \
     WHERE swimlane_id = $1 AND removed_at IS NULL \
       AND ($2 OR name = ANY($3) OR env_values ->> 'SPRING_APPLICATION_NAME' = ANY($3)) \
     ORDER BY namespace, name LIMIT $4";

/// The conditions of an admin rule, as the API's `SeverityMatch` stores them. Anything else in the column is a rule the hub cannot
/// read.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct Matcher {
    finding_kinds: Vec<FindingKind>,
    change_kinds: Vec<ChangeKind>,
    path_glob: Option<String>,
    setting_glob: Option<String>,
}

/// The admin's rules, or `None` when one of them cannot be read.
async fn admin_rules(hub: &Inner) -> Res<Option<Vec<SeverityRule>>> {
    // One more than the engine takes, so that a table over the limit is refused by the engine and not cut short here.
    let limit = i64::try_from(SeverityRules::MAX_RULES)
        .unwrap_or(1_000)
        .saturating_add(1);
    let rows = hub
        .within(
            "draft_rules",
            sqlx::query(ADMIN_RULES).bind(limit).fetch_all(&hub.pool),
        )
        .await?
        .map_err(|e| Internal::db("draft_rules", &e))?;
    let mut rules = Vec::with_capacity(rows.len());
    for row in rows {
        let get = |e: sqlx::Error| Internal::db("draft_rules", &e);
        let id: String = row.try_get("id").map_err(get)?;
        let matcher: serde_json::Value = row.try_get("matcher").map_err(get)?;
        let level: String = row.try_get("severity").map_err(get)?;
        let (Ok(id), Ok(m), Ok(severity)) = (
            ShortText::parse(&id),
            serde_json::from_value::<Matcher>(matcher),
            level.parse::<Severity>(),
        ) else {
            return Ok(None);
        };
        rules.push(SeverityRule {
            id,
            severity,
            enabled: true,
            matcher: SeverityMatch {
                finding_kinds: m.finding_kinds,
                change_kinds: m.change_kinds,
                path_glob: m.path_glob,
                setting_glob: m.setting_glob,
                ..SeverityMatch::default()
            },
        });
    }
    Ok(Some(rules))
}

/// The severity of changing `path` the way `changes` say (D76). `None`: no rule matched, or the rules could not be applied.
pub(crate) async fn severity_of(
    hub: &Inner,
    path: &NfsPath,
    changes: &[SettingChange],
) -> Res<Option<Severity>> {
    let Some(admin) = admin_rules(hub).await? else {
        return Ok(None);
    };
    let path = path.clone();
    let kinds: Vec<ChangeKind> = changes.iter().map(|c| c.kind).collect();
    let settings: Vec<SettingPath> = changes.iter().map(|c| c.path.clone()).collect();
    hub.blocking("draft_severity", move || {
        let rules = SeverityRules::with_defaults(admin).ok()?;
        let answer = severity(
            &ChangeContext {
                path: Some(&path),
                change_kinds: &kinds,
                setting_paths: &settings,
                ..ChangeContext::default()
            },
            &rules,
        );
        (!answer.rule_ids.is_empty()).then_some(answer.severity)
    })
    .await
}

/// When a change to `path` would take effect for each service that reads it (D85), ordered by namespace and name.
pub(crate) async fn effects_of(
    hub: &Inner,
    user: &User,
    swimlane: &SwimlaneId,
    path: &NfsPath,
) -> Res<Vec<(ServiceRef, PickupState)>> {
    let swimlane_facts = facts(hub, swimlane).await?;
    // A registry that does not answer is taken to have the folder: the least alarming guess is also the one a notification makes.
    let (folder_exists, _) = folder_facts(hub, user, swimlane, path).await;
    let deployments = deployment_names(&hub.pool, swimlane, std::slice::from_ref(path)).await?;
    let who = readers(&swimlane_facts, std::slice::from_ref(path), &deployments);
    let apps: Vec<String> = who.apps.into_iter().collect();
    let rows = hub
        .within(
            "draft_services",
            sqlx::query(SERVICES)
                .bind(swimlane.as_str())
                .bind(who.everyone)
                .bind(apps)
                .bind(MAX_EFFECTS)
                .fetch_all(&hub.pool),
        )
        .await?
        .map_err(|e| Internal::db("draft_services", &e))?;

    let now = Timestamp::from_unix_millis(hub.now_ms());
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let get = |e: sqlx::Error| Internal::db("draft_services", &e);
        let namespace: String = row.try_get("namespace").map_err(get)?;
        let name: String = row.try_get("name").map_err(get)?;
        let env: serde_json::Value = row.try_get("env_values").map_err(get)?;
        let Ok(service) = ServiceRef::new(&namespace, &name) else {
            continue;
        };
        let change = if folder_exists {
            change_of(path, &swimlane_facts, undeclared_name(&name, &env))
        } else {
            PickupChange::NewFolder
        };
        // A change nobody has been told about, made now: no notification yet, and no pod started after it. The config-server's own
        // start is long before it.
        let picked = pickup_state(&PickupInput {
            change,
            client: client_of(&env),
            changed_at: now,
            now,
            notified_at: None,
            pod_starts: &[],
            config_server_started: Timestamp::from_unix_millis(0),
        });
        out.push((service, picked.state));
    }
    Ok(out)
}

/// The statements, for the `EXPLAIN` test (P12).
pub(crate) fn queries() -> Vec<(&'static str, &'static str)> {
    vec![("draft_rules", ADMIN_RULES), ("draft_services", SERVICES)]
}
