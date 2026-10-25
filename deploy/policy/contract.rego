# Contract policy (09/T4; S16, S17). The agent chart's rendered output against the two contract files that the agent's owner wrote
# and its tests read: crates/agent/RBAC.md (the Roles) and crates/agent/HARDENING.md (the pod and the NetworkPolicy). Run with
#
#   conftest test --combine --namespace contract RENDERED agent-rbac-contract.yaml agent-hardening-contract.yaml
#
# where `deploy/tests/contract-extract.sh` wrote the two contract files from the fenced yaml blocks of the Markdown. They are not
# part of the other policies' input: their Roles and Pod would be taken for the chart's.
#
# What is compared, and what is not.
#   - Roles: the rules as sets (order is not part of the contract), the RoleBindings, and one workload Role per watched namespace.
#   - NetworkPolicy: the pod selector, the policy types, ingress and egress as sets of rules. The hub's and the API server's
#     addresses are values, so the contract's blocks stand for "an address block"; the metadata server's address is literal.
#   - Pod: the service account, the token, the host fields, seccomp, the container's security context, ports and probes.
#     The ids and the resources are values (HARDENING.md says its numbers are to be confirmed on the pilot).
#   - Not yet: environment, volumes and mounts. The spool arrives with T8; hardening.rego and deploy/tests/env-names.sh hold
#     the pod's paths and variable names until then.
#
# Each policy that guards a field on its own (hardening.rego, rbac.rego, network.rego) does not read these files, so the two
# barriers fail separately: a field that is wrong in the file and in the chart together is still refused there.
package contract

import rego.v1

import data.lk

is_contract_file(f) if endswith(f.path, "agent-rbac-contract.yaml")

is_contract_file(f) if endswith(f.path, "agent-hardening-contract.yaml")

contract_docs(suffix) := [d |
	some f in lk.files_ending(suffix)
	some d in lk.documents(f)
	is_object(d)
	is_string(d.kind)
]

rbac_docs := contract_docs("agent-rbac-contract.yaml")

hardening_docs := contract_docs("agent-hardening-contract.yaml")

rendered := [d |
	some f in input
	not is_contract_file(f)
	some d in lk.documents(f)
	is_object(d)
	is_string(d.kind)
]

of_kind(docs, kind) := [d | some d in docs; d.kind == kind]

named(docs, kind, name) := [d | some d in of_kind(docs, kind); d.metadata.name == name]

workloads_name := "lanekeeper-agent-workloads"

secrets_name := "lanekeeper-agent-secrets"

join_token_name := "lanekeeper-agent-join-token"

# --- the contract files are there, and say what they should -----------------------------------------------------------------------

deny contains msg if {
	count(rbac_docs) == 0
	msg := "the contract file for crates/agent/RBAC.md was not given (agent-rbac-contract.yaml); the Roles cannot be compared (S17)"
}

deny contains msg if {
	count(hardening_docs) == 0
	msg := "the contract file for crates/agent/HARDENING.md was not given (agent-hardening-contract.yaml); the pod and the policy cannot be compared (S17)"
}

deny contains msg if {
	count(rbac_docs) > 0
	some name in {workloads_name, secrets_name}
	count(named(rbac_docs, "Role", name)) != 1
	msg := sprintf("crates/agent/RBAC.md has no single Role %s; the contract cannot be compared (S17)", [name])
}

deny contains msg if {
	count(rbac_docs) > 0
	some name in {workloads_name, secrets_name}
	count(named(rbac_docs, "RoleBinding", name)) != 1
	msg := sprintf("crates/agent/RBAC.md has no single RoleBinding %s; the contract cannot be compared (S17)", [name])
}

deny contains msg if {
	count(hardening_docs) > 0
	count(of_kind(hardening_docs, "NetworkPolicy")) != 1
	msg := "crates/agent/HARDENING.md has no single NetworkPolicy block; the contract cannot be compared (S17)"
}

deny contains msg if {
	count(hardening_docs) > 0
	count(of_kind(hardening_docs, "Pod")) != 1
	msg := "crates/agent/HARDENING.md has no single Pod block; the contract cannot be compared (S16)"
}

# --- the chart makes one of each -----------------------------------------------------------------------------------------------

agent_deployments := named(rendered, "Deployment", lk.agent_name)

agent_policies := named(rendered, "NetworkPolicy", lk.agent_name)

deny contains msg if {
	count(agent_deployments) != 1
	msg := sprintf("the chart must render exactly one Deployment %s; found %d", [lk.agent_name, count(agent_deployments)])
}

# One policy, the agent's: a second one, whatever it is called, is another set of destinations the contract does not list.
deny contains msg if {
	count(of_kind(rendered, "NetworkPolicy")) != 1
	msg := sprintf("the chart must render exactly one NetworkPolicy; found %d", [count(of_kind(rendered, "NetworkPolicy"))])
}

deny contains msg if {
	count(agent_policies) != 1
	msg := sprintf("the chart must render a NetworkPolicy %s; found %d", [lk.agent_name, count(agent_policies)])
}

# --- Roles: the rules equal RBAC.md ---------------------------------------------------------------------------------------------

canonical(rule) := {
	"apiGroups": sort(object.get(rule, "apiGroups", [])),
	"resources": sort(object.get(rule, "resources", [])),
	"resourceNames": sort(object.get(rule, "resourceNames", [])),
	"verbs": sort(object.get(rule, "verbs", [])),
}

rule_set(role) := {canonical(r) | some r in object.get(role, "rules", [])}

contract_role(name) := role if {
	roles := named(rbac_docs, "Role", name)
	count(roles) == 1
	role := roles[0]
}

env_of(deployment, name) := v if {
	some c in deployment.spec.template.spec.containers
	v := lk.env_value(c, name)
}

# The Secrets the agent is configured to read: its certificate always, the join token only when LK_JOIN_TOKEN_SECRET is set.
configured_secrets(deployment) := {s |
	some variable in ["LK_CERT_SECRET", "LK_JOIN_TOKEN_SECRET"]
	s := env_of(deployment, variable)
}

# RBAC.md lists both Secrets; the chart grants the join token only with the variable that makes the agent read it.
expected_secret_rules(deployment) := {canonical(r) |
	some r in contract_role(secrets_name).rules
	every n in r.resourceNames {
		n in configured_secrets(deployment)
	}
}

deny contains msg if {
	some role in named(rendered, "Role", workloads_name)
	rule_set(role) != rule_set(contract_role(workloads_name))
	msg := sprintf("Role/%s in %s differs from RBAC.md: it grants %v, the file %v (S17)", [workloads_name, role.metadata.namespace, sort_rules(rule_set(role)), sort_rules(rule_set(contract_role(workloads_name)))])
}

deny contains msg if {
	some d in agent_deployments
	some role in named(rendered, "Role", secrets_name)
	rule_set(role) != expected_secret_rules(d)
	msg := sprintf("Role/%s in %s differs from RBAC.md: it grants %v, the file %v for the Secrets this agent is configured with (S17)", [secrets_name, role.metadata.namespace, sort_rules(rule_set(role)), sort_rules(expected_secret_rules(d))])
}

sort_rules(rules) := sort([sprintf("%v %v %v %v", [r.apiGroups, r.resources, r.resourceNames, r.verbs]) | some r in rules])

# The names in the Secrets Role are the names in the environment: a Role for one Secret and a variable for another is an agent that
# cannot read its certificate.
deny contains msg if {
	some d in agent_deployments
	some role in named(rendered, "Role", secrets_name)
	named_in_role := {n | some r in role.rules; some n in r.resourceNames}
	named_in_role != configured_secrets(d)
	msg := sprintf("Role/%s names the Secrets %v, and LK_CERT_SECRET and LK_JOIN_TOKEN_SECRET name %v (S17)", [secrets_name, sort(named_in_role), sort(configured_secrets(d))])
}

# --- Roles: one workload Role per watched namespace, and the Secrets Role in the agent's own ---------------------------------------

role_namespaces(name) := {role.metadata.namespace | some role in named(rendered, "Role", name)}

deny contains msg if {
	some d in agent_deployments
	some ns in lk.agent_namespaces
	not ns in role_namespaces(workloads_name)
	msg := sprintf("LK_NAMESPACES lists %s and there is no Role %s in it: the agent's watchers there would log 403 and retry (RBAC.md)", [ns, workloads_name])
}

deny contains msg if {
	some d in agent_deployments
	some ns in role_namespaces(workloads_name)
	not ns in lk.agent_namespaces
	msg := sprintf("Role %s is in namespace %s, which LK_NAMESPACES does not list; the agent has no business there (RBAC.md)", [workloads_name, ns])
}

deny contains msg if {
	some d in agent_deployments
	role_namespaces(secrets_name) != {d.metadata.namespace}
	msg := sprintf("Role %s must be in the agent's own namespace %s alone; it is in %v (RBAC.md)", [secrets_name, d.metadata.namespace, sort(role_namespaces(secrets_name))])
}

# --- RoleBindings equal RBAC.md -------------------------------------------------------------------------------------------------

deny contains msg if {
	some name in {workloads_name, secrets_name}
	some role in named(rendered, "Role", name)
	count([b | some b in named(rendered, "RoleBinding", name); b.metadata.namespace == role.metadata.namespace]) != 1
	msg := sprintf("Role/%s in %s has no RoleBinding of the same name in that namespace (RBAC.md)", [name, role.metadata.namespace])
}

contract_binding(name) := b if {
	bs := named(rbac_docs, "RoleBinding", name)
	count(bs) == 1
	b := bs[0]
}

deny contains msg if {
	some name in {workloads_name, secrets_name}
	some b in named(rendered, "RoleBinding", name)
	b.roleRef != contract_binding(name).roleRef
	msg := sprintf("RoleBinding/%s in %s: its roleRef %v differs from RBAC.md, %v (S17)", [name, b.metadata.namespace, b.roleRef, contract_binding(name).roleRef])
}

# The service account's namespace is the agent's own, whatever the example in RBAC.md says.
subject_shape(s) := {"kind": s.kind, "name": s.name}

deny contains msg if {
	some name in {workloads_name, secrets_name}
	some b in named(rendered, "RoleBinding", name)
	{subject_shape(s) | some s in object.get(b, "subjects", [])} != {subject_shape(s) | some s in contract_binding(name).subjects}
	msg := sprintf("RoleBinding/%s in %s: its subjects %v differ from RBAC.md, %v (S17)", [name, b.metadata.namespace, object.get(b, "subjects", []), contract_binding(name).subjects])
}

# --- NetworkPolicy equals HARDENING.md ------------------------------------------------------------------------------------------

metadata_cidr := "169.254.169.254/32"

# The hub's and the API server's addresses are values: any plain address block stands for the contract's example block.
normal_destination(d) := {"ipBlock": "<address block>"} if {
	d.ipBlock.cidr
	d.ipBlock.cidr != metadata_cidr
	not d.ipBlock.except
}

normal_destination(d) := d if d.ipBlock.cidr == metadata_cidr

normal_destination(d) := d if d.ipBlock.except

normal_destination(d) := d if not d.ipBlock

normal_ports(rule) := {{"protocol": object.get(p, "protocol", "TCP"), "port": p.port} | some p in object.get(rule, "ports", [])}

normal_to(rule) := {normal_destination(d) | some d in object.get(rule, "to", [])}

normal_from(rule) := {normal_destination(d) | some d in object.get(rule, "from", [])}

egress_set(policy) := sort([{"to": sort([x | some x in normal_to(r)]), "ports": sort([x | some x in normal_ports(r)])} | some r in object.get(policy.spec, "egress", [])])

ingress_set(policy) := sort([{"from": sort([x | some x in normal_from(r)]), "ports": sort([x | some x in normal_ports(r)])} | some r in object.get(policy.spec, "ingress", [])])

contract_policy := p if {
	ps := of_kind(hardening_docs, "NetworkPolicy")
	count(ps) == 1
	p := ps[0]
}

deny contains msg if {
	some p in agent_policies
	egress_set(p) != egress_set(contract_policy)
	msg := sprintf("NetworkPolicy/%s: its egress differs from HARDENING.md. The chart: %v. The file: %v (S17)", [p.metadata.name, egress_set(p), egress_set(contract_policy)])
}

deny contains msg if {
	some p in agent_policies
	ingress_set(p) != ingress_set(contract_policy)
	msg := sprintf("NetworkPolicy/%s: its ingress differs from HARDENING.md. The chart: %v. The file: %v (S18)", [p.metadata.name, ingress_set(p), ingress_set(contract_policy)])
}

deny contains msg if {
	some p in agent_policies
	sort(object.get(p.spec, "policyTypes", [])) != sort(contract_policy.spec.policyTypes)
	msg := sprintf("NetworkPolicy/%s: its policyTypes %v differs from HARDENING.md, %v (S18)", [p.metadata.name, object.get(p.spec, "policyTypes", []), contract_policy.spec.policyTypes])
}

deny contains msg if {
	some p in agent_policies
	object.get(p.spec.podSelector, "matchLabels", {}) != contract_policy.spec.podSelector.matchLabels
	msg := sprintf("NetworkPolicy/%s: its podSelector differs from HARDENING.md (S18)", [p.metadata.name])
}

# --- the pod equals HARDENING.md ------------------------------------------------------------------------------------------------

contract_pod := p if {
	ps := of_kind(hardening_docs, "Pod")
	count(ps) == 1
	p := ps[0]
}

pod_fields(spec) := {
	"serviceAccountName": object.get(spec, "serviceAccountName", ""),
	"automountServiceAccountToken": object.get(spec, "automountServiceAccountToken", null),
	"hostNetwork": object.get(spec, "hostNetwork", false),
	"hostPID": object.get(spec, "hostPID", false),
	"hostIPC": object.get(spec, "hostIPC", false),
	"runAsNonRoot": object.get(spec, ["securityContext", "runAsNonRoot"], null),
	"seccomp": object.get(spec, ["securityContext", "seccompProfile", "type"], null),
}

container_fields(c) := {
	"securityContext": object.get(c, "securityContext", {}),
	"ports": {[p.name, p.containerPort, object.get(p, "protocol", "TCP")] | some p in object.get(c, "ports", [])},
	"livenessProbe": object.get(c, "livenessProbe", null),
	"readinessProbe": object.get(c, "readinessProbe", null),
}

deny contains msg if {
	some d in agent_deployments
	some field, want in pod_fields(contract_pod.spec)
	got := pod_fields(d.spec.template.spec)[field]
	got != want
	msg := sprintf("%s: %s is %v and HARDENING.md says %v (S16)", [lk.who(d), field, got, want])
}

deny contains msg if {
	some d in agent_deployments
	count(d.spec.template.spec.containers) == 1
	count(contract_pod.spec.containers) == 1
	c := d.spec.template.spec.containers[0]
	some field, want in container_fields(contract_pod.spec.containers[0])
	got := container_fields(c)[field]
	got != want
	msg := sprintf("%s: the container's %s is %v and HARDENING.md says %v (S16)", [lk.who(d), field, got, want])
}

deny contains msg if {
	some d in agent_deployments
	count(d.spec.template.spec.containers) != 1
	msg := sprintf("%s: one container and no sidecar, as HARDENING.md says; found %d (S16)", [lk.who(d), count(d.spec.template.spec.containers)])
}
