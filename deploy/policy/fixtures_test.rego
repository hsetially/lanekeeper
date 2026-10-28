# The documents the policy tests start from (09/T3). `hub` is what `helm template` makes of the hub chart with the kind values,
# written out as conftest's `--combine` input (see lib.rego): one file of documents, and perf/budgets.toml. Every test takes a
# copy, changes one thing with `patch` (a JSON patch on one document), `without` (drops a document) or `with_doc` (adds one),
# and shows that the rule that guards that thing refuses the copy. The documents use documentation addresses only
# (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32), never a real address.
#
# `deploy/tests/render.sh` runs the real chart through the same policies, so a document here that stops resembling the chart's
# output does not hide a gap: the real output is checked too. Keep them alike; when the chart changes shape, change this.
package fixtures

import rego.v1

name := "lanekeeper-hub"

namespace := "lanekeeper"

digest := "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

selector := {"app.kubernetes.io/name": "lanekeeper-hub"}

pod_labels := {
	"app.kubernetes.io/name": "lanekeeper-hub",
	"app.kubernetes.io/instance": "lanekeeper",
	"app.kubernetes.io/part-of": "lanekeeper",
}

hardened_container := {
	"name": "hub",
	"image": concat("", ["ghcr.io/hsetially/lanekeeper/hub@", digest]),
	"imagePullPolicy": "IfNotPresent",
	"securityContext": {
		"allowPrivilegeEscalation": false,
		"privileged": false,
		"readOnlyRootFilesystem": true,
		"capabilities": {"drop": ["ALL"]},
	},
	"resources": {
		"requests": {"cpu": "500m", "memory": "1536Mi"},
		"limits": {"cpu": "4", "memory": "2Gi"},
	},
	"ports": [
		{"name": "web", "containerPort": 8443, "protocol": "TCP"},
		{"name": "agent", "containerPort": 8444, "protocol": "TCP"},
		{"name": "replica", "containerPort": 8445, "protocol": "TCP"},
		{"name": "ops", "containerPort": 9090, "protocol": "TCP"},
	],
	"startupProbe": {
		"httpGet": {"path": "/healthz", "port": "ops"},
		"periodSeconds": 2,
		"timeoutSeconds": 2,
		"failureThreshold": 30,
	},
	"livenessProbe": {
		"httpGet": {"path": "/healthz", "port": "ops"},
		"periodSeconds": 10,
		"timeoutSeconds": 3,
		"failureThreshold": 3,
	},
	"readinessProbe": {
		"httpGet": {"path": "/readyz", "port": "ops"},
		"periodSeconds": 5,
		"timeoutSeconds": 3,
	},
	"env": [
		{"name": "LK_WEB_ADDR", "value": "0.0.0.0:8443"},
		{"name": "LK_OPS_ADDR", "value": "0.0.0.0:9090"},
		{"name": "LK_AGENT_ADDR", "value": "0.0.0.0:8444"},
		{"name": "LK_REPLICA_ADDR", "value": "0.0.0.0:8445"},
		{"name": "LK_WEB_DIST", "value": "/app/web/dist"},
		{"name": "LK_TRUSTED_PROXY_HOPS", "value": "2"},
		{"name": "LK_DATABASE_URL_FILE", "value": "/etc/lanekeeper/db/uri"},
		{"name": "LK_DB_CA_FILE", "value": "/etc/lanekeeper/db-ca/ca.crt"},
		{"name": "LK_MIRROR_DIR", "value": "/var/lib/lanekeeper/mirror"},
		{"name": "LK_GCP_PROJECT", "value": "example-project"},
		{"name": "LK_WEB_CERT_REF", "value": "lanekeeper-web-cert"},
		{"name": "LK_WEB_KEY_REF", "value": "lanekeeper-web-key"},
		# The one reference the contract allows: this pod's own name. It comes before the address that is built from it.
		{"name": "LK_REPLICA_NAME", "valueFrom": {"fieldRef": {"fieldPath": "metadata.name"}}},
		{"name": "LK_REPLICA_ADDRESS", "value": "$(LK_REPLICA_NAME).lanekeeper-hub-replicas.lanekeeper.svc:8445"},
		{"name": "LK_PUBLIC_ORIGIN", "value": "https://lanekeeper.example.com"},
		{"name": "LK_HUB_AUDIENCE", "value": "https://lanekeeper.example.com"},
		{"name": "LK_AGENT_ENDPOINT_HOSTS", "value": "agents.lanekeeper.example.com"},
		{"name": "LK_AGENT_CA_KMS_REF", "value": "projects/example-project/locations/example-location/keyRings/lanekeeper/cryptoKeys/agent-ca/cryptoKeyVersions/1"},
		{"name": "LK_AGENT_CA_CERT_REF", "value": "lanekeeper-agent-ca-cert"},
		{"name": "LK_REPLICA_CA_KMS_REF", "value": "projects/example-project/locations/example-location/keyRings/lanekeeper/cryptoKeys/replica-ca/cryptoKeyVersions/1"},
		{"name": "LK_REPLICA_CA_CERT_REF", "value": "lanekeeper-replica-ca-cert"},
		{"name": "LK_GIT_BASE_URL", "value": "https://github.com/example-org/configuration-base-saas.git"},
		{"name": "LK_GIT_TENANT_URL", "value": "https://github.com/example-org/csp-tenant-data.git"},
		{"name": "LK_GIT_BASE_REPO", "value": "example-org/configuration-base-saas"},
		{"name": "LK_GIT_TENANT_REPO", "value": "example-org/csp-tenant-data"},
		{"name": "LK_ENTRA_TENANT_ID", "value": "11111111-1111-4111-8111-111111111111"},
		{"name": "LK_ENTRA_CLIENT_ID", "value": "22222222-2222-4222-8222-222222222222"},
		{"name": "LK_ENTRA_CLIENT_AUTH_REF", "value": "lanekeeper-entra-client"},
		{"name": "LK_ENTRA_COOKIE_KEY_REF", "value": "lanekeeper-login-cookie"},
		{"name": "LK_AUDIT_CHECKPOINT_BUCKET", "value": "example-project-lanekeeper-audit"},
		{"name": "LK_AUDIT_CHECKPOINT_KMS_REF", "value": "projects/example-project/locations/example-location/keyRings/lanekeeper/cryptoKeys/audit-checkpoint/cryptoKeyVersions/1"},
		{"name": "LK_VAULT_KMS_REF", "value": "projects/example-project/locations/example-location/keyRings/lanekeeper/cryptoKeys/vault"},
	],
	"volumeMounts": [
		{"name": "tmp", "mountPath": "/tmp"},
		{"name": "mirror", "mountPath": "/var/lib/lanekeeper/mirror"},
		{"name": "db-credentials", "mountPath": "/etc/lanekeeper/db", "readOnly": true},
		{"name": "db-ca", "mountPath": "/etc/lanekeeper/db-ca", "readOnly": true},
	],
}

statefulset := {
	"apiVersion": "apps/v1",
	"kind": "StatefulSet",
	"metadata": {"name": name, "namespace": namespace, "labels": pod_labels},
	"spec": {
		"replicas": 3,
		"serviceName": "lanekeeper-hub-replicas",
		"podManagementPolicy": "Parallel",
		"persistentVolumeClaimRetentionPolicy": {"whenDeleted": "Retain", "whenScaled": "Retain"},
		"selector": {"matchLabels": {
			"app.kubernetes.io/name": "lanekeeper-hub",
			"app.kubernetes.io/instance": "lanekeeper",
		}},
		"template": {
			"metadata": {"labels": pod_labels},
			"spec": {
				"serviceAccountName": "lanekeeper-hub",
				"automountServiceAccountToken": false,
				"enableServiceLinks": false,
				"terminationGracePeriodSeconds": 45,
				"securityContext": {
					"runAsNonRoot": true,
					"runAsUser": 65532,
					"runAsGroup": 65532,
					"fsGroup": 65532,
					"fsGroupChangePolicy": "OnRootMismatch",
					"seccompProfile": {"type": "RuntimeDefault"},
				},
				"affinity": {"podAntiAffinity": {"requiredDuringSchedulingIgnoredDuringExecution": [{
					"labelSelector": {"matchLabels": selector},
					"topologyKey": "topology.kubernetes.io/zone",
				}]}},
				"containers": [hardened_container],
				"volumes": [
					{"name": "tmp", "emptyDir": {"sizeLimit": "256Mi"}},
					{"name": "db-credentials", "secret": {
						"secretName": "lanekeeper-db-app",
						"defaultMode": 288,
						"items": [{"key": "uri", "path": "uri"}],
					}},
					{"name": "db-ca", "secret": {
						"secretName": "lanekeeper-db-ca",
						"items": [{"key": "ca.crt", "path": "ca.crt"}],
					}},
				],
			},
		},
		"volumeClaimTemplates": [{
			"metadata": {"name": "mirror", "labels": pod_labels},
			"spec": {
				"accessModes": ["ReadWriteOnce"],
				"storageClassName": "lanekeeper-hub-cmek",
				"resources": {"requests": {"storage": "50Gi"}},
			},
		}],
	},
}

storageclass := {
	"apiVersion": "storage.k8s.io/v1",
	"kind": "StorageClass",
	"metadata": {"name": "lanekeeper-hub-cmek", "labels": pod_labels},
	"provisioner": "pd.csi.storage.gke.io",
	"parameters": {
		"type": "pd-balanced",
		"disk-encryption-kms-key": "projects/example-project/locations/europe-west2/keyRings/example/cryptoKeys/disk",
	},
	"reclaimPolicy": "Delete",
	"volumeBindingMode": "WaitForFirstConsumer",
	"allowVolumeExpansion": true,
}

serviceaccount := {
	"apiVersion": "v1",
	"kind": "ServiceAccount",
	"metadata": {
		"name": "lanekeeper-hub",
		"namespace": namespace,
		"labels": pod_labels,
		"annotations": {"iam.gke.io/gcp-service-account": "lanekeeper-hub@example-project.iam.gserviceaccount.com"},
	},
	"automountServiceAccountToken": false,
}

service_headless := {
	"apiVersion": "v1",
	"kind": "Service",
	"metadata": {"name": "lanekeeper-hub-replicas", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"clusterIP": "None",
		"selector": selector,
		"ports": [{"name": "replica", "port": 8445, "targetPort": "replica", "protocol": "TCP"}],
	},
}

service_web := {
	"apiVersion": "v1",
	"kind": "Service",
	"metadata": {"name": "lanekeeper-hub-web", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"type": "ClusterIP",
		"selector": selector,
		"ports": [{"name": "web", "port": 443, "targetPort": "web", "protocol": "TCP", "appProtocol": "HTTPS"}],
	},
}

service_agent := {
	"apiVersion": "v1",
	"kind": "Service",
	"metadata": {"name": "lanekeeper-hub-agent", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"type": "LoadBalancer",
		"externalTrafficPolicy": "Local",
		"loadBalancerSourceRanges": ["198.51.100.0/24"],
		"selector": selector,
		"ports": [{"name": "agent", "port": 443, "targetPort": "agent", "protocol": "TCP"}],
	},
}

pdb := {
	"apiVersion": "policy/v1",
	"kind": "PodDisruptionBudget",
	"metadata": {"name": "lanekeeper-hub", "namespace": namespace, "labels": pod_labels},
	"spec": {"minAvailable": 2, "selector": {"matchLabels": selector}},
}

gateway := {
	"apiVersion": "gateway.networking.k8s.io/v1",
	"kind": "Gateway",
	"metadata": {"name": "lanekeeper-hub", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"gatewayClassName": "gke-l7-global-external-managed",
		"addresses": [{"type": "NamedAddress", "value": "lanekeeper-hub-ip"}],
		"listeners": [{
			"name": "https",
			"protocol": "HTTPS",
			"port": 443,
			"hostname": "lanekeeper.example.com",
			"tls": {"mode": "Terminate", "options": {"networking.gke.io/pre-shared-certs": "lanekeeper-hub-cert"}},
			"allowedRoutes": {"namespaces": {"from": "Same"}, "kinds": [{"kind": "HTTPRoute"}]},
		}],
	},
}

httproute := {
	"apiVersion": "gateway.networking.k8s.io/v1",
	"kind": "HTTPRoute",
	"metadata": {"name": "lanekeeper-hub", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"parentRefs": [{"name": "lanekeeper-hub"}],
		"hostnames": ["lanekeeper.example.com"],
		"rules": [
			{
				"matches": [{"path": {"type": "Exact", "value": "/hooks/github"}}],
				"backendRefs": [{"name": "lanekeeper-hub-web", "port": 443}],
			},
			{
				"matches": [{"path": {"type": "PathPrefix", "value": "/"}}],
				"backendRefs": [{"name": "lanekeeper-hub-web", "port": 443}],
			},
		],
	},
}

backendpolicy := {
	"apiVersion": "networking.gke.io/v1",
	"kind": "GCPBackendPolicy",
	"metadata": {"name": "lanekeeper-hub-web", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"default": {"securityPolicy": "lanekeeper-hub", "timeoutSec": 3600, "logging": {"enabled": true}},
		"targetRef": {"group": "", "kind": "Service", "name": "lanekeeper-hub-web"},
	},
}

healthcheckpolicy := {
	"apiVersion": "networking.gke.io/v1",
	"kind": "HealthCheckPolicy",
	"metadata": {"name": "lanekeeper-hub-web", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"default": {
			"checkIntervalSec": 5,
			"timeoutSec": 3,
			"healthyThreshold": 1,
			"unhealthyThreshold": 3,
			"config": {"type": "HTTP", "httpHealthCheck": {"portSpecification": "USE_FIXED_PORT", "port": 9090, "requestPath": "/readyz"}},
		},
		"targetRef": {"group": "", "kind": "Service", "name": "lanekeeper-hub-web"},
	},
}

# What the user-facing load balancer's proxies and health checks come from (A10).
google_ranges := ["35.191.0.0/16", "130.211.0.0/22"]

networkpolicy := {
	"apiVersion": "networking.k8s.io/v1",
	"kind": "NetworkPolicy",
	"metadata": {"name": "lanekeeper-hub", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"podSelector": {"matchLabels": selector},
		"policyTypes": ["Ingress", "Egress"],
		"ingress": [
			{"from": [{"ipBlock": {"cidr": "35.191.0.0/16"}}, {"ipBlock": {"cidr": "130.211.0.0/22"}}], "ports": [{"protocol": "TCP", "port": 8443}]},
			{"from": [{"ipBlock": {"cidr": "198.51.100.0/24"}}], "ports": [{"protocol": "TCP", "port": 8444}]},
			{"from": [{"podSelector": {"matchLabels": selector}}], "ports": [{"protocol": "TCP", "port": 8445}]},
			{"from": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "monitoring"}}}], "ports": [{"protocol": "TCP", "port": 9090}]},
			{"from": [{"ipBlock": {"cidr": "35.191.0.0/16"}}, {"ipBlock": {"cidr": "130.211.0.0/22"}}], "ports": [{"protocol": "TCP", "port": 9090}]},
		],
		"egress": [
			{
				"to": [{
					"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}},
					"podSelector": {"matchLabels": {"k8s-app": "kube-dns"}},
				}],
				"ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}],
			},
			{"to": [{"podSelector": {"matchLabels": {"cnpg.io/cluster": "lanekeeper-db"}}}], "ports": [{"protocol": "TCP", "port": 5432}]},
			{"to": [{"podSelector": {"matchLabels": selector}}], "ports": [{"protocol": "TCP", "port": 8445}]},
			{"to": [{"ipBlock": {"cidr": "169.254.169.254/32"}}], "ports": [{"protocol": "TCP", "port": 80}]},
		],
	},
}

fqdnpolicy := {
	"apiVersion": "networking.gke.io/v1alpha1",
	"kind": "FQDNNetworkPolicy",
	"metadata": {"name": "lanekeeper-hub-external", "namespace": namespace, "labels": pod_labels},
	"spec": {
		"podSelector": {"matchLabels": selector},
		"egress": [{
			"matches": [
				{"pattern": "login.microsoftonline.com"},
				{"pattern": "github.com"},
				{"pattern": "api.github.com"},
				{"pattern": "secretmanager.googleapis.com"},
				{"pattern": "cloudkms.googleapis.com"},
				{"pattern": "www.googleapis.com"},
				{"pattern": "storage.googleapis.com"},
			],
			"ports": [{"protocol": "TCP", "port": 443}],
		}],
	},
}

# The Cloud Armor policy of the gcp chart.
armor_rule_users := {
	"action": "allow",
	"priority": 1000,
	"description": "Users: GlobalProtect egress ranges (Q2)",
	"match": {"versionedExpr": "SRC_IPS_V1", "config": {"srcIpRanges": ["192.0.2.0/24"]}},
}

armor_rule_hooks := {
	"action": "allow",
	"priority": 2000,
	"description": "GitHub webhooks, only /hooks/github (Q28)",
	"match": {"expr": {"expression": "request.path == '/hooks/github' && (inIpRange(origin.ip, '203.0.113.0/24') || inIpRange(origin.ip, '2001:db8::/32'))"}},
}

armor_rule_default := {
	"action": "deny(403)",
	"priority": 2147483647,
	"description": "Default rule: deny",
	"match": {"versionedExpr": "SRC_IPS_V1", "config": {"srcIpRanges": ["*"]}},
}

securitypolicy := {
	"apiVersion": "compute.cnrm.cloud.google.com/v1beta1",
	"kind": "ComputeSecurityPolicy",
	"metadata": {"name": "lanekeeper-hub", "namespace": "lanekeeper-gcp", "labels": pod_labels},
	"spec": {
		"description": "lanekeeper hub",
		"rule": [armor_rule_users, armor_rule_hooks, armor_rule_default],
	},
}

# GitHub's hook ranges as the generator writes them (documentation addresses here).
github_hook_ranges := ["203.0.113.0/24", "2001:db8::/32"]

budgets := {
	"path": "perf/budgets.toml",
	"contents": {"budget": [
		{"id": "P11", "bench": "hub/startup", "threshold": 10000, "unit": "ms"},
		{"id": "P11.memory_mib", "bench": "hub/memory_at_design_scale_mib", "threshold": 1536, "unit": "mib"},
	]},
}

hub_docs := [
	statefulset,
	storageclass,
	serviceaccount,
	service_headless,
	service_web,
	service_agent,
	pdb,
	gateway,
	httproute,
	backendpolicy,
	healthcheckpolicy,
	networkpolicy,
	fqdnpolicy,
]

# The input of a conftest run over the hub chart's output.
hub := [{"path": "rendered/hub.yaml", "contents": hub_docs}, budgets]

# The input of a conftest run over the gcp chart's output.
gcp := [{"path": "rendered/gcp.yaml", "contents": [securitypolicy]}]

# The documents of `docs` with one change.
changed(docs, kind, doc_name, ops) := [patched(d, kind, doc_name, ops) | some d in docs]

is_target(d, kind, doc_name) if {
	d.kind == kind
	d.metadata.name == doc_name
}

patched(d, kind, doc_name, ops) := json.patch(d, ops) if is_target(d, kind, doc_name)

patched(d, kind, doc_name, _) := d if not is_target(d, kind, doc_name)

# The hub input with one document changed by a JSON patch. If the path is not in the document, the patch is undefined and the
# test that asked for it fails, which is how a test that no longer matches the chart shows itself.
patch(kind, doc_name, ops) := [{"path": "rendered/hub.yaml", "contents": changed(hub_docs, kind, doc_name, ops)}, budgets]

# The hub input without the documents of one kind.
without(kind) := [{"path": "rendered/hub.yaml", "contents": [d | some d in hub_docs; d.kind != kind]}, budgets]

# The hub input with one more document.
with_doc(doc) := [{"path": "rendered/hub.yaml", "contents": array.concat(hub_docs, [doc])}, budgets]

# The hub input without the budgets file.
without_budgets := [{"path": "rendered/hub.yaml", "contents": hub_docs}]

# The gcp input with the Cloud Armor rules replaced.
gcp_with_rules(rules) := [{"path": "rendered/gcp.yaml", "contents": [json.patch(securitypolicy, [{"op": "replace", "path": "/spec/rule", "value": rules}])]}]

# The first container's field patch.
container_op(op, field, value) := {"op": op, "path": concat("", ["/spec/template/spec/containers/0/", field]), "value": value}

container_remove(field) := {"op": "remove", "path": concat("", ["/spec/template/spec/containers/0/", field])}

pod_op(op, field, value) := {"op": op, "path": concat("", ["/spec/template/spec/", field]), "value": value}

pod_remove(field) := {"op": "remove", "path": concat("", ["/spec/template/spec/", field])}

sts_remove(path) := patch("StatefulSet", "lanekeeper-hub", [{"op": "remove", "path": path}])

sts_set(path, value) := patch("StatefulSet", "lanekeeper-hub", [{"op": "replace", "path": path, "value": value}])
