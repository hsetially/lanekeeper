# RBAC policy (09/T4; S7, S17). Run with `conftest test --combine --namespace rbac` over a chart's rendered output. What the agent's
# Roles may grant is the allow-list below and nothing else: workloads in the namespaces it watches (get, list, watch, and patch on
# Deployments for the restart annotation), and two Secrets of its own, by name, with get and update. No Secret list or watch, no
# unnamed Secret grant, no ClusterRole, no wildcard.
#
# This is the second barrier. contract.rego shows that the rendered Roles equal crates/agent/RBAC.md; this policy does not read that
# file, so a Role that is wrong in the file and in the chart together is still refused. It applies to every Role in the output, not
# only the agent's.
package rbac

import rego.v1

import data.lk

secret_names := {"lanekeeper-agent-cert", "lanekeeper-agent-join-token"}

# The only (apiGroup, resource, verb) triples that the agent's workload Roles may grant (RBAC.md, "What the agent does").
allowed := {
	["apps", "deployments", "get"], ["apps", "deployments", "list"], ["apps", "deployments", "watch"], ["apps", "deployments", "patch"],
	["", "pods", "get"], ["", "pods", "list"], ["", "pods", "watch"],
	["batch", "jobs", "get"], ["batch", "jobs", "list"], ["batch", "jobs", "watch"],
}

# The verbs on Secrets: get to read, update to write the renewed certificate. `create` cannot be limited by name, so the chart
# creates the certificate Secret and the agent never does.
allowed_secret_verbs := {"get", "update"}

roles := lk.of_kind("Role")

role_rules(role) := object.get(role, "rules", [])

grants(rule) := {[g, r, v] |
	some g in object.get(rule, "apiGroups", [])
	some r in object.get(rule, "resources", [])
	some v in object.get(rule, "verbs", [])
}

touches_secrets(rule) if "secrets" in object.get(rule, "resources", [])

# --- no ClusterRole -----------------------------------------------------------------------------------------------------------

deny contains msg if {
	some d in lk.docs
	d.kind in {"ClusterRole", "ClusterRoleBinding"}
	msg := sprintf("%s: the agent has Roles in its namespaces and no %s; a cluster-wide grant reaches every namespace and every Secret (S17)", [lk.who(d), d.kind])
}

# --- wildcards ----------------------------------------------------------------------------------------------------------------

wildcard_fields := ["apiGroups", "resources", "verbs", "resourceNames"]

deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	some field in wildcard_fields
	"*" in object.get(rule, field, [])
	msg := sprintf("%s: a wildcard in %s (S17)", [lk.who(role), field])
}

# --- non-resource URLs ----------------------------------------------------------------------------------------------------------

deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	"nonResourceURLs" in object.keys(rule)
	msg := sprintf("%s: nonResourceURLs %v; the agent reads no URL of the API server outside the resources (S17)", [lk.who(role), rule.nonResourceURLs])
}

# --- Secrets ------------------------------------------------------------------------------------------------------------------

# A Secret rule is for Secrets alone: next to another resource its resourceNames would not mean what they say.
deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	touches_secrets(rule)
	count(object.get(rule, "resources", [])) > 1
	msg := sprintf("%s: Secrets share a rule with other resources %v; a Secret rule names Secrets alone (S17)", [lk.who(role), rule.resources])
}

deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	touches_secrets(rule)
	count(object.get(rule, "resourceNames", [])) == 0
	msg := sprintf("%s: a grant on Secrets without resourceNames reaches every Secret of the namespace (S7, S17)", [lk.who(role)])
}

deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	touches_secrets(rule)
	some name in object.get(rule, "resourceNames", [])
	name != "*"
	not name in secret_names
	msg := sprintf("%s: a grant on the Secret %q; the agent's own are %v (S17)", [lk.who(role), name, sort(secret_names)])
}

# list and watch ignore resourceNames: whoever may list Secrets reads every one of them, however the rule is written.
deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	touches_secrets(rule)
	some verb in object.get(rule, "verbs", [])
	verb in {"list", "watch"}
	msg := sprintf("%s: the verb %s on Secrets; Kubernetes does not apply resourceNames to it, so it reads every Secret of the namespace (S7, S17)", [lk.who(role), verb])
}

deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	touches_secrets(rule)
	some verb in object.get(rule, "verbs", [])
	not verb in allowed_secret_verbs
	not verb in {"list", "watch", "*"}
	msg := sprintf("%s: the verb %s on Secrets; the agent may only get and update its own (S17)", [lk.who(role), verb])
}

# The Secrets are in the agent's own namespace, never in a namespace it watches.
deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	touches_secrets(rule)
	some d in lk.agent_deployments
	role.metadata.namespace != d.metadata.namespace
	msg := sprintf("%s: a grant on Secrets in namespace %s; the Secrets are the agent's own, so the grant is in the agent's own namespace, %s (S17)", [lk.who(role), role.metadata.namespace, d.metadata.namespace])
}

# --- everything else is the allow-list ------------------------------------------------------------------------------------------

deny contains msg if {
	some role in roles
	some rule in role_rules(role)
	not touches_secrets(rule)
	some triple in grants(rule)
	not "*" in triple
	not triple in allowed
	msg := sprintf("%s: the verb %s on %s (group %q) is not one the agent needs; its Roles grant what RBAC.md lists and nothing else (S17)", [lk.who(role), triple[2], triple[1], triple[0]])
}

# --- who the Roles are bound to ------------------------------------------------------------------------------------------------

bindings := lk.of_kind("RoleBinding")

deny contains msg if {
	some b in bindings
	b.roleRef.kind != "Role"
	msg := sprintf("%s: it binds a %s; only the namespaced Roles of this chart may be bound (S17)", [lk.who(b), b.roleRef.kind])
}

deny contains msg if {
	some b in bindings
	b.roleRef.kind == "Role"
	not role_named(b.metadata.namespace, b.roleRef.name)
	msg := sprintf("%s: it binds the Role %q, a Role that this chart does not render in namespace %s (S17)", [lk.who(b), b.roleRef.name, b.metadata.namespace])
}

role_named(namespace, name) if {
	some role in roles
	role.metadata.namespace == namespace
	role.metadata.name == name
}

deny contains msg if {
	some b in bindings
	some s in object.get(b, "subjects", [])
	not agent_subject(s)
	msg := sprintf("%s: the subject %v is not the agent's service account; only %s may be bound (S17)", [lk.who(b), s, lk.agent_name])
}

agent_subject(s) if {
	s.kind == "ServiceAccount"
	s.name == lk.agent_name
}

deny contains msg if {
	some b in bindings
	some s in object.get(b, "subjects", [])
	agent_subject(s)
	some d in lk.agent_deployments
	object.get(s, "namespace", "") != d.metadata.namespace
	msg := sprintf("%s: the service account is in namespace %q, and the agent runs in the agent's namespace, %s (S17)", [lk.who(b), object.get(s, "namespace", ""), d.metadata.namespace])
}

deny contains msg if {
	some b in bindings
	count(object.get(b, "subjects", [])) == 0
	msg := sprintf("%s: it names no subject (S17)", [lk.who(b)])
}

# --- namespaces: the Roles are in the namespaces the agent is configured for, and in its own ----------------------------------

deny contains msg if {
	some d in lk.agent_deployments
	some x in array.concat(roles, bindings)
	ns := x.metadata.namespace
	ns != d.metadata.namespace
	not ns in lk.agent_namespaces
	msg := sprintf("%s: it is in namespace %s, which is neither the agent's own (%s) nor one of LK_NAMESPACES %v; the agent has no business there (S17)", [lk.who(x), ns, d.metadata.namespace, sort(lk.agent_namespaces)])
}
