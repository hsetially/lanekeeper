# Argo CD policy (09/T6; S16, S19, plan 09 A14, D26). Run with `conftest test --combine --namespace argocd` over the files of deploy/argocd that
# one Argo CD applies (the platform cluster's, or one swimlane's with its values file). An Application deploys one chart of this repository at
# one commit, with the values file of its environment or swimlane and nothing set inline, into the one namespace it belongs to, from a
# project that cannot deploy anything else. What is checked in the cluster is what `deploy/tests/render.sh` rendered and checked here.
package argocd

import rego.v1

import data.lk

repo_url := "https://github.com/hsetially/lanekeeper"

in_cluster := "https://kubernetes.default.svc"

placeholder_revision := "0000000000000000000000000000000000000000"

applications := [a |
	some a in lk.of_kind("Application")
	startswith(a.apiVersion, "argoproj.io/")
]

projects := [p |
	some p in lk.of_kind("AppProject")
	startswith(p.apiVersion, "argoproj.io/")
]

project_names := {p.metadata.name | some p in projects}

# Every Application, with the file it is in.
application_files := [{"path": f.path, "app": d} |
	some f in input
	some d in lk.documents(f)
	is_object(d)
	d.kind == "Application"
	startswith(d.apiVersion, "argoproj.io/")
]

# What each Application is. `prune` is false where a removal from Git must never delete what exists: a database, and the resources of a
# cloud project. The labels lanekeeper.io/environment (staging or prod) and lanekeeper.io/swimlane say which values file it is for.
components := {
	"lanekeeper-hub": {"chart": "hub", "project": "lanekeeper-workloads", "namespace": "lanekeeper", "prune": true},
	"lanekeeper-db": {"chart": "db", "project": "lanekeeper-workloads", "namespace": "lanekeeper", "prune": false},
	"lanekeeper-gcp": {"chart": "gcp", "project": "lanekeeper-cloud", "namespace": "lanekeeper-gcp", "prune": false},
	"lanekeeper-agent": {"chart": "agent", "project": "lanekeeper-agent", "namespace": "lanekeeper", "prune": true},
}

label(r, key) := object.get(r.metadata, ["labels", key], "")

environments := {"staging", "prod"}

swimlane_id := `^[a-z0-9][a-z0-9-]{0,62}$`

# --- which Application ------------------------------------------------------------------------------------------------------------

deny contains msg if {
	some a in applications
	not a.metadata.name in object.keys(components)
	msg := sprintf("%s: the name is not one of %v; an Application here deploys one of the charts of this repository", [lk.who(a), sort(object.keys(components))])
}

deny contains msg if {
	some a in applications
	object.get(a.metadata, "namespace", "") != "argocd"
	msg := sprintf("%s: metadata.namespace is %q; an Application lives in the namespace of its Argo CD, argocd", [lk.who(a), object.get(a.metadata, "namespace", "")])
}

deny contains msg if {
	some a in applications
	some finalizer in object.get(a.metadata, "finalizers", [])
	startswith(finalizer, "resources-finalizer.argocd.argoproj.io")
	msg := sprintf("%s: the finalizer %s deletes everything the Application made when the Application is deleted; deleting an Application must leave what runs, and removing a workload is a deliberate act", [lk.who(a), finalizer])
}

# --- where it comes from: this repository, at a commit -----------------------------------------------------------------------------

deny contains msg if {
	some a in applications
	object.get(object.get(a.spec, "source", {}), "repoURL", "") != repo_url
	msg := sprintf("%s: spec.source.repoURL must be %s", [lk.who(a), repo_url])
}

valid_revision(r) if {
	is_string(r)
	regex.match(`^[0-9a-f]{40}$`, r)
}

deny contains msg if {
	some a in applications
	rev := object.get(object.get(a.spec, "source", {}), "targetRevision", "")
	not valid_revision(rev)
	msg := sprintf("%s: spec.source.targetRevision is %v; it must be a full commit id (40 lower-case hex digits), because a branch moves under a running system and a tag can be moved (S19)", [lk.who(a), rev])
}

# A commit cannot name the commit that contains it, so the id of the merge commit is put here by the next change (the same as the image
# digests, plan 09 A29). Argo CD cannot resolve the placeholder, so nothing syncs until it is replaced.
warn contains msg if {
	some a in applications
	object.get(object.get(a.spec, "source", {}), "targetRevision", "") == placeholder_revision
	msg := sprintf("%s: spec.source.targetRevision is the placeholder (40 zeros); Argo CD cannot resolve it, so nothing syncs until the commit that holds these charts is put here (deploy/argocd/README.md, \"Moving a revision\")", [lk.who(a)])
}

deny contains msg if {
	some a in applications
	"chart" in object.keys(object.get(a.spec, "source", {}))
	msg := sprintf("%s: spec.source.chart is set; the chart is deployed from this repository by path, never from a Helm repository", [lk.who(a)])
}

source_keys := {"repoURL", "targetRevision", "path", "helm"}

deny contains msg if {
	some a in applications
	some key in object.keys(object.get(a.spec, "source", {}))
	not key in source_keys
	key != "chart"
	msg := sprintf("%s: spec.source.%s is set; an Application here is a chart of this repository at a commit, with a values file, and nothing else", [lk.who(a), key])
}

deny contains msg if {
	some a in applications
	some key in {"sources", "sourceHydrator"}
	key in object.keys(a.spec)
	msg := sprintf("%s: spec.%s is set; an Application here has one source", [lk.who(a), key])
}

# --- which chart, from which project, into which namespace ----------------------------------------------------------------------

deny contains msg if {
	some a in applications
	c := components[a.metadata.name]
	object.get(object.get(a.spec, "source", {}), "path", "") != sprintf("deploy/charts/%s", [c.chart])
	msg := sprintf("%s: spec.source.path is %q; it must be deploy/charts/%s", [lk.who(a), object.get(object.get(a.spec, "source", {}), "path", ""), c.chart])
}

deny contains msg if {
	some a in applications
	c := components[a.metadata.name]
	object.get(a.spec, "project", "") != c.project
	msg := sprintf("%s: spec.project is %q; it must be %s, a project that can deploy this chart and nothing else", [lk.who(a), object.get(a.spec, "project", ""), c.project])
}

deny contains msg if {
	some a in applications
	c := components[a.metadata.name]
	object.get(a.spec, "project", "") == c.project
	not c.project in project_names
	msg := sprintf("%s: spec.project %s is not in this input; the project is applied to the same Argo CD, with the file that holds it", [lk.who(a), c.project])
}

deny contains msg if {
	some a in applications
	c := components[a.metadata.name]
	object.get(object.get(a.spec, "destination", {}), "namespace", "") != c.namespace
	msg := sprintf("%s: spec.destination.namespace is %q; it must be %s", [lk.who(a), object.get(object.get(a.spec, "destination", {}), "namespace", ""), c.namespace])
}

deny contains msg if {
	some a in applications
	object.get(object.get(a.spec, "destination", {}), "server", "") != in_cluster
	msg := sprintf("%s: spec.destination.server must be %s; each cluster's own Argo CD deploys to its own cluster (D26)", [lk.who(a), in_cluster])
}

deny contains msg if {
	some a in applications
	"name" in object.keys(object.get(a.spec, "destination", {}))
	msg := sprintf("%s: spec.destination.name is set; the destination is the in-cluster server, by address", [lk.who(a)])
}

# --- the values: the file of the environment or swimlane, and nothing inline ---------------------------------------------------------

helm(a) := object.get(object.get(a.spec, "source", {}), "helm", {})

deny contains msg if {
	some a in applications
	object.get(helm(a), "releaseName", "") != a.metadata.name
	msg := sprintf("%s: spec.source.helm.releaseName must be %q, the name of the Application: the release name is in the labels, and deploy/tests/render.sh checks the chart under that name", [lk.who(a), a.metadata.name])
}

helm_keys := {"releaseName", "valueFiles"}

deny contains msg if {
	some a in applications
	some key in object.keys(helm(a))
	not key in helm_keys
	msg := sprintf("%s: spec.source.helm.%s is set; what is deployed is the chart with the values file and nothing else, so that what deploy/tests/render.sh checks is what runs", [lk.who(a), key])
}

# The values files, relative to the chart, in the order that deploy/tests/render.sh layers them. The ranges that Cloud Armor allows
# for GitHub's webhooks come first, as generated output, so that the environment's values win.
value_files(a) := [sprintf("../../values/%s/%s.yaml", [c.chart, env])] if {
	c := components[a.metadata.name]
	c.chart in {"hub", "db"}
	env := label(a, "lanekeeper.io/environment")
	env in environments
}

value_files(a) := ["../../values/hub/github-hook-ranges.yaml", sprintf("../../values/gcp/%s.yaml", [env])] if {
	components[a.metadata.name].chart == "gcp"
	env := label(a, "lanekeeper.io/environment")
	env in environments
}

value_files(a) := [sprintf("../../values/agent/%s.yaml", [id])] if {
	components[a.metadata.name].chart == "agent"
	id := label(a, "lanekeeper.io/swimlane")
	regex.match(swimlane_id, id)
}

deny contains msg if {
	some a in applications
	want := value_files(a)
	got := object.get(helm(a), "valueFiles", [])
	got != want
	msg := sprintf("%s: spec.source.helm.valueFiles is %v; it must be exactly %v", [lk.who(a), got, want])
}

deny contains msg if {
	some a in applications
	components[a.metadata.name].chart in {"hub", "db", "gcp"}
	not label(a, "lanekeeper.io/environment") in environments
	msg := sprintf("%s: the label lanekeeper.io/environment is %q; it must be one of %v, and it names the values file", [lk.who(a), label(a, "lanekeeper.io/environment"), sort(environments)])
}

deny contains msg if {
	some a in applications
	components[a.metadata.name].chart == "agent"
	not regex.match(swimlane_id, label(a, "lanekeeper.io/swimlane"))
	msg := sprintf("%s: the label lanekeeper.io/swimlane is %q; it must be the swimlane's id, and it names the values file", [lk.who(a), label(a, "lanekeeper.io/swimlane")])
}

deny contains msg if {
	some x in application_files
	components[x.app.metadata.name].chart == "agent"
	id := label(x.app, "lanekeeper.io/swimlane")
	regex.match(swimlane_id, id)
	not endswith(x.path, sprintf("/swimlanes/%s.yaml", [id]))
	msg := sprintf("%s is in %s; the file of a swimlane is deploy/argocd/swimlanes/<swimlane>.yaml, named after its swimlane (%s)", [lk.who(x.app), x.path, id])
}

# --- how it syncs ----------------------------------------------------------------------------------------------------------------

automated(a) := object.get(object.get(a.spec, "syncPolicy", {}), "automated", {})

deny contains msg if {
	some a in applications
	not is_object(object.get(object.get(a.spec, "syncPolicy", {}), "automated", null))
	msg := sprintf("%s: spec.syncPolicy.automated is missing; what is in Git is what runs, and a drifted cluster is put back (D26)", [lk.who(a)])
}

deny contains msg if {
	some a in applications
	is_object(object.get(object.get(a.spec, "syncPolicy", {}), "automated", null))
	object.get(automated(a), "selfHeal", false) != true
	msg := sprintf("%s: spec.syncPolicy.automated.selfHeal must be true: a change made in the cluster by hand is reverted", [lk.who(a)])
}

deny contains msg if {
	some a in applications
	c := components[a.metadata.name]
	is_object(object.get(object.get(a.spec, "syncPolicy", {}), "automated", null))
	object.get(automated(a), "prune", false) != c.prune
	msg := sprintf("%s: spec.syncPolicy.automated.prune must be %v: %s", [lk.who(a), c.prune, prune_reason(c.prune)])
}

prune_reason(true) := "a resource that leaves Git must leave the cluster, or a removed rule stays in force"

prune_reason(false) := "what this Application makes is never deleted because a file left Git; removing it is a person's act"

deny contains msg if {
	some a in applications
	object.get(automated(a), "allowEmpty", false) == true
	msg := sprintf("%s: spec.syncPolicy.automated.allowEmpty is true; an empty render (a broken template) would be synced and everything pruned", [lk.who(a)])
}

sync_options := {"CreateNamespace=true", "RespectIgnoreDifferences=true", "ServerSideApply=true"}

deny contains msg if {
	some a in applications
	some option in object.get(object.get(a.spec, "syncPolicy", {}), "syncOptions", [])
	not option in sync_options
	msg := sprintf("%s: syncOptions has %q; the options used here are %v (Replace and Force delete and recreate, and the dry run is never skipped)", [lk.who(a), option, sort(sync_options)])
}

# --- the agent's certificate Secret (plan 09 A14) ------------------------------------------------------------------------------------
# The agent writes its certificate into the Secret the chart renders empty. With self-heal on, Argo CD would put the empty Secret back at
# the next sync, so it ignores the Secret's data and does not write it either.

agent_ignored := [{"group": "", "kind": "Secret", "name": "lanekeeper-agent-cert", "jsonPointers": ["/data"]}]

normalized(e) := object.union({"group": ""}, e)

deny contains msg if {
	some a in applications
	components[a.metadata.name].chart == "agent"
	got := [normalized(e) | some e in object.get(a.spec, "ignoreDifferences", [])]
	got != agent_ignored
	msg := sprintf("%s: spec.ignoreDifferences must be exactly the data of the Secret lanekeeper-agent-cert, %v, and is %v: self-heal would empty the agent's certificate (plan 09 A14)", [lk.who(a), agent_ignored, got])
}

deny contains msg if {
	some a in applications
	components[a.metadata.name].chart != "agent"
	count(object.get(a.spec, "ignoreDifferences", [])) > 0
	msg := sprintf("%s: spec.ignoreDifferences is set; only the agent's certificate Secret is ignored, and everything else in Git is what runs", [lk.who(a)])
}

deny contains msg if {
	some a in applications
	count(object.get(a.spec, "ignoreDifferences", [])) > 0
	not "RespectIgnoreDifferences=true" in object.get(object.get(a.spec, "syncPolicy", {}), "syncOptions", [])
	msg := sprintf("%s: ignoreDifferences without RespectIgnoreDifferences=true; the sync would write the ignored field anyway (plan 09 A14)", [lk.who(a)])
}

# --- the projects --------------------------------------------------------------------------------------------------------------------

# What each project may deploy (the approved list: a new kind is a decision, so it goes here with a test), as group/kind. deploy/tests/
# render.sh renders every chart and shows that the project allows everything the chart makes (argocd_scope.rego).
approved_namespaced := {
	"lanekeeper-workloads": {
		"/ServiceAccount", "/Service", "apps/StatefulSet", "policy/PodDisruptionBudget", "networking.k8s.io/NetworkPolicy",
		"networking.gke.io/FQDNNetworkPolicy", "networking.gke.io/GCPBackendPolicy", "networking.gke.io/HealthCheckPolicy",
		"gateway.networking.k8s.io/Gateway", "gateway.networking.k8s.io/HTTPRoute",
		"postgresql.cnpg.io/Cluster", "postgresql.cnpg.io/ScheduledBackup",
		# The hub image's Jobs (`hub migrate`, a hook; `hub seed`) and the ConfigMap of the seed file (09/fix, J5): batch/Job and nothing else of
		# batch (no CronJob), and ConfigMaps, which hold no secret (secrets.rego refuses a private key in one) and are not Secrets.
		"batch/Job", "/ConfigMap",
	},
	"lanekeeper-cloud": {
		"storage.cnrm.cloud.google.com/StorageBucket", "iam.cnrm.cloud.google.com/IAMServiceAccount",
		"iam.cnrm.cloud.google.com/IAMPolicyMember", "compute.cnrm.cloud.google.com/ComputeSecurityPolicy",
		"binaryauthorization.cnrm.cloud.google.com/BinaryAuthorizationPolicy",
		"binaryauthorization.cnrm.cloud.google.com/BinaryAuthorizationAttestor",
		"containeranalysis.cnrm.cloud.google.com/ContainerAnalysisNote",
		# The hub's keys and the secret containers it reads (09/fix, I1): the key ring, the keys, the secrets. No KMSCryptoKeyVersion,
		# KMSImportJob or SecretManagerSecretVersion: a version is where a value is, and the values are put in by a person.
		"kms.cnrm.cloud.google.com/KMSKeyRing", "kms.cnrm.cloud.google.com/KMSCryptoKey",
		"secretmanager.cnrm.cloud.google.com/SecretManagerSecret",
	},
	"lanekeeper-agent": {
		"/ServiceAccount", "/Secret", "/ConfigMap", "apps/Deployment", "rbac.authorization.k8s.io/Role",
		"rbac.authorization.k8s.io/RoleBinding", "networking.k8s.io/NetworkPolicy", "/PersistentVolumeClaim",
	},
}

# Cluster-scoped kinds, as group/kind/name. CreateNamespace=true makes the Application's namespace as part of the sync, and Argo CD checks that
# Namespace against the project like any resource (gitops-engine, sync_context.go: the namespace task goes through the permission validator),
# so each project allows the Namespace of its own, by name, and no other cluster-scoped kind but the StorageClass of the charts that make one.
# The agent's spool (09/T8) is on a StorageClass with a customer-managed key, which the agent chart makes: that one, by name.
approved_cluster := {
	"lanekeeper-workloads": {"storage.k8s.io/StorageClass/", "/Namespace/lanekeeper"},
	"lanekeeper-cloud": {"/Namespace/lanekeeper-gcp"},
	"lanekeeper-agent": {"/Namespace/lanekeeper", "storage.k8s.io/StorageClass/lanekeeper-agent-cmek"},
}

approved_namespaces := {
	"lanekeeper-workloads": {"lanekeeper"},
	"lanekeeper-cloud": {"lanekeeper-gcp"},
}

# A namespace that no project of ours deploys to: the system's, Argo CD's own (a project that could deploy there could rewrite its own
# restrictions), and the default.
forbidden_namespaces := {"kube-system", "kube-public", "kube-node-lease", "argocd", "default"}

deny contains msg if {
	some p in projects
	not p.metadata.name in object.keys(approved_namespaced)
	msg := sprintf("%s: the name is not one of %v", [lk.who(p), sort(object.keys(approved_namespaced))])
}

deny contains msg if {
	some p in projects
	object.get(p.metadata, "namespace", "") != "argocd"
	msg := sprintf("%s: metadata.namespace is %q; a project lives in the namespace of its Argo CD, argocd", [lk.who(p), object.get(p.metadata, "namespace", "")])
}

deny contains msg if {
	some p in projects
	object.get(p.spec, "sourceRepos", []) != [repo_url]
	msg := sprintf("%s: spec.sourceRepos must be exactly [%s]: a project that reads from any repository can deploy what anyone pushed", [lk.who(p), repo_url])
}

deny contains msg if {
	some p in projects
	some field in {"roles", "sourceNamespaces"}
	count(object.get(p.spec, field, [])) > 0
	msg := sprintf("%s: spec.%s is set; the project has no tokens, and only Applications in the Argo CD namespace use it", [lk.who(p), field])
}

# Destinations: the in-cluster server, by address, and namespaces that are named.
deny contains msg if {
	some p in projects
	some d in object.get(p.spec, "destinations", [])
	object.get(d, "server", "") != in_cluster
	msg := sprintf("%s: spec.destinations has the server %q; it must be %s, by address (a cluster's own Argo CD deploys to its own cluster)", [lk.who(p), object.get(d, "server", ""), in_cluster])
}

deny contains msg if {
	some p in projects
	some d in object.get(p.spec, "destinations", [])
	"name" in object.keys(d)
	msg := sprintf("%s: spec.destinations has a destination by cluster name (%q); destinations are by address", [lk.who(p), d.name])
}

deny contains msg if {
	some p in projects
	some d in object.get(p.spec, "destinations", [])
	ns := object.get(d, "namespace", "")
	contains(ns, "*")
	msg := sprintf("%s: spec.destinations has the namespace pattern %q; namespaces are named", [lk.who(p), ns])
}

deny contains msg if {
	some p in projects
	some d in object.get(p.spec, "destinations", [])
	ns := object.get(d, "namespace", "")
	ns in forbidden_namespaces
	msg := sprintf("%s: the project must not deploy to the namespace %q (%v are the system's, Argo CD's own and the default)", [lk.who(p), ns, sort(forbidden_namespaces)])
}

destination_namespaces(p) := {d.namespace | some d in object.get(p.spec, "destinations", [])}

deny contains msg if {
	some p in projects
	want := approved_namespaces[p.metadata.name]
	got := destination_namespaces(p)
	got != want
	msg := sprintf("%s: spec.destinations have the namespaces %v; they must be exactly %v", [lk.who(p), sort(got), sort(want)])
}

# The agent's project may deploy to the agent's namespace and to the namespaces the agent watches, which are in the swimlane's values file.
agent_values(p) := [f |
	some f in input
	endswith(f.path, sprintf("values/agent/%s.yaml", [label(p, "lanekeeper.io/swimlane")]))
]

deny contains msg if {
	some p in projects
	p.metadata.name == "lanekeeper-agent"
	not regex.match(swimlane_id, label(p, "lanekeeper.io/swimlane"))
	msg := sprintf("%s: the label lanekeeper.io/swimlane is %q; it must be the swimlane's id, and it names the values file whose namespaces the project allows", [lk.who(p), label(p, "lanekeeper.io/swimlane")])
}

deny contains msg if {
	some p in projects
	p.metadata.name == "lanekeeper-agent"
	id := label(p, "lanekeeper.io/swimlane")
	regex.match(swimlane_id, id)
	count(agent_values(p)) == 0
	msg := sprintf("%s: deploy/values/agent/%s.yaml is not in this input, so the namespaces that the agent watches cannot be compared with the project's destinations", [lk.who(p), id])
}

deny contains msg if {
	some p in projects
	p.metadata.name == "lanekeeper-agent"
	id := label(p, "lanekeeper.io/swimlane")
	some f in agent_values(p)
	watched := {n | some n in object.get(f.contents, "namespaces", [])}
	want := watched | {"lanekeeper"}
	got := destination_namespaces(p)
	got != want
	msg := sprintf("%s: spec.destinations have the namespaces %v, and the agent's namespace plus the namespaces of deploy/values/agent/%s.yaml are %v", [lk.who(p), sort(got), id, sort(want)])
}

deny contains msg if {
	some p in projects
	p.metadata.name == "lanekeeper-agent"
	not "lanekeeper" in destination_namespaces(p)
	msg := sprintf("%s: spec.destinations do not include the agent's namespace, lanekeeper", [lk.who(p)])
}

# Kinds. A missing namespaced whitelist means "every kind" to Argo CD, and an empty one means none (pkg/apis/application/v1alpha1,
# IsGroupKindNamePermitted), so both are refused; a wildcard is not in the approved list.
kinds(list) := {sprintf("%s/%s", [object.get(x, "group", ""), object.get(x, "kind", "")]) | some x in list}

cluster_kinds(list) := {sprintf("%s/%s/%s", [object.get(x, "group", ""), object.get(x, "kind", ""), object.get(x, "name", "")]) | some x in list}

# The Namespace that CreateNamespace=true makes must be allowed by the project, by its exact name, or the first sync stops at "resource
# :Namespace is not permitted in project".
deny contains msg if {
	some a in applications
	"CreateNamespace=true" in object.get(object.get(a.spec, "syncPolicy", {}), "syncOptions", [])
	ns := object.get(object.get(a.spec, "destination", {}), "namespace", "")
	some p in projects
	p.metadata.name == object.get(a.spec, "project", "")
	not namespace_allowed(p, ns)
	msg := sprintf("%s: syncOptions has CreateNamespace=true, and the project %s does not allow the Namespace %q in spec.clusterResourceWhitelist (group \"\", kind Namespace, name %s), so the sync would stop at the namespace", [lk.who(a), p.metadata.name, ns, ns])
}

namespace_allowed(p, ns) if {
	some w in object.get(p.spec, "clusterResourceWhitelist", [])
	object.get(w, "group", "") == ""
	object.get(w, "kind", "") == "Namespace"
	object.get(w, "name", "") == ns
}

deny contains msg if {
	some p in projects
	count(object.get(p.spec, "namespaceResourceWhitelist", [])) == 0
	msg := sprintf("%s: spec.namespaceResourceWhitelist is missing or empty, which Argo CD reads as every namespaced kind; list the kinds the chart makes", [lk.who(p)])
}

deny contains msg if {
	some p in projects
	want := approved_namespaced[p.metadata.name]
	got := kinds(object.get(p.spec, "namespaceResourceWhitelist", []))
	count(got) > 0
	got != want
	msg := sprintf("%s: spec.namespaceResourceWhitelist differs from the approved list: extra %v, missing %v", [lk.who(p), sort(got - want), sort(want - got)])
}

deny contains msg if {
	some p in projects
	want := approved_cluster[p.metadata.name]
	got := cluster_kinds(object.get(p.spec, "clusterResourceWhitelist", []))
	got != want
	msg := sprintf("%s: spec.clusterResourceWhitelist differs from the approved list: extra %v, missing %v; a project that deploys a cluster-scoped kind can grant itself anything", [lk.who(p), sort(got - want), sort(want - got)])
}
