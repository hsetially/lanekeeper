# Tests for argocd.rego (09/T6; S16, S19, plan 09 A14): each Application deploys one chart of this repository at one commit, with the
# values file of its environment or swimlane and nothing set inline, into the one namespace it belongs to, from a project that cannot
# deploy anything else. Run with `conftest verify --policy deploy/policy`.
package argocd

import rego.v1

import data.fixtures

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

warned(planted, fragment) if {
	msgs := warn with input as planted
	some msg in msgs
	contains(msg, fragment)
}

hub := "deploy/argocd/platform/hub.yaml"

db := "deploy/argocd/platform/db.yaml"

gcp := "deploy/argocd/platform/gcp.yaml"

lane := "deploy/argocd/swimlanes/sit1.yaml"

test_good_platform_passes if {
	count(deny) == 0 with input as fixtures.ac_platform
	count(warn) == 0 with input as fixtures.ac_platform
}

test_good_swimlane_passes if {
	count(deny) == 0 with input as fixtures.ac_swimlane
	count(warn) == 0 with input as fixtures.ac_swimlane
}

test_other_charts_are_not_argocd if {
	count(deny) == 0 with input as fixtures.hub
	count(deny) == 0 with input as fixtures.agent
}

# --- the revision: a commit, and it says when it is a placeholder

test_placeholder_revision_is_allowed_with_a_warning if {
	planted := fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/targetRevision", "value": "0000000000000000000000000000000000000000"}])
	count(deny) == 0 with input as planted
	warned(planted, "placeholder")
}

test_placeholder_revision_of_a_swimlane_warns if {
	planted := fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/source/targetRevision", "value": "0000000000000000000000000000000000000000"}])
	warned(planted, "placeholder")
}

test_deny_branch_as_revision if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/targetRevision", "value": "main"}]), "targetRevision")
}

test_deny_head_as_revision if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/targetRevision", "value": "HEAD"}]), "targetRevision")
}

test_deny_tag_as_revision if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/targetRevision", "value": "v1.2.3"}]), "targetRevision")
}

test_deny_short_commit_as_revision if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/targetRevision", "value": "1111111"}]), "targetRevision")
}

test_deny_upper_case_commit_as_revision if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/targetRevision", "value": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}]), "targetRevision")
}

test_deny_range_as_revision if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/targetRevision", "value": "1.*"}]), "targetRevision")
}

test_deny_no_revision if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "remove", "path": "/spec/source/targetRevision"}]), "targetRevision")
}

test_deny_branch_as_revision_of_a_swimlane if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/source/targetRevision", "value": "main"}]), "targetRevision")
}

# --- where it comes from

test_deny_other_repository if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/repoURL", "value": "https://github.com/someone/lanekeeper"}]), "repoURL")
}

test_deny_repository_over_http if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/repoURL", "value": "http://github.com/hsetially/lanekeeper"}]), "repoURL")
}

test_deny_repository_with_a_suffix if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/repoURL", "value": "https://github.com/hsetially/lanekeeper.git"}]), "repoURL")
}

test_deny_helm_repository_as_the_source if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/chart", "value": "hub"}]), "Helm repository")
}

test_deny_several_sources if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/sources", "value": [{"repoURL": "https://example.com/x", "path": "y"}]}]), "spec.sources")
}

test_deny_hydrated_source if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/sourceHydrator", "value": {}}]), "sourceHydrator")
}

test_deny_a_plugin if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/plugin", "value": {"name": "x"}}]), "spec.source.plugin")
}

test_deny_kustomize if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/kustomize", "value": {}}]), "spec.source.kustomize")
}

test_deny_a_directory_source if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/directory", "value": {"recurse": true}}]), "spec.source.directory")
}

test_deny_a_ref if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/ref", "value": "values"}]), "spec.source.ref")
}

# --- which chart, from which project, into which namespace

test_deny_unknown_application if {
	planted := fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/metadata/name", "value": "lanekeeper-other"}])
	refused(planted, "is not one of")
}

test_deny_application_outside_the_argocd_namespace if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/metadata/namespace", "value": "default"}]), "metadata.namespace")
}

test_deny_default_project if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/project", "value": "default"}]), "spec.project")
}

test_deny_the_hub_in_the_cloud_project if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/project", "value": "lanekeeper-cloud"}]), "spec.project")
}

test_deny_the_cloud_in_the_workloads_project if {
	refused(fixtures.ac_platform_patch(gcp, [{"op": "replace", "path": "/spec/project", "value": "lanekeeper-workloads"}]), "spec.project")
}

test_deny_an_application_without_its_project_in_the_input if {
	files := {p: c | some p, c in fixtures.ac_platform_files; p != "deploy/argocd/platform/projects.yaml"}
	refused(fixtures.ac_input(files), "is not in this input")
}

test_deny_a_project_that_is_not_in_the_input if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/project", "value": "somewhere-else"}]), "spec.project")
}

test_deny_the_hub_with_another_chart if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/path", "value": "deploy/charts/db"}]), "spec.source.path")
}

test_deny_a_path_outside_the_charts if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/path", "value": "deploy"}]), "spec.source.path")
}

test_deny_a_path_that_climbs if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/path", "value": "deploy/charts/hub/../../.."}]), "spec.source.path")
}

test_deny_the_hub_in_another_namespace if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/destination/namespace", "value": "kube-system"}]), "spec.destination.namespace")
}

test_deny_the_cloud_resources_in_the_hubs_namespace if {
	refused(fixtures.ac_platform_patch(gcp, [{"op": "replace", "path": "/spec/destination/namespace", "value": "lanekeeper"}]), "spec.destination.namespace")
}

test_deny_another_cluster if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/destination/server", "value": "https://203.0.113.5"}]), "spec.destination.server")
}

test_deny_a_cluster_by_name if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/destination/name", "value": "other"}]), "spec.destination.name")
}

test_deny_the_agent_in_another_namespace if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/destination/namespace", "value": "sit1"}]), "spec.destination.namespace")
}

# --- the values: the file of the environment, and nothing inline

test_deny_staging_values_for_prod if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["../../values/hub/staging.yaml"]}]), "valueFiles")
}

test_deny_the_values_of_another_chart if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["../../values/db/prod.yaml"]}]), "valueFiles")
}

test_deny_values_from_outside_the_repository if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["../../../../etc/passwd"]}]), "valueFiles")
}

test_deny_values_from_a_url if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["https://example.com/values.yaml"]}]), "valueFiles")
}

test_deny_no_values_file if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": []}]), "valueFiles")
}

test_deny_another_values_file_after_the_right_one if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/valueFiles/-", "value": "../../values/hub/kind.yaml"}]), "valueFiles")
}

test_deny_the_cloud_without_the_hook_ranges if {
	refused(fixtures.ac_platform_patch(gcp, [{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["../../values/gcp/prod.yaml"]}]), "valueFiles")
}

test_deny_the_cloud_with_the_ranges_after_the_values if {
	refused(fixtures.ac_platform_patch(gcp, [{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["../../values/gcp/prod.yaml", "../../values/hub/github-hook-ranges.yaml"]}]), "valueFiles")
}

test_deny_the_agent_with_another_swimlanes_values if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["../../values/agent/sit2.yaml"]}]), "valueFiles")
}

test_deny_the_agent_with_values_of_another_chart if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["../../values/hub/prod.yaml"]}]), "valueFiles")
}

test_deny_another_release_name if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/source/helm/releaseName", "value": "lanekeeper"}]), "releaseName")
}

test_deny_no_release_name if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "remove", "path": "/spec/source/helm/releaseName"}]), "releaseName")
}

test_deny_inline_values if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/values", "value": "replicas: 1"}]), "spec.source.helm.values")
}

test_deny_inline_values_object if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/valuesObject", "value": {"replicas": 1}}]), "spec.source.helm.valuesObject")
}

test_deny_inline_parameters if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/parameters", "value": [{"name": "image.digest", "value": "sha256:x"}]}]), "spec.source.helm.parameters")
}

test_deny_file_parameters if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/fileParameters", "value": [{"name": "x", "path": "y"}]}]), "spec.source.helm.fileParameters")
}

test_deny_ignoring_a_missing_values_file if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/ignoreMissingValueFiles", "value": true}]), "spec.source.helm.ignoreMissingValueFiles")
}

test_deny_skipping_crds if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/skipCrds", "value": true}]), "spec.source.helm.skipCrds")
}

test_deny_passing_credentials if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/passCredentials", "value": true}]), "spec.source.helm.passCredentials")
}

test_deny_skipping_schema_validation if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/skipSchemaValidation", "value": true}]), "spec.source.helm.skipSchemaValidation")
}

test_deny_another_kubernetes_version if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/kubeVersion", "value": "1.20"}]), "spec.source.helm.kubeVersion")
}

test_deny_another_helm_version if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/source/helm/version", "value": "v2"}]), "spec.source.helm.version")
}

# --- the labels say which environment or swimlane the files are for

test_deny_no_environment_label if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "remove", "path": "/metadata/labels/lanekeeper.io~1environment"}]), "lanekeeper.io/environment")
}

test_deny_an_environment_that_is_not_one if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1environment", "value": "dev"}]), "lanekeeper.io/environment")
}

test_a_staging_application_with_staging_values_passes if {
	planted := fixtures.ac_platform_patch(hub, [
		{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1environment", "value": "staging"},
		{"op": "replace", "path": "/spec/source/helm/valueFiles", "value": ["../../values/hub/staging.yaml"]},
	])
	count(deny) == 0 with input as planted
}

test_deny_no_swimlane_label if {
	refused(fixtures.ac_agent_app_patch([{"op": "remove", "path": "/metadata/labels/lanekeeper.io~1swimlane"}]), "lanekeeper.io/swimlane")
}

test_deny_a_swimlane_label_that_is_not_an_id if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1swimlane", "value": "Sit 1"}]), "lanekeeper.io/swimlane")
}

test_deny_a_file_that_is_not_named_after_its_swimlane if {
	planted := fixtures.ac_input({
		"deploy/argocd/swimlanes/sit2.yaml": [fixtures.ac_proj_agent, fixtures.ac_agent_app],
		"deploy/values/agent/sit1.yaml": {"swimlane": {"id": "sit1"}, "namespaces": ["sit1"]},
	})
	refused(planted, "named after its swimlane")
}

# --- how it syncs

test_deny_manual_sync if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "remove", "path": "/spec/syncPolicy/automated"}]), "syncPolicy.automated")
}

test_deny_no_sync_policy if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "remove", "path": "/spec/syncPolicy"}]), "syncPolicy.automated")
}

test_deny_no_self_heal if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/syncPolicy/automated/selfHeal", "value": false}]), "selfHeal")
}

test_deny_no_self_heal_missing if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "remove", "path": "/spec/syncPolicy/automated/selfHeal"}]), "selfHeal")
}

test_deny_the_hub_without_prune if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "replace", "path": "/spec/syncPolicy/automated/prune", "value": false}]), "prune")
}

test_deny_the_database_with_prune if {
	refused(fixtures.ac_platform_patch(db, [{"op": "replace", "path": "/spec/syncPolicy/automated/prune", "value": true}]), "prune")
}

test_deny_the_cloud_resources_with_prune if {
	refused(fixtures.ac_platform_patch(gcp, [{"op": "replace", "path": "/spec/syncPolicy/automated/prune", "value": true}]), "prune")
}

test_deny_the_agent_without_prune if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/syncPolicy/automated/prune", "value": false}]), "prune")
}

test_deny_syncing_an_empty_render if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/syncPolicy/automated/allowEmpty", "value": true}]), "allowEmpty")
}

test_deny_replace if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/syncPolicy/syncOptions/-", "value": "Replace=true"}]), "syncOptions")
}

test_deny_force if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/syncPolicy/syncOptions/-", "value": "Force=true"}]), "syncOptions")
}

test_deny_skipping_the_dry_run if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/syncPolicy/syncOptions/-", "value": "SkipDryRunOnMissingResource=true"}]), "syncOptions")
}

test_deny_pruning_last_is_not_an_option_we_use if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/syncPolicy/syncOptions/-", "value": "Prune=false"}]), "syncOptions")
}

test_server_side_apply_is_allowed if {
	count(deny) == 0 with input as fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/syncPolicy/syncOptions/-", "value": "ServerSideApply=true"}])
}

test_deny_deleting_the_workloads_with_the_application if {
	refused(fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/metadata/finalizers", "value": ["resources-finalizer.argocd.argoproj.io"]}]), "resources-finalizer")
}

test_deny_deleting_the_database_with_the_application if {
	refused(fixtures.ac_platform_patch(db, [{"op": "add", "path": "/metadata/finalizers", "value": ["resources-finalizer.argocd.argoproj.io/foreground"]}]), "resources-finalizer")
}

test_deny_deleting_the_agent_with_the_application if {
	refused(fixtures.ac_agent_app_patch([{"op": "add", "path": "/metadata/finalizers", "value": ["resources-finalizer.argocd.argoproj.io"]}]), "resources-finalizer")
}

# --- the agent's certificate Secret (plan 09 A14)

test_deny_the_agent_without_ignored_differences if {
	refused(fixtures.ac_agent_app_patch([{"op": "remove", "path": "/spec/ignoreDifferences"}]), "ignoreDifferences")
}

test_deny_ignoring_the_wrong_secret if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/ignoreDifferences/0/name", "value": "lanekeeper-agent-join-token"}]), "ignoreDifferences")
}

test_deny_ignoring_more_than_the_data if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/ignoreDifferences/0/jsonPointers", "value": ["/data", "/metadata"]}]), "ignoreDifferences")
}

test_deny_ignoring_the_whole_secret if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/ignoreDifferences/0/jsonPointers", "value": ["/"]}]), "ignoreDifferences")
}

test_deny_ignoring_by_expression if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/ignoreDifferences/0", "value": {"kind": "Secret", "name": "lanekeeper-agent-cert", "jqPathExpressions": [".data"]}}]), "ignoreDifferences")
}

test_deny_ignoring_every_secret if {
	refused(fixtures.ac_agent_app_patch([{"op": "remove", "path": "/spec/ignoreDifferences/0/name"}]), "ignoreDifferences")
}

test_deny_ignoring_a_second_thing if {
	refused(fixtures.ac_agent_app_patch([{"op": "add", "path": "/spec/ignoreDifferences/-", "value": {"group": "apps", "kind": "Deployment", "jsonPointers": ["/spec"]}}]), "ignoreDifferences")
}

test_deny_ignoring_without_respecting if {
	refused(fixtures.ac_agent_app_patch([{"op": "replace", "path": "/spec/syncPolicy/syncOptions", "value": ["CreateNamespace=true"]}]), "RespectIgnoreDifferences")
}

test_deny_ignoring_differences_on_the_hub if {
	planted := fixtures.ac_platform_patch(hub, [{"op": "add", "path": "/spec/ignoreDifferences", "value": [{"group": "apps", "kind": "StatefulSet", "jsonPointers": ["/spec"]}]}])
	refused(planted, "only the agent's certificate Secret is ignored")
}

# --- the projects

test_deny_a_project_named_default if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/metadata/name", "value": "default"}]), "is not one of")
}

test_deny_a_project_outside_the_argocd_namespace if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/metadata/namespace", "value": "default"}]), "metadata.namespace")
}

test_deny_any_repository if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/spec/sourceRepos", "value": ["*"]}]), "sourceRepos")
}

test_deny_a_second_repository if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/sourceRepos/-", "value": "https://github.com/someone/else"}]), "sourceRepos")
}

test_deny_no_repository if {
	refused(fixtures.ac_workloads_patch([{"op": "remove", "path": "/spec/sourceRepos"}]), "sourceRepos")
}

test_deny_every_namespace if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/spec/destinations/0/namespace", "value": "*"}]), "namespace pattern")
}

test_deny_a_namespace_pattern if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/spec/destinations/0/namespace", "value": "lanekeeper-*"}]), "namespace pattern")
}

test_deny_every_server if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/spec/destinations/0/server", "value": "*"}]), "destinations")
}

test_deny_a_cluster_by_name_in_a_project if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/destinations/0/name", "value": "other"}]), "destinations")
}

test_deny_a_second_server if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/destinations/-", "value": {"server": "https://203.0.113.5", "namespace": "lanekeeper"}}]), "destinations")
}

test_deny_a_project_that_deploys_to_kube_system if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/destinations/-", "value": {"server": "https://kubernetes.default.svc", "namespace": "kube-system"}}]), "must not deploy to")
}

test_deny_a_project_that_deploys_to_argocd if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/destinations/-", "value": {"server": "https://kubernetes.default.svc", "namespace": "argocd"}}]), "must not deploy to")
}

test_deny_a_project_that_deploys_to_default if {
	refused(fixtures.ac_cloud_patch([{"op": "add", "path": "/spec/destinations/-", "value": {"server": "https://kubernetes.default.svc", "namespace": "default"}}]), "must not deploy to")
}

test_deny_workloads_in_the_clouds_namespace if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/destinations/-", "value": {"server": "https://kubernetes.default.svc", "namespace": "lanekeeper-gcp"}}]), "destinations")
}

test_deny_the_cloud_project_with_a_cluster_kind if {
	refused(fixtures.ac_cloud_patch([{"op": "add", "path": "/spec/clusterResourceWhitelist/-", "value": {"group": "rbac.authorization.k8s.io", "kind": "ClusterRole"}}]), "clusterResourceWhitelist")
}

test_deny_every_cluster_kind if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/spec/clusterResourceWhitelist", "value": [{"group": "*", "kind": "*"}]}]), "clusterResourceWhitelist")
}

test_deny_a_cluster_role_in_the_workloads_project if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/clusterResourceWhitelist/-", "value": {"group": "rbac.authorization.k8s.io", "kind": "ClusterRole"}}]), "clusterResourceWhitelist")
}

test_deny_no_cluster_whitelist_in_the_workloads_project if {
	refused(fixtures.ac_workloads_patch([{"op": "remove", "path": "/spec/clusterResourceWhitelist"}]), "clusterResourceWhitelist")
}

test_deny_no_cluster_whitelist_in_the_cloud_project if {
	refused(fixtures.ac_cloud_patch([{"op": "remove", "path": "/spec/clusterResourceWhitelist"}]), "clusterResourceWhitelist")
}

test_deny_every_namespace_in_the_cluster_whitelist if {
	refused(fixtures.ac_cloud_patch([{"op": "replace", "path": "/spec/clusterResourceWhitelist/0/name", "value": "*"}]), "clusterResourceWhitelist")
}

test_deny_a_namespace_pattern_in_the_cluster_whitelist if {
	refused(fixtures.ac_cloud_patch([{"op": "replace", "path": "/spec/clusterResourceWhitelist/0/name", "value": "lanekeeper-*"}]), "clusterResourceWhitelist")
}

test_deny_a_namespace_without_a_name_in_the_cluster_whitelist if {
	refused(fixtures.ac_cloud_patch([{"op": "remove", "path": "/spec/clusterResourceWhitelist/0/name"}]), "clusterResourceWhitelist")
}

test_deny_another_namespace_in_the_cluster_whitelist if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/clusterResourceWhitelist/-", "value": {"group": "", "kind": "Namespace", "name": "kube-system"}}]), "clusterResourceWhitelist")
}

test_deny_creating_a_namespace_the_project_does_not_allow if {
	refused(fixtures.ac_cloud_patch([{"op": "replace", "path": "/spec/clusterResourceWhitelist/0/name", "value": "lanekeeper-other"}]), "CreateNamespace=true")
}

test_deny_creating_a_namespace_without_any_cluster_kind if {
	refused(fixtures.ac_cloud_patch([{"op": "remove", "path": "/spec/clusterResourceWhitelist"}]), "CreateNamespace=true")
}

test_deny_creating_the_agents_namespace_without_its_project_allowing_it if {
	refused(fixtures.ac_agent_project_patch([{"op": "replace", "path": "/spec/clusterResourceWhitelist", "value": []}]), "CreateNamespace=true")
}

test_no_namespace_entry_is_needed_when_the_namespace_is_not_created if {
	project := json.patch(fixtures.ac_proj_cloud, [{"op": "remove", "path": "/spec/clusterResourceWhitelist/0"}])
	app := json.patch(fixtures.ac_gcp, [{"op": "replace", "path": "/spec/syncPolicy/syncOptions", "value": []}])
	files := fixtures.ac_replace(fixtures.ac_replace(fixtures.ac_platform_files, "deploy/argocd/platform/projects.yaml", [fixtures.ac_proj_workloads, project]), gcp, app)
	not refused(fixtures.ac_input(files), "CreateNamespace=true")
}

test_deny_no_namespaced_whitelist if {
	refused(fixtures.ac_workloads_patch([{"op": "remove", "path": "/spec/namespaceResourceWhitelist"}]), "namespaceResourceWhitelist")
}

test_deny_an_empty_namespaced_whitelist if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/spec/namespaceResourceWhitelist", "value": []}]), "namespaceResourceWhitelist")
}

test_deny_every_namespaced_kind if {
	refused(fixtures.ac_workloads_patch([{"op": "replace", "path": "/spec/namespaceResourceWhitelist", "value": [{"group": "*", "kind": "*"}]}]), "namespaceResourceWhitelist")
}

test_deny_every_kind_of_a_group if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "apps", "kind": "*"}}]), "namespaceResourceWhitelist")
}

test_deny_secrets_in_the_workloads_project if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "", "kind": "Secret"}}]), "namespaceResourceWhitelist")
}

test_deny_roles_in_the_workloads_project if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "rbac.authorization.k8s.io", "kind": "RoleBinding"}}]), "namespaceResourceWhitelist")
}

test_deny_cloud_resources_in_the_workloads_project if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "iam.cnrm.cloud.google.com", "kind": "IAMPolicyMember"}}]), "namespaceResourceWhitelist")
}

test_deny_workloads_in_the_cloud_project if {
	refused(fixtures.ac_cloud_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "apps", "kind": "Deployment"}}]), "namespaceResourceWhitelist")
}

test_deny_a_project_that_cannot_deploy_what_its_chart_makes if {
	refused(fixtures.ac_workloads_patch([{"op": "remove", "path": "/spec/namespaceResourceWhitelist/0"}]), "namespaceResourceWhitelist")
}

# The hub image's Jobs and the ConfigMap of the seed file (09/fix, J5): `batch/Job` and `ConfigMap`, and nothing wider.
test_deny_the_workloads_project_without_jobs if {
	refused(fixtures.ac_workloads_patch([{"op": "remove", "path": "/spec/namespaceResourceWhitelist/12"}]), "missing [\"batch/Job\"]")
}

test_deny_the_workloads_project_without_config_maps if {
	refused(fixtures.ac_workloads_patch([{"op": "remove", "path": "/spec/namespaceResourceWhitelist/13"}]), "missing [\"/ConfigMap\"]")
}

test_deny_the_workloads_project_with_cron_jobs if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "batch", "kind": "CronJob"}}]), "extra [\"batch/CronJob\"]")
}

test_deny_the_workloads_project_with_every_kind_of_batch if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "batch", "kind": "*"}}]), "namespaceResourceWhitelist")
}

test_deny_the_cloud_project_with_jobs if {
	refused(fixtures.ac_cloud_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "batch", "kind": "Job"}}]), "extra [\"batch/Job\"]")
}

test_deny_project_roles if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/roles", "value": [{"name": "ci", "policies": ["p, proj:x:ci, applications, sync, x/*, allow"]}]}]), "roles")
}

test_deny_applications_in_other_namespaces if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/sourceNamespaces", "value": ["*"]}]), "sourceNamespaces")
}

# --- the swimlane's project

test_deny_an_agent_project_for_another_swimlane_namespace if {
	refused(fixtures.ac_agent_project_patch([{"op": "add", "path": "/spec/destinations/-", "value": {"server": "https://kubernetes.default.svc", "namespace": "sit2"}}]), "namespaces of deploy/values/agent/sit1.yaml")
}

test_deny_an_agent_project_without_a_watched_namespace if {
	refused(fixtures.ac_agent_project_patch([{"op": "remove", "path": "/spec/destinations/1"}]), "namespaces of deploy/values/agent/sit1.yaml")
}

test_deny_an_agent_project_without_the_agents_namespace if {
	refused(fixtures.ac_agent_project_patch([{"op": "remove", "path": "/spec/destinations/0"}]), "do not include the agent's namespace")
}

test_deny_an_agent_project_without_a_swimlane_label if {
	refused(fixtures.ac_agent_project_patch([{"op": "remove", "path": "/metadata/labels/lanekeeper.io~1swimlane"}]), "names the values file whose namespaces")
}

test_deny_an_agent_project_with_a_swimlane_label_that_is_not_an_id if {
	refused(fixtures.ac_agent_project_patch([{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1swimlane", "value": "Sit 1"}]), "names the values file whose namespaces")
}

test_deny_an_agent_project_that_deploys_to_kube_system if {
	refused(fixtures.ac_agent_project_patch([{"op": "add", "path": "/spec/destinations/-", "value": {"server": "https://kubernetes.default.svc", "namespace": "kube-system"}}]), "must not deploy to")
}

test_deny_an_agent_project_with_a_cluster_kind if {
	refused(fixtures.ac_agent_project_patch([{"op": "add", "path": "/spec/clusterResourceWhitelist", "value": [{"group": "rbac.authorization.k8s.io", "kind": "ClusterRole"}]}]), "clusterResourceWhitelist")
}

test_deny_an_agent_project_with_secrets_of_everyone if {
	refused(fixtures.ac_agent_project_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "batch", "kind": "Job"}}]), "namespaceResourceWhitelist")
}

test_deny_an_agent_project_without_its_values_file if {
	planted := [f | some f in fixtures.ac_swimlane; not endswith(f.path, "values/agent/sit1.yaml")]
	refused(planted, "values/agent/sit1.yaml")
}

test_two_watched_namespaces_pass if {
	planted := array.concat(
		[f | some f in fixtures.ac_agent_project_patch([{"op": "add", "path": "/spec/destinations/-", "value": {"server": "https://kubernetes.default.svc", "namespace": "sit1-extra"}}]); not endswith(f.path, "values/agent/sit1.yaml")],
		[{"path": "deploy/values/agent/sit1.yaml", "contents": {"swimlane": {"id": "sit1"}, "namespaces": ["sit1", "sit1-extra"]}}],
	)
	count(deny) == 0 with input as planted
}

# --- the agent's spool (09/T8): a claim in the namespaced list, and the one StorageClass the agent chart makes, by name

test_deny_the_agent_project_with_a_storage_class_of_any_name if {
	refused(fixtures.ac_agent_project_patch([{"op": "remove", "path": "/spec/clusterResourceWhitelist/1/name"}]), "extra [\"storage.k8s.io/StorageClass/\"]")
}

test_deny_the_agent_project_with_a_storage_class_of_another_name if {
	refused(fixtures.ac_agent_project_patch([{"op": "replace", "path": "/spec/clusterResourceWhitelist/1/name", "value": "lanekeeper-hub-cmek"}]), "missing [\"storage.k8s.io/StorageClass/lanekeeper-agent-cmek\"]")
}

test_deny_the_agent_project_without_its_storage_class if {
	refused(fixtures.ac_agent_project_patch([{"op": "remove", "path": "/spec/clusterResourceWhitelist/1"}]), "missing [\"storage.k8s.io/StorageClass/lanekeeper-agent-cmek\"]")
}

test_deny_the_agent_project_without_persistent_volume_claims if {
	refused(fixtures.ac_agent_project_patch([{"op": "remove", "path": "/spec/namespaceResourceWhitelist/7"}]), "missing [\"/PersistentVolumeClaim\"]")
}

test_deny_the_workloads_project_with_persistent_volume_claims if {
	refused(fixtures.ac_workloads_patch([{"op": "add", "path": "/spec/namespaceResourceWhitelist/-", "value": {"group": "", "kind": "PersistentVolumeClaim"}}]), "extra [\"/PersistentVolumeClaim\"]")
}
