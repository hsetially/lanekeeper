# The documents the Argo CD policy tests start from (09/T6). They are the manifests of deploy/argocd, written out as conftest's `--combine`
# input: `ac_platform` is the platform cluster's files (the projects and the three Applications), `ac_swimlane` is one swimlane's file
# with the values file of that swimlane (its namespaces are the ones the project must allow). A test takes a copy, changes one thing
# with `ac_platform_patch` (a JSON patch on one file) or `ac_platform_with` (a file replaced), and shows that the rule that guards that
# thing refuses the copy. `deploy/tests/render.sh` runs the real manifests through the same policy, so a document here that stops
# resembling them does not hide a gap: the real files are checked too. Keep them alike.
#
# The revision here is a commit id, not the placeholder of the real files (40 zeros), so that the good input has no warning.
package fixtures

import rego.v1

ac_repo := "https://github.com/hsetially/lanekeeper"

ac_revision := "1111111111111111111111111111111111111111"

ac_labels(extra) := object.union({"app.kubernetes.io/part-of": "lanekeeper"}, extra)

ac_app(name, project, chart, namespace, value_files, labels, prune) := {
	"apiVersion": "argoproj.io/v1alpha1",
	"kind": "Application",
	"metadata": {"name": name, "namespace": "argocd", "labels": ac_labels(labels)},
	"spec": {
		"project": project,
		"source": {
			"repoURL": ac_repo,
			"targetRevision": ac_revision,
			"path": concat("/", ["deploy/charts", chart]),
			"helm": {"releaseName": name, "valueFiles": value_files},
		},
		"destination": {"server": "https://kubernetes.default.svc", "namespace": namespace},
		"syncPolicy": {
			"automated": {"prune": prune, "selfHeal": true},
			"syncOptions": ["CreateNamespace=true"],
		},
	},
}

ac_hub := ac_app("lanekeeper-hub", "lanekeeper-workloads", "hub", "lanekeeper", ["../../values/hub/prod.yaml"], {"lanekeeper.io/environment": "prod"}, true)

ac_db := ac_app("lanekeeper-db", "lanekeeper-workloads", "db", "lanekeeper", ["../../values/db/prod.yaml"], {"lanekeeper.io/environment": "prod"}, false)

ac_gcp := ac_app("lanekeeper-gcp", "lanekeeper-cloud", "gcp", "lanekeeper-gcp", ["../../values/hub/github-hook-ranges.yaml", "../../values/gcp/prod.yaml"], {"lanekeeper.io/environment": "prod"}, false)

ac_workloads_kinds := [
	{"group": "", "kind": "ServiceAccount"},
	{"group": "", "kind": "Service"},
	{"group": "apps", "kind": "StatefulSet"},
	{"group": "policy", "kind": "PodDisruptionBudget"},
	{"group": "networking.k8s.io", "kind": "NetworkPolicy"},
	{"group": "networking.gke.io", "kind": "FQDNNetworkPolicy"},
	{"group": "networking.gke.io", "kind": "GCPBackendPolicy"},
	{"group": "networking.gke.io", "kind": "HealthCheckPolicy"},
	{"group": "gateway.networking.k8s.io", "kind": "Gateway"},
	{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute"},
	{"group": "postgresql.cnpg.io", "kind": "Cluster"},
	{"group": "postgresql.cnpg.io", "kind": "ScheduledBackup"},
]

ac_cloud_kinds := [
	{"group": "storage.cnrm.cloud.google.com", "kind": "StorageBucket"},
	{"group": "iam.cnrm.cloud.google.com", "kind": "IAMServiceAccount"},
	{"group": "iam.cnrm.cloud.google.com", "kind": "IAMPolicyMember"},
	{"group": "compute.cnrm.cloud.google.com", "kind": "ComputeSecurityPolicy"},
	{"group": "binaryauthorization.cnrm.cloud.google.com", "kind": "BinaryAuthorizationPolicy"},
	{"group": "binaryauthorization.cnrm.cloud.google.com", "kind": "BinaryAuthorizationAttestor"},
	{"group": "containeranalysis.cnrm.cloud.google.com", "kind": "ContainerAnalysisNote"},
]

ac_agent_kinds := [
	{"group": "", "kind": "ServiceAccount"},
	{"group": "", "kind": "Secret"},
	{"group": "", "kind": "ConfigMap"},
	{"group": "apps", "kind": "Deployment"},
	{"group": "rbac.authorization.k8s.io", "kind": "Role"},
	{"group": "rbac.authorization.k8s.io", "kind": "RoleBinding"},
	{"group": "networking.k8s.io", "kind": "NetworkPolicy"},
]

ac_project(name, labels, namespaces, cluster_kinds, kinds) := {
	"apiVersion": "argoproj.io/v1alpha1",
	"kind": "AppProject",
	"metadata": {"name": name, "namespace": "argocd", "labels": ac_labels(labels)},
	"spec": {
		"description": "x",
		"sourceRepos": [ac_repo],
		"destinations": [{"server": "https://kubernetes.default.svc", "namespace": ns} | some ns in namespaces],
		"clusterResourceWhitelist": cluster_kinds,
		"namespaceResourceWhitelist": kinds,
		"orphanedResources": {"warn": true},
	},
}

# Argo CD checks the Namespace that CreateNamespace=true makes against the project's cluster-scoped kinds, so each project allows the one
# Namespace its Applications create, by name.
ac_namespace_entry(name) := {"group": "", "kind": "Namespace", "name": name}

ac_proj_workloads := ac_project("lanekeeper-workloads", {}, ["lanekeeper"], [{"group": "storage.k8s.io", "kind": "StorageClass"}, ac_namespace_entry("lanekeeper")], ac_workloads_kinds)

ac_proj_cloud := ac_project("lanekeeper-cloud", {}, ["lanekeeper-gcp"], [ac_namespace_entry("lanekeeper-gcp")], ac_cloud_kinds)

ac_proj_agent := ac_project("lanekeeper-agent", {"lanekeeper.io/swimlane": "sit1"}, ["lanekeeper", "sit1"], [ac_namespace_entry("lanekeeper")], ac_agent_kinds)

ac_agent_app := object.union(
	ac_app("lanekeeper-agent", "lanekeeper-agent", "agent", "lanekeeper", ["../../values/agent/sit1.yaml"], {"lanekeeper.io/swimlane": "sit1"}, true),
	{"spec": {
		"syncPolicy": {
			"automated": {"prune": true, "selfHeal": true},
			"syncOptions": ["CreateNamespace=true", "RespectIgnoreDifferences=true"],
		},
		"ignoreDifferences": [{"kind": "Secret", "name": "lanekeeper-agent-cert", "jsonPointers": ["/data"]}],
	}},
)

# The files of the platform cluster, by path.
ac_platform_files := {
	"deploy/argocd/platform/projects.yaml": [ac_proj_workloads, ac_proj_cloud],
	"deploy/argocd/platform/hub.yaml": ac_hub,
	"deploy/argocd/platform/db.yaml": ac_db,
	"deploy/argocd/platform/gcp.yaml": ac_gcp,
}

# The swimlane's file, and its values file (the part of it the policy reads).
ac_swimlane_files := {
	"deploy/argocd/swimlanes/sit1.yaml": [ac_proj_agent, ac_agent_app],
	"deploy/values/agent/sit1.yaml": {"swimlane": {"id": "sit1"}, "namespaces": ["sit1"]},
}

ac_input(files) := [{"path": path, "contents": contents} | some path, contents in files]

ac_platform := ac_input(ac_platform_files)

ac_swimlane := ac_input(ac_swimlane_files)

# The files with one replaced. (Not object.union: it merges the new file into the old one, so a key that a test removes would come back.)
ac_replace(files, path, contents) := {p: c | some p, old in files; c := ac_pick(p, path, contents, old)}

ac_pick(p, path, contents, _) := contents if p == path

ac_pick(p, path, _, old) := old if p != path

# The platform input with one file replaced, or changed by a JSON patch.
ac_platform_with(path, contents) := ac_input(ac_replace(ac_platform_files, path, contents))

ac_platform_patch(path, ops) := ac_platform_with(path, json.patch(ac_platform_files[path], ops))

ac_swimlane_with(path, contents) := ac_input(ac_replace(ac_swimlane_files, path, contents))

ac_swimlane_patch(path, ops) := ac_swimlane_with(path, json.patch(ac_swimlane_files[path], ops))

# The swimlane's file with its project or its Application changed.
ac_agent_project_patch(ops) := ac_swimlane_with("deploy/argocd/swimlanes/sit1.yaml", [json.patch(ac_proj_agent, ops), ac_agent_app])

ac_agent_app_patch(ops) := ac_swimlane_with("deploy/argocd/swimlanes/sit1.yaml", [ac_proj_agent, json.patch(ac_agent_app, ops)])

# One of the platform's projects changed; the other stays.
ac_workloads_patch(ops) := ac_platform_with("deploy/argocd/platform/projects.yaml", [json.patch(ac_proj_workloads, ops), ac_proj_cloud])

ac_cloud_patch(ops) := ac_platform_with("deploy/argocd/platform/projects.yaml", [ac_proj_workloads, json.patch(ac_proj_cloud, ops)])
