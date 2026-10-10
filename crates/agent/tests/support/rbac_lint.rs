//! A lint for the agent's RBAC manifest (`RBAC.md`, S17, acceptance 4).
//!
//! The manifest is in fenced YAML blocks in the Markdown. The lint reads the Roles and `ClusterRole`s in them and checks
//! every rule against an allow-list of exactly what the agent does; anything else is a violation. The Secret rules get
//! their own, plainer messages, because "no Secret list" is the rule that matters most.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Rule {
    pub api_groups: Vec<String>,
    pub resources: Vec<String>,
    pub verbs: Vec<String>,
    pub resource_names: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Meta {
    pub name: String,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Role {
    pub kind: String,
    pub metadata: Meta,
    pub rules: Vec<Rule>,
}

/// The `(api group, resource, verbs)` the agent may be granted without a name. Everything else is refused.
const ALLOWED: &[(&str, &str, &[&str])] = &[
    ("apps", "deployments", &["get", "list", "watch", "patch"]),
    ("", "pods", &["get", "list", "watch"]),
    ("batch", "jobs", &["get", "list", "watch"]),
];

/// The Secret verbs the agent may be granted, and only on named Secrets.
const SECRET_VERBS: &[&str] = &["get", "update"];

/// Every `Role` and `ClusterRole` in the fenced `yaml` blocks of a Markdown file.
pub fn roles_from_markdown(markdown: &str) -> Vec<Role> {
    let mut roles = Vec::new();
    let mut block: Option<String> = None;
    for line in markdown.lines() {
        match (&mut block, line.trim_end()) {
            (None, "```yaml") => block = Some(String::new()),
            (Some(text), "```") => {
                roles.extend(roles_from_yaml(text));
                block = None;
            }
            (Some(text), l) => {
                text.push_str(l);
                text.push('\n');
            }
            (None, _) => {}
        }
    }
    roles
}

/// The `Role`s and `ClusterRole`s in a multi-document YAML text. Other kinds (bindings) are skipped.
pub fn roles_from_yaml(text: &str) -> Vec<Role> {
    let mut documents = vec![String::new()];
    for line in text.lines() {
        if line.trim_end() == "---" {
            documents.push(String::new());
        } else {
            let current = documents.last_mut().unwrap();
            current.push_str(line);
            current.push('\n');
        }
    }
    documents
        .iter()
        .filter(|d| !d.trim().is_empty())
        .map(|d| {
            serde_saphyr::from_str::<Role>(d).unwrap_or_else(|e| panic!("YAML does not parse: {e}\n{d}"))
        })
        .filter(|r| r.kind == "Role" || r.kind == "ClusterRole")
        .collect()
}

fn has(list: &[String], item: &str) -> bool {
    list.iter().any(|x| x == item)
}

/// Why a manifest grants more than the agent needs. Empty when it does not.
pub fn lint(roles: &[Role]) -> Vec<String> {
    let mut found = Vec::new();
    for role in roles {
        let who = format!("{} {}", role.kind, role.metadata.name);
        for rule in &role.rules {
            let wildcard = rule
                .api_groups
                .iter()
                .chain(&rule.resources)
                .chain(&rule.verbs)
                .any(|x| x.contains('*'));
            if wildcard {
                found.push(format!("{who}: a wildcard in {rule:?}"));
                continue;
            }
            if has(&rule.resources, "secrets") {
                if role.kind == "ClusterRole" {
                    found.push(format!(
                        "{who}: a ClusterRole grants access to Secrets in every namespace"
                    ));
                }
                if rule.resource_names.is_empty() {
                    found.push(format!(
                        "{who}: Secret access without resourceNames ({:?})",
                        rule.verbs
                    ));
                }
                for verb in &rule.verbs {
                    if !SECRET_VERBS.contains(&verb.as_str()) {
                        found.push(format!("{who}: the verb {verb} on Secrets"));
                    }
                }
                if rule.resources.len() > 1 {
                    found.push(format!(
                        "{who}: Secrets share a rule with other resources: {:?}",
                        rule.resources
                    ));
                }
                continue;
            }
            for group in &rule.api_groups {
                for resource in &rule.resources {
                    let Some((_, _, verbs)) = ALLOWED.iter().find(|(g, r, _)| g == group && r == resource)
                    else {
                        found.push(format!(
                            "{who}: {resource} in {group:?} is not something the agent uses"
                        ));
                        continue;
                    };
                    for verb in &rule.verbs {
                        if !verbs.contains(&verb.as_str()) {
                            found.push(format!(
                                "{who}: the verb {verb} on {resource} is not something the agent uses"
                            ));
                        }
                    }
                    if !rule.resource_names.is_empty() {
                        found.push(format!(
                            "{who}: resourceNames on {resource} would break list and watch"
                        ));
                    }
                }
            }
        }
    }
    found
}

/// Would the manifest let a call with this group, resource and verb through? `name` is the object's name, if the call
/// names one. Secrets need the name to be in `resourceNames`.
pub fn permits(roles: &[Role], group: &str, resource: &str, verb: &str, name: Option<&str>) -> bool {
    roles.iter().flat_map(|r| &r.rules).any(|rule| {
        has(&rule.api_groups, group)
            && has(&rule.resources, resource)
            && has(&rule.verbs, verb)
            && (rule.resource_names.is_empty() || name.is_some_and(|n| has(&rule.resource_names, n)))
    })
}
