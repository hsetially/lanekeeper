# Tests for argocd_scope.rego (09/T6; S16, S19): the project that deploys a chart allows every resource the chart renders. Without that, the
# first sync in the cluster stops at the first resource the project does not know. Run with `conftest verify --policy deploy/policy`.
package argocd_scope

import rego.v1

import data.fixtures

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

platform_projects := {"deploy/argocd/platform/projects.yaml": [fixtures.ac_proj_workloads, fixtures.ac_proj_cloud]}

# The platform's projects, and the rendered documents of a chart.
platform(rendered) := array.concat([{"path": "rendered/chart.yaml", "contents": rendered}], fixtures.ac_input(platform_projects))

# One swimlane's project, and the rendered documents of the agent chart.
swimlane(rendered) := array.concat(
	[{"path": "rendered/agent.yaml", "contents": rendered}],
	fixtures.ac_input({"deploy/argocd/swimlanes/sit1.yaml": [fixtures.ac_proj_agent, fixtures.ac_agent_app]}),
)

test_the_hub_chart_is_allowed if {
	count(deny) == 0 with input as platform(fixtures.hub_docs)
}

test_the_db_chart_is_allowed if {
	count(deny) == 0 with input as platform(fixtures.db_docs)
}

test_the_cloud_chart_is_allowed if {
	count(deny) == 0 with input as platform(fixtures.gs_docs)
	count(deny) == 0 with input as platform(fixtures.ba_docs)
	count(deny) == 0 with input as platform([fixtures.securitypolicy])
}

test_the_agent_chart_is_allowed if {
	count(deny) == 0 with input as swimlane(fixtures.agent_docs)
}

test_nothing_to_check_without_rendered_documents if {
	count(deny) == 0 with input as fixtures.ac_platform
	count(deny) == 0 with input as fixtures.ac_swimlane
}

test_deny_a_kind_no_project_knows if {
	planted := array.concat(fixtures.hub_docs, [{"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": "x", "namespace": "lanekeeper"}}])
	refused(platform(planted), "Job")
}

test_deny_a_secret_in_the_workloads if {
	planted := array.concat(fixtures.hub_docs, [{"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "x", "namespace": "lanekeeper"}}])
	refused(platform(planted), "Secret")
}

test_deny_a_cloud_binding_in_the_workloads if {
	planted := array.concat(fixtures.hub_docs, [{"apiVersion": "iam.cnrm.cloud.google.com/v1beta1", "kind": "IAMPolicyMember", "metadata": {"name": "x", "namespace": "lanekeeper"}}])
	refused(platform(planted), "IAMPolicyMember")
}

test_deny_a_workload_in_the_clouds_namespace if {
	planted := array.concat(fixtures.gs_docs, [{"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "x", "namespace": "lanekeeper-gcp"}}])
	refused(platform(planted), "Deployment")
}

test_deny_a_namespace_no_project_deploys_to if {
	planted := array.concat(fixtures.hub_docs, [json.patch(fixtures.serviceaccount, [{"op": "replace", "path": "/metadata/namespace", "value": "elsewhere"}])])
	refused(platform(planted), "elsewhere")
}

test_deny_a_namespaced_document_with_no_namespace if {
	planted := array.concat(fixtures.hub_docs, [json.patch(fixtures.serviceaccount, [{"op": "remove", "path": "/metadata/namespace"}])])
	refused(platform(planted), "names no namespace")
}

test_deny_a_cluster_role if {
	planted := array.concat(fixtures.hub_docs, [{"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "ClusterRole", "metadata": {"name": "x"}}])
	refused(platform(planted), "ClusterRole")
}

test_deny_a_namespace_resource_of_another_name if {
	planted := array.concat(fixtures.hub_docs, [{"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": "x"}}])
	refused(platform(planted), "Namespace")
}

test_the_projects_own_namespace_is_allowed_by_name if {
	planted := array.concat(fixtures.hub_docs, [{"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": "lanekeeper"}}])
	count(deny) == 0 with input as platform(planted)
}

test_deny_a_storage_class_in_the_cloud_project_alone if {
	only_cloud := array.concat(
		[{"path": "rendered/chart.yaml", "contents": [fixtures.storageclass]}],
		fixtures.ac_input({"deploy/argocd/platform/projects.yaml": [fixtures.ac_proj_cloud]}),
	)
	refused(only_cloud, "StorageClass")
}

test_deny_a_role_in_a_namespace_the_swimlane_does_not_watch if {
	role := {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "Role", "metadata": {"name": "x", "namespace": "sit2"}}
	refused(swimlane(array.concat(fixtures.agent_docs, [role])), "sit2")
}

test_a_role_in_the_watched_namespace_is_allowed if {
	role := {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "Role", "metadata": {"name": "x", "namespace": "sit1"}}
	count(deny) == 0 with input as swimlane(array.concat(fixtures.agent_docs, [role]))
}

test_deny_a_cluster_role_for_the_agent if {
	planted := array.concat(fixtures.agent_docs, [{"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "ClusterRole", "metadata": {"name": "x"}}])
	refused(swimlane(planted), "ClusterRole")
}

test_deny_a_persistent_volume_claim_until_the_project_knows_it if {
	pvc := {"apiVersion": "v1", "kind": "PersistentVolumeClaim", "metadata": {"name": "lanekeeper-agent-spool", "namespace": "lanekeeper"}}
	refused(swimlane(array.concat(fixtures.agent_docs, [pvc])), "PersistentVolumeClaim")
}
