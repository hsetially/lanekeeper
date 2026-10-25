# The documents the agent policy tests start from (09/T4). `agent` is what `helm template` makes of the agent chart, written out as
# conftest's `--combine` input (see lib.rego); `agent_contract` is what `deploy/tests/contract-extract.sh` makes of the YAML blocks of
# crates/agent/RBAC.md and crates/agent/HARDENING.md. Both take the same helpers as the hub's documents in fixtures_test.rego:
# `agent_patch` changes one document with a JSON patch, `agent_without` drops the documents of a kind, `agent_with_doc` adds one.
#
# `deploy/tests/render.sh` runs the real chart through the same policies, so a document here that stops resembling the chart's
# output does not hide a gap: the real output is checked too. Keep them alike; when the chart changes shape, change this.
# Documentation addresses only (RFC 5737), never a real one.
package fixtures

import rego.v1

agent_name := "lanekeeper-agent"

agent_selector := {"app.kubernetes.io/name": "lanekeeper-agent"}

agent_pod_labels := {
	"app.kubernetes.io/name": "lanekeeper-agent",
	"app.kubernetes.io/instance": "lanekeeper",
	"app.kubernetes.io/part-of": "lanekeeper",
}

agent_container := {
	"name": "agent",
	"image": concat("", ["ghcr.io/hsetially/lanekeeper/agent@", digest]),
	"imagePullPolicy": "IfNotPresent",
	"securityContext": {
		"allowPrivilegeEscalation": false,
		"privileged": false,
		"readOnlyRootFilesystem": true,
		"capabilities": {"drop": ["ALL"]},
	},
	"resources": {
		"requests": {"cpu": "25m", "memory": "64Mi"},
		"limits": {"cpu": "1", "memory": "128Mi"},
	},
	"ports": [{"name": "ops", "containerPort": 9090, "protocol": "TCP"}],
	"livenessProbe": {
		"httpGet": {"path": "/healthz", "port": "ops"},
		"periodSeconds": 20,
		"timeoutSeconds": 3,
		"failureThreshold": 3,
	},
	"readinessProbe": {
		"httpGet": {"path": "/readyz", "port": "ops"},
		"periodSeconds": 10,
		"timeoutSeconds": 3,
	},
	"env": [
		{"name": "LK_HUB_ENDPOINT", "value": "https://hub.example.com"},
		{"name": "LK_HUB_AUDIENCE", "value": "https://hub.example.com"},
		{"name": "LK_HUB_CA_FILE", "value": "/etc/lanekeeper/hub-ca/ca.pem"},
		{"name": "LK_SWIMLANE", "value": "sit1"},
		{"name": "LK_CLUSTER", "value": "example-cluster"},
		{"name": "LK_PROJECT", "value": "example-project"},
		{"name": "LK_NFS_SERVER", "value": "192.0.2.10"},
		{"name": "LK_NFS_EXPORT", "value": "/export/csp-configuration"},
		{"name": "LK_NFS_ROOT", "value": "/mnt/csp-configuration"},
		{"name": "LK_CERT_SECRET", "value": "lanekeeper-agent-cert"},
		{"name": "LK_JOIN_MODE", "value": "workload-identity"},
		{"name": "LK_NAMESPACES", "value": "sit1"},
		{"name": "LK_SPOOL_DIR", "value": "/var/lib/lanekeeper/spool"},
		{"name": "LK_SPOOL_MAX_BYTES", "value": "536870912"},
		{"name": "LK_TMP_DIR", "value": "/tmp"},
		{"name": "LK_CONFIG_SERVER_URL", "value": "http://csp-configuration-server:8888"},
		{"name": "LK_HEALTH_ADDR", "value": "0.0.0.0:9090"},
	],
	"volumeMounts": [
		{"name": "nfs", "mountPath": "/mnt/csp-configuration"},
		{"name": "tmp", "mountPath": "/tmp"},
		{"name": "spool", "mountPath": "/var/lib/lanekeeper/spool"},
		{"name": "hub-ca", "mountPath": "/etc/lanekeeper/hub-ca", "readOnly": true},
	],
}

agent_deployment := {
	"apiVersion": "apps/v1",
	"kind": "Deployment",
	"metadata": {"name": agent_name, "namespace": namespace, "labels": agent_pod_labels},
	"spec": {
		"replicas": 1,
		# The spool is a ReadWriteOnce volume that one agent writes: a rolling update would start a second agent beside the first.
		"strategy": {"type": "Recreate"},
		"selector": {"matchLabels": agent_pod_labels},
		"template": {
			"metadata": {"labels": agent_pod_labels, "annotations": {"checksum/hub-ca": "0000000000000000000000000000000000000000000000000000000000000000"}},
			"spec": {
				"serviceAccountName": agent_name,
				"automountServiceAccountToken": true,
				"enableServiceLinks": false,
				"securityContext": {
					"runAsNonRoot": true,
					"runAsUser": 1010,
					"runAsGroup": 1010,
					"fsGroup": 1010,
					"seccompProfile": {"type": "RuntimeDefault"},
				},
				"containers": [agent_container],
				"volumes": [
					{"name": "nfs", "nfs": {"server": "192.0.2.10", "path": "/export/csp-configuration", "readOnly": false}},
					{"name": "tmp", "emptyDir": {"sizeLimit": "64Mi"}},
					{"name": "spool", "persistentVolumeClaim": {"claimName": "lanekeeper-agent-spool"}},
					{"name": "hub-ca", "configMap": {"name": "lanekeeper-hub-ca"}},
				],
			},
		},
	},
}

agent_serviceaccount := {
	"apiVersion": "v1",
	"kind": "ServiceAccount",
	"metadata": {
		"name": agent_name,
		"namespace": namespace,
		"labels": agent_pod_labels,
		"annotations": {"iam.gke.io/gcp-service-account": "lanekeeper-agent-sit1@example-project.iam.gserviceaccount.com"},
	},
	"automountServiceAccountToken": true,
}

agent_cert_secret := {
	"apiVersion": "v1",
	"kind": "Secret",
	"type": "Opaque",
	"metadata": {
		"name": "lanekeeper-agent-cert",
		"namespace": namespace,
		"labels": agent_pod_labels,
		"annotations": {"helm.sh/resource-policy": "keep"},
	},
}

agent_hub_ca := {
	"apiVersion": "v1",
	"kind": "ConfigMap",
	"metadata": {"name": "lanekeeper-hub-ca", "namespace": namespace, "labels": agent_pod_labels},
	"data": {"ca.pem": "-----BEGIN CERTIFICATE-----\nMIIBsynthetic\n-----END CERTIFICATE-----\n"},
}

# The spool (D74): one claim of 1 GiB on a class that carries a customer-managed key (S10), which the chart renders with it. The class
# name is the one agent/templates/storageclass.yaml makes.
agent_storageclass := {
	"apiVersion": "storage.k8s.io/v1",
	"kind": "StorageClass",
	"metadata": {"name": "lanekeeper-agent-cmek", "labels": agent_pod_labels},
	"provisioner": "pd.csi.storage.gke.io",
	"parameters": {"type": "pd-balanced", "disk-encryption-kms-key": "projects/example-project/locations/example-location/keyRings/lanekeeper/cryptoKeys/lanekeeper-disk-sit1"},
	"volumeBindingMode": "WaitForFirstConsumer",
	"allowVolumeExpansion": true,
	"reclaimPolicy": "Delete",
}

agent_spool_pvc := {
	"apiVersion": "v1",
	"kind": "PersistentVolumeClaim",
	"metadata": {
		"name": "lanekeeper-agent-spool",
		"namespace": namespace,
		"labels": agent_pod_labels,
		"annotations": {
			"helm.sh/resource-policy": "keep",
			"argocd.argoproj.io/sync-options": "Prune=false",
		},
	},
	"spec": {
		"accessModes": ["ReadWriteOnce"],
		"storageClassName": "lanekeeper-agent-cmek",
		"resources": {"requests": {"storage": "1Gi"}},
	},
}

# RBAC.md, rendered: one workload Role and RoleBinding per watched namespace (here sit1) and one pair for the two Secrets.
agent_workload_rules := [
	{"apiGroups": ["apps"], "resources": ["deployments"], "verbs": ["get", "list", "watch"]},
	{"apiGroups": ["apps"], "resources": ["deployments"], "verbs": ["patch"]},
	{"apiGroups": [""], "resources": ["pods"], "verbs": ["get", "list", "watch"]},
	{"apiGroups": ["batch"], "resources": ["jobs"], "verbs": ["get", "list", "watch"]},
]

agent_secret_rules := [{"apiGroups": [""], "resources": ["secrets"], "resourceNames": ["lanekeeper-agent-cert"], "verbs": ["get", "update"]}]

agent_workload_role(ns) := {
	"apiVersion": "rbac.authorization.k8s.io/v1",
	"kind": "Role",
	"metadata": {"name": "lanekeeper-agent-workloads", "namespace": ns, "labels": agent_pod_labels},
	"rules": agent_workload_rules,
}

agent_binding(role_name, ns) := {
	"apiVersion": "rbac.authorization.k8s.io/v1",
	"kind": "RoleBinding",
	"metadata": {"name": role_name, "namespace": ns, "labels": agent_pod_labels},
	"roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": role_name},
	"subjects": [{"kind": "ServiceAccount", "name": agent_name, "namespace": namespace}],
}

agent_secrets_role := {
	"apiVersion": "rbac.authorization.k8s.io/v1",
	"kind": "Role",
	"metadata": {"name": "lanekeeper-agent-secrets", "namespace": namespace, "labels": agent_pod_labels},
	"rules": agent_secret_rules,
}

agent_networkpolicy := {
	"apiVersion": "networking.k8s.io/v1",
	"kind": "NetworkPolicy",
	"metadata": {"name": agent_name, "namespace": namespace, "labels": agent_pod_labels},
	"spec": {
		"podSelector": {"matchLabels": agent_selector},
		"policyTypes": ["Ingress", "Egress"],
		"ingress": [{
			"from": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "monitoring"}}}],
			"ports": [{"protocol": "TCP", "port": 9090}],
		}],
		"egress": [
			{"to": [{"ipBlock": {"cidr": "203.0.113.10/32"}}], "ports": [{"protocol": "TCP", "port": 443}]},
			{"to": [{"ipBlock": {"cidr": "198.51.100.2/32"}}], "ports": [{"protocol": "TCP", "port": 443}]},
			{"to": [{"podSelector": {"matchLabels": {"app": "csp-configuration-server"}}}], "ports": [{"protocol": "TCP", "port": 8888}]},
			{"to": [{"ipBlock": {"cidr": "169.254.169.254/32"}}], "ports": [{"protocol": "TCP", "port": 80}]},
			{
				"to": [{
					"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}},
					"podSelector": {"matchLabels": {"k8s-app": "kube-dns"}},
				}],
				"ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}],
			},
		],
	},
}

agent_docs := [
	agent_serviceaccount,
	agent_deployment,
	agent_workload_role("sit1"),
	agent_binding("lanekeeper-agent-workloads", "sit1"),
	agent_secrets_role,
	agent_binding("lanekeeper-agent-secrets", namespace),
	agent_cert_secret,
	agent_hub_ca,
	agent_storageclass,
	agent_spool_pvc,
	agent_networkpolicy,
]

# The input of a conftest run over the agent chart's output.
agent := [{"path": "rendered/agent.yaml", "contents": agent_docs}]

# The agent input with one document changed by a JSON patch. A patch whose path is not in the document is undefined, and the test
# that asked for it fails: that is how a test that no longer matches the chart shows itself.
agent_patch(kind, doc_name, ops) := [{"path": "rendered/agent.yaml", "contents": changed(agent_docs, kind, doc_name, ops)}]

# The agent input with these documents.
agent_with(docs) := [{"path": "rendered/agent.yaml", "contents": docs}]

# The agent input without the documents of one kind.
agent_without(kind) := [{"path": "rendered/agent.yaml", "contents": [d | some d in agent_docs; d.kind != kind]}]

# The agent input with one more document.
agent_with_doc(doc) := [{"path": "rendered/agent.yaml", "contents": array.concat(agent_docs, [doc])}]

agent_pod_op(op, field, value) := {"op": op, "path": concat("", ["/spec/template/spec/", field]), "value": value}

agent_container_op(op, field, value) := {"op": op, "path": concat("", ["/spec/template/spec/containers/0/", field]), "value": value}

agent_set(path, value) := agent_patch("Deployment", agent_name, [{"op": "replace", "path": path, "value": value}])

agent_remove(path) := agent_patch("Deployment", agent_name, [{"op": "remove", "path": path}])

# The contract files, as contract-extract.sh writes them: crates/agent/RBAC.md has one block of four documents, and
# crates/agent/HARDENING.md has the Pod and the NetworkPolicy.
agent_contract_rbac_docs := [
	{
		"apiVersion": "rbac.authorization.k8s.io/v1",
		"kind": "Role",
		"metadata": {"name": "lanekeeper-agent-workloads", "namespace": "sit1"},
		"rules": array.concat(agent_workload_rules, []),
	},
	{
		"apiVersion": "rbac.authorization.k8s.io/v1",
		"kind": "RoleBinding",
		"metadata": {"name": "lanekeeper-agent-workloads", "namespace": "sit1"},
		"roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "lanekeeper-agent-workloads"},
		"subjects": [{"kind": "ServiceAccount", "name": agent_name, "namespace": "lanekeeper"}],
	},
	{
		"apiVersion": "rbac.authorization.k8s.io/v1",
		"kind": "Role",
		"metadata": {"name": "lanekeeper-agent-secrets", "namespace": "lanekeeper"},
		"rules": [
			{"apiGroups": [""], "resources": ["secrets"], "resourceNames": ["lanekeeper-agent-cert"], "verbs": ["get", "update"]},
			{"apiGroups": [""], "resources": ["secrets"], "resourceNames": ["lanekeeper-agent-join-token"], "verbs": ["get"]},
		],
	},
	{
		"apiVersion": "rbac.authorization.k8s.io/v1",
		"kind": "RoleBinding",
		"metadata": {"name": "lanekeeper-agent-secrets", "namespace": "lanekeeper"},
		"roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "lanekeeper-agent-secrets"},
		"subjects": [{"kind": "ServiceAccount", "name": agent_name, "namespace": "lanekeeper"}],
	},
]

agent_contract_pod := {
	"apiVersion": "v1",
	"kind": "Pod",
	"metadata": {"name": agent_name, "namespace": "lanekeeper", "labels": {"app.kubernetes.io/name": "lanekeeper-agent"}},
	"spec": {
		"serviceAccountName": agent_name,
		"automountServiceAccountToken": true,
		"hostNetwork": false,
		"hostPID": false,
		"hostIPC": false,
		"securityContext": {
			"runAsNonRoot": true,
			"runAsUser": 1010,
			"runAsGroup": 1010,
			"fsGroup": 1010,
			"seccompProfile": {"type": "RuntimeDefault"},
		},
		"containers": [{
			"name": "agent",
			"image": "ghcr.io/<org>/lanekeeper-agent@sha256:<digest>",
			"securityContext": {
				"allowPrivilegeEscalation": false,
				"privileged": false,
				"readOnlyRootFilesystem": true,
				"capabilities": {"drop": ["ALL"]},
			},
			"resources": {"requests": {"cpu": "25m", "memory": "64Mi"}, "limits": {"cpu": "1", "memory": "128Mi"}},
			"ports": [{"name": "ops", "containerPort": 9090, "protocol": "TCP"}],
			"livenessProbe": {"httpGet": {"path": "/healthz", "port": "ops"}, "periodSeconds": 20, "timeoutSeconds": 3, "failureThreshold": 3},
			"readinessProbe": {"httpGet": {"path": "/readyz", "port": "ops"}, "periodSeconds": 10, "timeoutSeconds": 3},
			"env": [
				{"name": "LK_HUB_CA_FILE", "value": "/etc/lanekeeper/hub-ca/ca.pem"},
				{"name": "LK_NFS_ROOT", "value": "/mnt/csp-configuration"},
				{"name": "LK_SPOOL_DIR", "value": "/var/lib/lanekeeper/spool"},
				{"name": "LK_TMP_DIR", "value": "/tmp"},
			],
			"volumeMounts": [
				{"name": "nfs", "mountPath": "/mnt/csp-configuration", "readOnly": false},
				{"name": "tmp", "mountPath": "/tmp"},
				{"name": "spool", "mountPath": "/var/lib/lanekeeper/spool"},
				{"name": "hub-ca", "readOnly": true, "mountPath": "/etc/lanekeeper/hub-ca"},
			],
		}],
		"volumes": [
			{"name": "nfs", "nfs": {"server": "<nfs server>", "path": "<export path>", "readOnly": false}},
			{"name": "tmp", "emptyDir": {"sizeLimit": "64Mi"}},
			{"name": "spool", "persistentVolumeClaim": {"claimName": "lanekeeper-agent-spool"}},
			{"name": "hub-ca", "configMap": {"name": "lanekeeper-hub-ca"}},
		],
	},
}

agent_contract_networkpolicy := {
	"apiVersion": "networking.k8s.io/v1",
	"kind": "NetworkPolicy",
	"metadata": {"name": agent_name, "namespace": "lanekeeper"},
	"spec": agent_networkpolicy.spec,
}

agent_contract := [
	{"path": "contract/agent-rbac-contract.yaml", "contents": agent_contract_rbac_docs},
	{"path": "contract/agent-hardening-contract.yaml", "contents": [agent_contract_pod, agent_contract_networkpolicy]},
]

# A conftest run over the agent chart's output and the contract files.
agent_with_contract := array.concat(agent, agent_contract)

# The same, with one rendered document changed.
agent_contract_patch(kind, doc_name, ops) := array.concat(agent_patch(kind, doc_name, ops), agent_contract)

# The same, with the rendered documents replaced.
agent_contract_with(docs) := array.concat([{"path": "rendered/agent.yaml", "contents": docs}], agent_contract)

# The same, with one document of one contract file changed (a contract that no longer says what the chart renders). File 0 is
# RBAC.md's, file 1 is HARDENING.md's.
agent_contract_changed(file_index, kind, doc_name, ops) := array.concat(agent, [f |
	some i, file in agent_contract
	f := contract_file_after(i, file, file_index, kind, doc_name, ops)
])

contract_file_after(i, file, file_index, kind, doc_name, ops) := {"path": file.path, "contents": changed(file.contents, kind, doc_name, ops)} if i == file_index

contract_file_after(i, file, file_index, _, _, _) := file if i != file_index
