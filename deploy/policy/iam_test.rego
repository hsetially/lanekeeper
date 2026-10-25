# Tests for iam.rego (09/T5; S7, S9, S10, S18): the IAM bindings of the gcp chart give each service account the one role it needs on the one
# resource it needs it on, to nobody else. Run with `conftest verify --policy deploy/policy`.
package iam

import rego.v1

import data.fixtures

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

test_good_passes if {
	count(deny) == 0 with input as fixtures.gs
}

test_no_bindings_is_fine_for_other_charts if {
	count(deny) == 0 with input as fixtures.hub
	count(deny) == 0 with input as fixtures.db
}

# --- who may be bound

test_deny_all_users if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/member", "allUsers"), "which is everyone")
}

test_deny_all_authenticated_users if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/member", "allAuthenticatedUsers"), "which is everyone")
}

test_deny_a_person if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/member", "user:someone@example.com"), "service account")
}

test_deny_a_group if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/member", "group:everyone@example.com"), "service account")
}

test_deny_a_domain if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/member", "domain:example.com"), "service account")
}

test_deny_no_member if {
	planted := fixtures.gs_patch("IAMPolicyMember", "lanekeeper-hub-docs", [{"op": "remove", "path": "/spec/member"}])
	refused(planted, "names no member")
}

# --- which roles

test_deny_owner if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/role", "roles/owner"), "not one of the roles")
}

test_deny_editor if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/role", "roles/editor"), "not one of the roles")
}

test_deny_storage_admin if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/role", "roles/storage.admin"), "not one of the roles")
}

test_deny_object_admin if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/role", "roles/storage.objectAdmin"), "not one of the roles")
}

test_deny_token_creator if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/role", "roles/iam.serviceAccountTokenCreator"), "not one of the roles")
}

test_deny_a_custom_role if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/role", "projects/example-project/roles/anything"), "not one of the roles")
}

# --- where each role may be bound

test_deny_object_user_on_the_project if {
	planted := fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/resourceRef", {"kind": "Project", "external": "projects/example-project"})
	refused(planted, "on a StorageBucket")
}

test_deny_trace_agent_on_a_bucket if {
	planted := fixtures.gs_member_set("lanekeeper-hub-trace", "/spec/resourceRef", fixtures.gs_bucket_ref("example-project-lanekeeper-docs-prod"))
	refused(planted, "on the project")
}

test_deny_trace_agent_for_the_database if {
	refused(fixtures.gs_member_set("lanekeeper-hub-trace", "/spec/member", fixtures.gs_gsa("lanekeeper-db")), "the hub's service account")
}

test_deny_a_key_grant_on_a_bucket if {
	planted := fixtures.gs_member_set("lanekeeper-gcs-agent-key1", "/spec/resourceRef", fixtures.gs_bucket_ref("example-project-lanekeeper-docs-prod"))
	refused(planted, "on a KMSCryptoKey")
}

test_deny_a_workload_identity_grant_on_a_bucket if {
	planted := fixtures.gs_member_set("lanekeeper-hub-workload-identity", "/spec/resourceRef", fixtures.gs_bucket_ref("example-project-lanekeeper-docs-prod"))
	refused(planted, "on an IAMServiceAccount")
}

# --- Workload Identity

test_deny_workload_identity_for_a_google_service_account if {
	refused(fixtures.gs_member_set("lanekeeper-hub-workload-identity", "/spec/member", fixtures.gs_gsa("someone-else")), "Kubernetes service account")
}

test_deny_workload_identity_for_the_default_service_account if {
	planted := fixtures.gs_member_set("lanekeeper-hub-workload-identity", "/spec/member", "serviceAccount:example-project.svc.id.goog[lanekeeper/default]")
	refused(planted, "the default service account")
}

test_deny_workload_identity_for_every_namespace if {
	planted := fixtures.gs_member_set("lanekeeper-hub-workload-identity", "/spec/member", "serviceAccount:example-project.svc.id.goog[*/lanekeeper-hub]")
	refused(planted, "Kubernetes service account")
}

test_deny_workload_identity_for_another_pool if {
	planted := fixtures.gs_member_set("lanekeeper-hub-workload-identity", "/spec/member", "serviceAccount:other-project.svc.id.goog[lanekeeper/lanekeeper-hub]")
	refused(planted, "pool of the project")
}

# --- the Google service agents and the keys

test_deny_a_key_grant_to_the_hub if {
	refused(fixtures.gs_member_set("lanekeeper-gcs-agent-key1", "/spec/member", fixtures.gs_gsa("lanekeeper-hub")), "Google-managed service agent")
}

test_deny_a_key_grant_to_a_person if {
	refused(fixtures.gs_member_set("lanekeeper-gcs-agent-key1", "/spec/member", "user:someone@example.com"), "service account")
}

# --- each bucket has its own caller

test_deny_audit_bucket_object_user if {
	refused(fixtures.gs_member_set("lanekeeper-hub-audit-create", "/spec/role", "roles/storage.objectUser"), "may only create and read")
}

test_deny_audit_bucket_for_the_database if {
	refused(fixtures.gs_member_set("lanekeeper-hub-audit-create", "/spec/member", fixtures.gs_gsa("lanekeeper-db")), "the hub's service account")
}

test_deny_backup_bucket_for_the_hub if {
	refused(fixtures.gs_member_set("lanekeeper-db-backup", "/spec/member", fixtures.gs_gsa("lanekeeper-hub")), "the database's service account")
}

test_deny_docs_bucket_for_the_database if {
	refused(fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/member", fixtures.gs_gsa("lanekeeper-db")), "the hub's service account")
}

test_deny_a_binding_on_a_bucket_that_is_not_here if {
	planted := fixtures.gs_member_set("lanekeeper-hub-docs", "/spec/resourceRef", fixtures.gs_bucket_ref("some-other-bucket"))
	refused(planted, "is not in this output")
}

test_deny_object_creator_on_the_backup_bucket if {
	refused(fixtures.gs_member_set("lanekeeper-db-backup", "/spec/role", "roles/storage.objectCreator"), "needs to read, create and delete")
}
