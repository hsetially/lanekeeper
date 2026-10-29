# Tests for contract.rego (S17): the agent chart's rendered Roles equal crates/agent/RBAC.md, and its NetworkPolicy and pod equal
# crates/agent/HARDENING.md. A change to the chart that the contract does not say, and a change to the contract that the chart
# does not follow, are both refused. Run with `conftest verify --policy deploy/policy`.
package contract

import rego.v1

import data.fixtures

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

patched(kind, name, ops) := fixtures.agent_contract_patch(kind, name, ops)

# The contract files changed: file 0 is RBAC.md's, file 1 is HARDENING.md's.
rbac_changed(kind, name, ops) := fixtures.agent_contract_changed(0, kind, name, ops)

hardening_changed(kind, name, ops) := fixtures.agent_contract_changed(1, kind, name, ops)

join_token_rule := {"apiGroups": [""], "resources": ["secrets"], "resourceNames": ["lanekeeper-agent-join-token"], "verbs": ["get"]}

join_token_env := {"name": "LK_JOIN_TOKEN_SECRET", "value": "lanekeeper-agent-join-token"}

test_good_passes if {
	count(deny) == 0 with input as fixtures.agent_with_contract
}

# --- the contract files are there, and say what they should

test_deny_without_contract_files if {
	refused(fixtures.agent, "agent-rbac-contract.yaml")
	refused(fixtures.agent, "agent-hardening-contract.yaml")
}

# Each file is needed on its own: with one of them, the other one's absence is said, and only that one's.
test_deny_without_the_rbac_contract_file if {
	planted := array.concat(fixtures.agent, [fixtures.agent_contract[1]])
	refused(planted, "agent-rbac-contract.yaml")
	not refused(planted, "agent-hardening-contract.yaml")
}

test_deny_without_the_hardening_contract_file if {
	planted := array.concat(fixtures.agent, [fixtures.agent_contract[0]])
	refused(planted, "agent-hardening-contract.yaml")
	not refused(planted, "agent-rbac-contract.yaml")
}

# The extraction can lose a block; a contract that lacks a document must say so and not compare against nothing.
rbac_without(kind, name) := array.concat(fixtures.agent, [
	{"path": "contract/agent-rbac-contract.yaml", "contents": [d | some d in fixtures.agent_contract_rbac_docs; not is_doc(d, kind, name)]},
	fixtures.agent_contract[1],
])

is_doc(d, kind, name) if {
	d.kind == kind
	d.metadata.name == name
}

test_deny_contract_without_workload_role if {
	refused(rbac_without("Role", "lanekeeper-agent-workloads"), "no single Role lanekeeper-agent-workloads")
}

test_deny_contract_without_secrets_role if {
	refused(rbac_without("Role", "lanekeeper-agent-secrets"), "no single Role lanekeeper-agent-secrets")
}

test_deny_contract_without_workload_binding if {
	refused(rbac_without("RoleBinding", "lanekeeper-agent-workloads"), "no single RoleBinding lanekeeper-agent-workloads")
}

test_deny_contract_without_secrets_binding if {
	refused(rbac_without("RoleBinding", "lanekeeper-agent-secrets"), "no single RoleBinding lanekeeper-agent-secrets")
}

test_deny_contract_without_network_policy if {
	planted := array.concat(fixtures.agent, [
		fixtures.agent_contract[0],
		{"path": "contract/agent-hardening-contract.yaml", "contents": [fixtures.agent_contract_pod]},
	])
	refused(planted, "no single NetworkPolicy block")
}

test_deny_contract_without_pod if {
	planted := array.concat(fixtures.agent, [
		fixtures.agent_contract[0],
		{"path": "contract/agent-hardening-contract.yaml", "contents": [fixtures.agent_contract_networkpolicy]},
	])
	refused(planted, "no single Pod block")
}

# --- the workload Role equals RBAC.md

test_deny_workload_role_without_a_verb if {
	refused(patched("Role", "lanekeeper-agent-workloads", [{"op": "replace", "path": "/rules/1/verbs", "value": []}]), "differs from RBAC.md")
}

test_deny_workload_role_with_an_extra_rule if {
	extra := {"apiGroups": [""], "resources": ["pods"], "verbs": ["delete"]}
	refused(patched("Role", "lanekeeper-agent-workloads", [{"op": "add", "path": "/rules/-", "value": extra}]), "differs from RBAC.md")
}

test_deny_workload_role_with_a_verb_less_than_the_contract if {
	refused(patched("Role", "lanekeeper-agent-workloads", [{"op": "remove", "path": "/rules/3"}]), "differs from RBAC.md")
}

# Rules are a set: the order and the order of the lists inside a rule are not part of the contract.
test_workload_role_in_another_order_passes if {
	planted := patched("Role", "lanekeeper-agent-workloads", [{"op": "replace", "path": "/rules", "value": [
		{"apiGroups": ["batch"], "resources": ["jobs"], "verbs": ["watch", "list", "get"]},
		{"apiGroups": [""], "resources": ["pods"], "verbs": ["watch", "get", "list"]},
		{"apiGroups": ["apps"], "resources": ["deployments"], "verbs": ["patch"]},
		{"apiGroups": ["apps"], "resources": ["deployments"], "verbs": ["list", "get", "watch"]},
	]}])
	count(deny) == 0 with input as planted
}

# The contract moves first: RBAC.md grants one more thing, and the chart does not.
test_deny_contract_that_grants_more_than_the_chart if {
	extra := {"apiGroups": [""], "resources": ["events"], "verbs": ["create"]}
	refused(rbac_changed("Role", "lanekeeper-agent-workloads", [{"op": "add", "path": "/rules/-", "value": extra}]), "differs from RBAC.md")
}

# --- the Secrets Role: the certificate always, the join token only when it is configured

test_deny_secrets_role_without_update if {
	refused(patched("Role", "lanekeeper-agent-secrets", [{"op": "replace", "path": "/rules/0/verbs", "value": ["get"]}]), "differs from RBAC.md")
}

test_deny_secrets_role_with_a_wider_verb if {
	refused(patched("Role", "lanekeeper-agent-secrets", [{"op": "replace", "path": "/rules/0/verbs", "value": ["get", "update", "patch"]}]), "differs from RBAC.md")
}

test_deny_join_token_grant_without_the_variable if {
	refused(patched("Role", "lanekeeper-agent-secrets", [{"op": "add", "path": "/rules/-", "value": join_token_rule}]), "differs from RBAC.md")
}

test_deny_join_token_variable_without_the_grant if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/containers/0/env/-", "value": join_token_env}]), "differs from RBAC.md")
}

test_join_token_with_grant_and_variable_passes if {
	docs := [d | some d in fixtures.agent_docs; d.kind != "Role"; d.kind != "Deployment"]
	with_token := array.concat(docs, [
		fixtures.agent_workload_role("sit1"),
		json.patch(fixtures.agent_secrets_role, [{"op": "add", "path": "/rules/-", "value": join_token_rule}]),
		json.patch(fixtures.agent_deployment, [{"op": "add", "path": "/spec/template/spec/containers/0/env/-", "value": join_token_env}]),
	])
	count(deny) == 0 with input as fixtures.agent_contract_with(with_token)
}

test_deny_secret_role_naming_another_secret_than_the_variable if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/env/9/value", "value": "other-cert"}]), "LK_CERT_SECRET")
}

# --- the bindings

test_deny_missing_workload_binding if {
	planted := fixtures.agent_contract_with([d | some d in fixtures.agent_docs; not is_workload_binding(d)])
	refused(planted, "no RoleBinding")
}

is_workload_binding(d) if {
	d.kind == "RoleBinding"
	d.metadata.name == "lanekeeper-agent-workloads"
}

test_deny_binding_to_another_role if {
	refused(patched("RoleBinding", "lanekeeper-agent-secrets", [{"op": "replace", "path": "/roleRef/name", "value": "lanekeeper-agent-workloads"}]), "roleRef")
}

test_deny_binding_to_another_subject if {
	refused(patched("RoleBinding", "lanekeeper-agent-secrets", [{"op": "replace", "path": "/subjects/0/name", "value": "default"}]), "subjects")
}

test_deny_binding_of_a_cluster_role_kind if {
	refused(patched("RoleBinding", "lanekeeper-agent-workloads", [{"op": "replace", "path": "/roleRef/kind", "value": "ClusterRole"}]), "roleRef")
}

# --- the namespaces: the Roles and LK_NAMESPACES name the same ones (RBAC.md, "Notes for the chart")

test_deny_watched_namespace_without_a_role if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/env/11/value", "value": "sit1,sit2"}]), "sit2")
}

test_deny_role_in_a_namespace_that_is_not_watched if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/env/11/value", "value": "sit2"}]), "sit1")
}

test_two_watched_namespaces_pass if {
	docs := array.concat(
		[d | some d in fixtures.agent_docs; d.kind != "Deployment"],
		[
			json.patch(fixtures.agent_deployment, [{"op": "replace", "path": "/spec/template/spec/containers/0/env/11/value", "value": "sit1,sit2"}]),
			fixtures.agent_workload_role("sit2"),
			fixtures.agent_binding("lanekeeper-agent-workloads", "sit2"),
		],
	)
	count(deny) == 0 with input as fixtures.agent_contract_with(docs)
}

test_deny_binding_in_the_wrong_namespace if {
	refused(patched("RoleBinding", "lanekeeper-agent-workloads", [{"op": "replace", "path": "/metadata/namespace", "value": "sit2"}]), "no RoleBinding")
}

# --- the NetworkPolicy equals HARDENING.md

test_deny_egress_rule_missing if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "remove", "path": "/spec/egress/3"}]), "differs from HARDENING.md")
}

test_deny_egress_rule_added if {
	extra := {"to": [{"ipBlock": {"cidr": "192.0.2.77/32"}}], "ports": [{"protocol": "TCP", "port": 443}]}
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "add", "path": "/spec/egress/-", "value": extra}]), "differs from HARDENING.md")
}

test_deny_egress_port_changed if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": "/spec/egress/2/ports/0/port", "value": 8889}]), "differs from HARDENING.md")
}

test_deny_egress_port_added if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "add", "path": "/spec/egress/0/ports/-", "value": {"protocol": "TCP", "port": 8444}}]), "differs from HARDENING.md")
}

test_deny_egress_protocol_added_to_dns if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "remove", "path": "/spec/egress/4/ports/1"}]), "differs from HARDENING.md")
}

test_deny_egress_block_with_exceptions if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "add", "path": "/spec/egress/0/to/0/ipBlock/except", "value": ["203.0.113.1/32"]}]), "differs from HARDENING.md")
}

# The hub's and the API server's addresses are values, so the block in the contract is a placeholder for any address.
test_other_addresses_for_the_hub_and_the_api_server_pass if {
	planted := patched("NetworkPolicy", "lanekeeper-agent", [
		{"op": "replace", "path": "/spec/egress/0/to/0/ipBlock/cidr", "value": "192.0.2.80/32"},
		{"op": "replace", "path": "/spec/egress/1/to/0/ipBlock/cidr", "value": "198.51.100.0/24"},
	])
	count(deny) == 0 with input as planted
}

test_several_addresses_for_the_hub_pass if {
	planted := patched("NetworkPolicy", "lanekeeper-agent", [{"op": "add", "path": "/spec/egress/0/to/-", "value": {"ipBlock": {"cidr": "192.0.2.81/32"}}}])
	count(deny) == 0 with input as planted
}

test_deny_ingress_port_changed if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": "/spec/ingress/0/ports/0/port", "value": 8080}]), "differs from HARDENING.md")
}

test_deny_ingress_source_changed if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": "/spec/ingress/0/from/0/namespaceSelector/matchLabels/kubernetes.io~1metadata.name", "value": "default"}]), "differs from HARDENING.md")
}

test_deny_policy_types_changed if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": "/spec/policyTypes", "value": ["Egress"]}]), "differs from HARDENING.md")
}

test_deny_pod_selector_changed if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": "/spec/podSelector/matchLabels", "value": {"app.kubernetes.io/name": "other"}}]), "differs from HARDENING.md")
}

# The contract moves first: HARDENING.md allows one more destination, and the chart does not.
test_deny_contract_that_allows_more_than_the_chart if {
	extra := {"to": [{"ipBlock": {"cidr": "169.254.169.250/32"}}], "ports": [{"protocol": "TCP", "port": 80}]}
	refused(hardening_changed("NetworkPolicy", "lanekeeper-agent", [{"op": "add", "path": "/spec/egress/-", "value": extra}]), "differs from HARDENING.md")
}

test_deny_contract_whose_config_server_port_moved if {
	refused(hardening_changed("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": "/spec/egress/2/ports/0/port", "value": 9999}]), "differs from HARDENING.md")
}

# The config-server's namespace is a value per swimlane (HARDENING.md: "latest-pci here"): the example stands for any one explicit
# namespace, chosen by kubernetes.io/metadata.name in the same entry as the pods, on the chart's side and on the contract's. What is still
# compared is the shape: a rule that is wider, or chosen by anything else, is not the contract's.
config_server_namespace_path := "/spec/egress/2/to/0/namespaceSelector"

config_server_name_path := concat("", [config_server_namespace_path, "/matchLabels/kubernetes.io~1metadata.name"])

test_the_config_servers_namespace_is_a_value if {
	planted := patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": config_server_name_path, "value": "other-pci"}])
	count(deny) == 0 with input as planted
}

test_the_config_servers_namespace_in_the_contract_is_a_value_too if {
	planted := hardening_changed("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": config_server_name_path, "value": "another-pci"}])
	count(deny) == 0 with input as planted
}

test_deny_a_config_server_rule_without_a_namespace if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "remove", "path": config_server_namespace_path}]), "differs from HARDENING.md")
}

test_deny_a_config_server_rule_in_every_namespace if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": config_server_namespace_path, "value": {}}]), "differs from HARDENING.md")
}

test_deny_a_config_server_rule_in_namespaces_chosen_by_an_expression if {
	selector := {"matchExpressions": [{"key": "kubernetes.io/metadata.name", "operator": "In", "values": ["latest-pci"]}]}
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": config_server_namespace_path, "value": selector}]), "differs from HARDENING.md")
}

test_deny_a_config_server_rule_in_namespaces_chosen_by_another_label if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": config_server_namespace_path, "value": {"matchLabels": {"team": "pci"}}}]), "differs from HARDENING.md")
}

test_deny_a_config_server_rule_in_namespaces_chosen_by_two_labels if {
	selector := {"matchLabels": {"kubernetes.io/metadata.name": "latest-pci", "team": "pci"}}
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": config_server_namespace_path, "value": selector}]), "differs from HARDENING.md")
}

test_deny_a_config_server_rule_whose_namespace_is_not_a_string if {
	selector := {"matchLabels": {"kubernetes.io/metadata.name": ["latest-pci"]}}
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": config_server_namespace_path, "value": selector}]), "differs from HARDENING.md")
}

# The contract moves first: HARDENING.md widens the rule, or drops the namespace, and the chart does not follow.
test_deny_contract_that_widens_the_config_servers_namespace if {
	refused(hardening_changed("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": config_server_namespace_path, "value": {}}]), "differs from HARDENING.md")
}

test_deny_contract_that_drops_the_config_servers_namespace if {
	refused(hardening_changed("NetworkPolicy", "lanekeeper-agent", [{"op": "remove", "path": config_server_namespace_path}]), "differs from HARDENING.md")
}

# --- the pod equals HARDENING.md

test_deny_pod_token_not_mounted if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/automountServiceAccountToken", "value": false}]), "automountServiceAccountToken")
}

test_deny_pod_with_another_service_account if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/serviceAccountName", "value": "default"}]), "serviceAccountName")
}

test_deny_pod_seccomp_changed if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/securityContext/seccompProfile/type", "value": "Unconfined"}]), "seccomp")
}

test_deny_pod_on_the_host_network if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/hostNetwork", "value": true}]), "hostNetwork")
}

test_deny_container_writable_root if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/securityContext/readOnlyRootFilesystem", "value": false}]), "securityContext")
}

test_deny_container_with_a_capability if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/containers/0/securityContext/capabilities/add", "value": ["NET_RAW"]}]), "securityContext")
}

test_deny_container_port_changed if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/ports/0/containerPort", "value": 8080}]), "ports")
}

test_deny_liveness_probe_changed if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/livenessProbe/periodSeconds", "value": 60}]), "livenessProbe")
}

test_deny_readiness_probe_changed if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/readinessProbe/httpGet/path", "value": "/healthz"}]), "readinessProbe")
}

# The contract moves first: HARDENING.md asks for a stricter field, and the chart does not have it.
test_deny_contract_with_a_stricter_probe if {
	planted := hardening_changed("Pod", "lanekeeper-agent", [{"op": "replace", "path": "/spec/containers/0/readinessProbe/periodSeconds", "value": 5}])
	refused(planted, "readinessProbe")
}

test_deny_contract_that_drops_a_capability_the_chart_keeps if {
	planted := hardening_changed("Pod", "lanekeeper-agent", [{"op": "replace", "path": "/spec/containers/0/securityContext/readOnlyRootFilesystem", "value": false}])
	refused(planted, "securityContext")
}

# The ids and the resources are values; the contract's numbers are the defaults, to be confirmed on the pilot.
test_other_ids_and_resources_pass if {
	planted := patched("Deployment", "lanekeeper-agent", [
		{"op": "replace", "path": "/spec/template/spec/securityContext/runAsUser", "value": 2020},
		{"op": "replace", "path": "/spec/template/spec/securityContext/runAsGroup", "value": 2020},
		{"op": "replace", "path": "/spec/template/spec/securityContext/fsGroup", "value": 2020},
		{"op": "replace", "path": "/spec/template/spec/containers/0/resources/limits/memory", "value": "256Mi"},
	])
	count(deny) == 0 with input as planted
}

# --- the chart makes one of each

test_deny_without_a_deployment if {
	refused(fixtures.agent_contract_with([d | some d in fixtures.agent_docs; d.kind != "Deployment"]), "Deployment")
}

test_deny_without_a_network_policy if {
	refused(fixtures.agent_contract_with([d | some d in fixtures.agent_docs; d.kind != "NetworkPolicy"]), "NetworkPolicy")
}

test_deny_a_network_policy_of_another_name if {
	refused(patched("NetworkPolicy", "lanekeeper-agent", [{"op": "replace", "path": "/metadata/name", "value": "lanekeeper-agent-extra"}]), "must render a NetworkPolicy lanekeeper-agent")
}

test_deny_secrets_role_in_a_watched_namespace if {
	refused(patched("Role", "lanekeeper-agent-secrets", [{"op": "replace", "path": "/metadata/namespace", "value": "sit1"}]), "own namespace lanekeeper alone")
}

test_deny_a_second_container if {
	planted := patched("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/containers/-", "value": {"name": "sidecar", "image": "x"}}])
	refused(planted, "no sidecar")
}

test_deny_two_network_policies if {
	second := json.patch(fixtures.agent_networkpolicy, [{"op": "replace", "path": "/metadata/name", "value": "lanekeeper-agent-extra"}])
	refused(fixtures.agent_contract_with(array.concat(fixtures.agent_docs, [second])), "exactly one")
}

# --- the spool, and the pod's paths (09/T8; D74): the volumes, the mounts and the path variables equal HARDENING.md, the pod is
# replaced and not surged because a volume of the contract is a claim, and the claim the contract names is rendered

spool_volume_op(value) := {"op": "replace", "path": "/spec/template/spec/volumes/2", "value": value}

test_deny_volume_that_differs_from_the_contract if {
	refused(patched("Deployment", "lanekeeper-agent", [spool_volume_op({"name": "spool", "persistentVolumeClaim": {"claimName": "lanekeeper-agent-other"}})]), "volume spool is")
}

test_deny_volume_of_another_kind_than_the_contract if {
	refused(patched("Deployment", "lanekeeper-agent", [spool_volume_op({"name": "spool", "emptyDir": {}})]), "volume spool is")
}

test_deny_volume_the_contract_has_and_the_pod_lacks if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "remove", "path": "/spec/template/spec/volumes/2"}]), "the pod has no volume spool")
}

test_deny_volume_the_contract_lacks if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/volumes/-", "value": {"name": "scratch", "emptyDir": {}}}]), "volume scratch is not in HARDENING.md")
}

test_deny_hub_ca_volume_from_another_config_map if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/volumes/3/configMap/name", "value": "other-ca"}]), "volume hub-ca is")
}

test_deny_tmp_volume_without_the_size_limit if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/volumes/1/emptyDir", "value": {}}]), "volume tmp is")
}

test_deny_mount_path_that_differs_from_the_contract if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/volumeMounts/2/mountPath", "value": "/var/spool/lk"}]), "volumeMounts differ from HARDENING.md")
}

test_deny_mount_the_contract_lacks if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/containers/0/volumeMounts/-", "value": {"name": "scratch", "mountPath": "/scratch"}}]), "volumeMounts differ from HARDENING.md")
}

test_deny_hub_ca_mount_that_is_not_read_only if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "remove", "path": "/spec/template/spec/containers/0/volumeMounts/3/readOnly"}]), "volumeMounts differ from HARDENING.md")
}

test_deny_spool_dir_that_differs_from_the_contract if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/env/12/value", "value": "/var/spool/lk"}]), "LK_SPOOL_DIR is")
}

test_deny_tmp_dir_that_differs_from_the_contract if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/env/14/value", "value": "/var/tmp"}]), "LK_TMP_DIR is")
}

test_deny_path_variable_the_pod_lacks if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "remove", "path": "/spec/template/spec/containers/0/env/12"}]), "the pod does not set LK_SPOOL_DIR")
}

test_deny_path_variable_the_contract_lacks if {
	refused(hardening_changed("Pod", "lanekeeper-agent", [{"op": "remove", "path": "/spec/containers/0/env/1"}]), "HARDENING.md does not set LK_NFS_ROOT")
}

test_deny_rolling_update_when_a_volume_of_the_contract_is_a_claim if {
	refused(patched("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/strategy/type", "value": "RollingUpdate"}]), "strategy.type must be Recreate")
}

test_recreate_is_not_asked_when_the_contract_has_no_claim if {
	no_claim := hardening_changed("Pod", "lanekeeper-agent", [{"op": "replace", "path": "/spec/volumes/2", "value": {"name": "spool", "emptyDir": {}}}])
	planted := array.concat(
		[{"path": "rendered/agent.yaml", "contents": [json.patch(d, [{"op": "replace", "path": "/spec/strategy/type", "value": "RollingUpdate"}]) | some d in fixtures.agent_docs; d.kind == "Deployment"]}],
		[no_claim[1]],
	)
	not refused(planted, "strategy.type must be Recreate")
}

test_deny_claim_the_contract_names_but_the_chart_does_not_render if {
	refused(fixtures.agent_contract_with([d | some d in fixtures.agent_docs; d.kind != "PersistentVolumeClaim"]), "renders no PersistentVolumeClaim lanekeeper-agent-spool")
}
