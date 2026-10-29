# Tests for rbac.rego (S7, S17): what the agent's Roles may grant. The rendered Roles pass, and each planted Role (a Secret list, a
# Secret watch, an unnamed Secret grant, a ClusterRole, a wildcard, ...) is refused by its own rule, for its own reason.
# Run with `conftest verify --policy deploy/policy`.
package rbac

import rego.v1

import data.fixtures

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

# The Secrets Role of the fixture has one rule, rules/0: get and update on the certificate Secret.
secrets_role(ops) := fixtures.agent_patch("Role", "lanekeeper-agent-secrets", ops)

workloads_role(ops) := fixtures.agent_patch("Role", "lanekeeper-agent-workloads", ops)

secret_verbs(verbs) := secrets_role([{"op": "replace", "path": "/rules/0/verbs", "value": verbs}])

join_token_rule := {"apiGroups": [""], "resources": ["secrets"], "resourceNames": ["lanekeeper-agent-join-token"], "verbs": ["get"]}

extra_rule(rule) := workloads_role([{"op": "add", "path": "/rules/-", "value": rule}])

binding(ops) := fixtures.agent_patch("RoleBinding", "lanekeeper-agent-workloads", ops)

test_good_passes if {
	count(deny) == 0 with input as fixtures.agent
}

# The join-token grant is the one Secret rule beyond the certificate's, and the chart renders it only when it is enabled.
test_join_token_grant_passes if {
	planted := secrets_role([{"op": "add", "path": "/rules/-", "value": join_token_rule}])
	count(deny) == 0 with input as planted
}

# --- Secrets (S17, acceptance 4 of prompt 02): get and update, by name, nothing else

test_deny_secret_list if {
	refused(secret_verbs(["get", "list"]), "the verb list on Secrets")
}

test_deny_secret_list_alone if {
	refused(secret_verbs(["list"]), "the verb list on Secrets")
}

test_deny_secret_watch if {
	refused(secret_verbs(["get", "watch"]), "the verb watch on Secrets")
}

# Kubernetes does not apply resourceNames to list and watch, so a grant of either, named or not, reads every Secret of the namespace.
test_deny_secret_list_with_names if {
	refused(secret_verbs(["list", "watch"]), "every Secret")
}

test_deny_unnamed_secret if {
	refused(secrets_role([{"op": "remove", "path": "/rules/0/resourceNames"}]), "without resourceNames")
}

test_deny_unnamed_secret_empty_names if {
	refused(secrets_role([{"op": "replace", "path": "/rules/0/resourceNames", "value": []}]), "without resourceNames")
}

test_deny_secret_create if {
	refused(secret_verbs(["get", "create"]), "the verb create on Secrets")
}

test_deny_secret_delete if {
	refused(secret_verbs(["get", "delete"]), "the verb delete on Secrets")
}

test_deny_secret_patch if {
	refused(secret_verbs(["get", "patch"]), "the verb patch on Secrets")
}

test_deny_secret_deletecollection if {
	refused(secret_verbs(["deletecollection"]), "the verb deletecollection on Secrets")
}

test_deny_secret_wildcard_verb if {
	refused(secret_verbs(["*"]), "a wildcard")
}

test_deny_secret_of_another_name if {
	refused(secrets_role([{"op": "replace", "path": "/rules/0/resourceNames", "value": ["lanekeeper-db-app"]}]), "lanekeeper-db-app")
}

test_deny_secret_wildcard_name if {
	refused(secrets_role([{"op": "replace", "path": "/rules/0/resourceNames", "value": ["*"]}]), "a wildcard")
}

test_deny_secret_in_a_rule_with_other_resources if {
	refused(secrets_role([{"op": "replace", "path": "/rules/0/resources", "value": ["pods", "secrets"]}]), "share a rule")
}

# The Secrets are the agent's own: the Role that grants them is in the agent's namespace and not in a namespace it watches.
test_deny_secret_grant_in_a_watched_namespace if {
	planted := fixtures.agent_patch("Role", "lanekeeper-agent-secrets", [{"op": "replace", "path": "/metadata/namespace", "value": "sit1"}])
	refused(planted, "the agent's own namespace")
}

# --- no ClusterRole, no wildcard, nothing but the allow-list

test_deny_clusterrole if {
	planted := fixtures.agent_with_doc({
		"apiVersion": "rbac.authorization.k8s.io/v1",
		"kind": "ClusterRole",
		"metadata": {"name": "lanekeeper-agent-everything"},
		"rules": [{"apiGroups": [""], "resources": ["pods"], "verbs": ["get"]}],
	})
	refused(planted, "ClusterRole")
}

test_deny_clusterrole_for_secrets if {
	planted := fixtures.agent_with_doc({
		"apiVersion": "rbac.authorization.k8s.io/v1",
		"kind": "ClusterRole",
		"metadata": {"name": "lanekeeper-agent-secrets"},
		"rules": [{"apiGroups": [""], "resources": ["secrets"], "resourceNames": ["lanekeeper-agent-cert"], "verbs": ["get"]}],
	})
	refused(planted, "ClusterRole")
}

test_deny_clusterrolebinding if {
	planted := fixtures.agent_with_doc({
		"apiVersion": "rbac.authorization.k8s.io/v1",
		"kind": "ClusterRoleBinding",
		"metadata": {"name": "lanekeeper-agent"},
		"roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": "view"},
		"subjects": [{"kind": "ServiceAccount", "name": "lanekeeper-agent", "namespace": "lanekeeper"}],
	})
	refused(planted, "ClusterRoleBinding")
}

test_deny_wildcard_resource if {
	refused(extra_rule({"apiGroups": [""], "resources": ["*"], "verbs": ["get"]}), "a wildcard")
}

test_deny_wildcard_api_group if {
	refused(extra_rule({"apiGroups": ["*"], "resources": ["pods"], "verbs": ["get"]}), "a wildcard")
}

test_deny_wildcard_verb if {
	refused(extra_rule({"apiGroups": ["apps"], "resources": ["deployments"], "verbs": ["*"]}), "a wildcard")
}

test_deny_configmaps if {
	refused(extra_rule({"apiGroups": [""], "resources": ["configmaps"], "verbs": ["get", "list", "watch"]}), "configmaps")
}

test_deny_pods_exec if {
	refused(extra_rule({"apiGroups": [""], "resources": ["pods/exec"], "verbs": ["create"]}), "pods/exec")
}

test_deny_pods_log if {
	refused(extra_rule({"apiGroups": [""], "resources": ["pods/log"], "verbs": ["get"]}), "pods/log")
}

test_deny_delete_deployments if {
	refused(extra_rule({"apiGroups": ["apps"], "resources": ["deployments"], "verbs": ["delete"]}), "the verb delete on deployments")
}

test_deny_create_deployments if {
	refused(extra_rule({"apiGroups": ["apps"], "resources": ["deployments"], "verbs": ["create"]}), "the verb create on deployments")
}

test_deny_update_jobs if {
	refused(extra_rule({"apiGroups": ["batch"], "resources": ["jobs"], "verbs": ["update"]}), "the verb update on jobs")
}

test_deny_delete_pods if {
	refused(extra_rule({"apiGroups": [""], "resources": ["pods"], "verbs": ["delete"]}), "the verb delete on pods")
}

test_deny_privilege_escalation_verbs if {
	refused(extra_rule({"apiGroups": ["rbac.authorization.k8s.io"], "resources": ["roles"], "verbs": ["escalate", "bind"]}), "roles")
}

test_deny_impersonate if {
	refused(extra_rule({"apiGroups": [""], "resources": ["serviceaccounts"], "verbs": ["impersonate"]}), "serviceaccounts")
}

test_deny_non_resource_urls if {
	refused(extra_rule({"nonResourceURLs": ["/metrics"], "verbs": ["get"]}), "nonResourceURLs")
}

# --- who the Roles are bound to

test_deny_binding_to_a_cluster_role if {
	refused(binding([{"op": "replace", "path": "/roleRef/kind", "value": "ClusterRole"}]), "binds a ClusterRole")
}

test_deny_binding_to_another_role if {
	refused(binding([{"op": "replace", "path": "/roleRef/name", "value": "admin"}]), "a Role that this chart does not render")
}

test_deny_binding_to_a_user if {
	refused(binding([{"op": "replace", "path": "/subjects", "value": [{"kind": "User", "name": "someone"}]}]), "lanekeeper-agent")
}

test_deny_binding_to_a_group if {
	refused(binding([{"op": "replace", "path": "/subjects", "value": [{"kind": "Group", "name": "system:authenticated"}]}]), "lanekeeper-agent")
}

test_deny_binding_to_another_service_account if {
	refused(binding([{"op": "replace", "path": "/subjects", "value": [{"kind": "ServiceAccount", "name": "default", "namespace": "lanekeeper"}]}]), "lanekeeper-agent")
}

test_deny_binding_to_a_service_account_elsewhere if {
	refused(binding([{"op": "replace", "path": "/subjects/0/namespace", "value": "kube-system"}]), "the agent's namespace")
}

test_deny_binding_without_a_subject if {
	refused(binding([{"op": "replace", "path": "/subjects", "value": []}]), "names no subject")
}

test_deny_binding_with_an_extra_subject if {
	refused(binding([{"op": "add", "path": "/subjects/-", "value": {"kind": "ServiceAccount", "name": "default", "namespace": "lanekeeper"}}]), "lanekeeper-agent")
}

# --- namespaces: the Roles are in the namespaces the agent is configured for, and nowhere else

test_deny_role_in_a_namespace_the_agent_does_not_watch if {
	planted := fixtures.agent_with_doc(fixtures.agent_workload_role("kube-system"))
	refused(planted, "kube-system")
}

test_deny_binding_in_a_namespace_the_agent_does_not_watch if {
	planted := fixtures.agent_with_doc(fixtures.agent_binding("lanekeeper-agent-workloads", "kube-system"))
	refused(planted, "kube-system")
}

test_two_watched_namespaces_pass if {
	planted := fixtures.agent_with(array.concat(
		[d | some d in fixtures.agent_docs; d.kind != "Deployment"],
		[
			json.patch(fixtures.agent_deployment, [{"op": "replace", "path": "/spec/template/spec/containers/0/env/11/value", "value": "sit1,sit2"}]),
			fixtures.agent_workload_role("sit2"),
			fixtures.agent_binding("lanekeeper-agent-workloads", "sit2"),
		],
	))
	count(deny) == 0 with input as planted
}
