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

# --- the hub's keys (09/fix, I1): signing keys and the vault key ----------------------------------------------------------------------

# The roles that Cloud KMS has beyond the two the hub needs are refused by name: administering a key, signing without verifying,
# reading a public key, using a key for anything.
test_deny_kms_admin if {
	refused(fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/role", "roles/cloudkms.admin"), "not one of the roles")
}

test_deny_kms_signer_without_verifier if {
	refused(fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/role", "roles/cloudkms.signer"), "not one of the roles")
}

test_deny_kms_public_key_viewer_for_the_hub if {
	refused(fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/role", "roles/cloudkms.publicKeyViewer"), "not one of the roles")
}

test_deny_kms_crypto_key_encrypter_for_the_hub if {
	refused(fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/role", "roles/cloudkms.cryptoKeyEncrypter"), "not one of the roles")
}

test_deny_kms_crypto_operator_for_the_hub if {
	refused(fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/role", "roles/cloudkms.cryptoOperator"), "not one of the roles")
}

test_the_signing_keys_are_signed_with_by_the_hub if {
	count(deny) == 0 with input as fixtures.gs
	some m in fixtures.gs_members
	m.spec.role == "roles/cloudkms.signerVerifier"
}

test_deny_signer_verifier_on_the_vault_key if {
	planted := fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/resourceRef", fixtures.gs_kms_ref("lanekeeper-vault-prod"))
	refused(planted, "signerVerifier is for the agent CA")
}

test_deny_signer_verifier_on_the_key_ring if {
	planted := fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/resourceRef", {"apiVersion": "kms.cnrm.cloud.google.com/v1beta1", "kind": "KMSKeyRing", "name": "lanekeeper"})
	refused(planted, "on a KMSCryptoKey, never on \"KMSKeyRing\"")
}

test_deny_signer_verifier_on_the_project if {
	planted := fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/resourceRef", {"kind": "Project", "external": "projects/example-project"})
	refused(planted, "on a KMSCryptoKey, never on \"Project\"")
}

test_deny_signer_verifier_on_a_key_that_is_not_here if {
	planted := fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/resourceRef", fixtures.gs_kms_ref("some-other-key"))
	refused(planted, "is not in this output")
}

test_deny_signer_verifier_on_an_external_key if {
	planted := fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/resourceRef", {"kind": "KMSCryptoKey", "external": fixtures.gs_key("agent-ca")})
	refused(planted, "is not in this output")
}

test_deny_signer_verifier_on_a_key_of_another_namespace if {
	planted := fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/resourceRef", object.union(fixtures.gs_kms_ref("lanekeeper-agent-ca-prod"), {"namespace": "other"}))
	refused(planted, "is not in this output")
}

test_deny_signer_verifier_for_the_database if {
	refused(fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/member", fixtures.gs_gsa("lanekeeper-db")), "only the hub's service account signs")
}

test_deny_signer_verifier_for_a_person if {
	refused(fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/member", "user:someone@example.com"), "service account")
}

test_deny_signer_verifier_for_a_group if {
	refused(fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/member", "group:everyone@example.com"), "service account")
}

test_deny_signer_verifier_for_everyone if {
	refused(fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/member", "allAuthenticatedUsers"), "which is everyone")
}

test_deny_signer_verifier_for_a_service_agent if {
	refused(fixtures.gs_member_set("lanekeeper-hub-sign-agent-ca", "/spec/member", fixtures.gs_storage_agent), "only the hub's service account signs")
}

test_deny_encrypter_decrypter_for_the_hub_on_a_ca_key if {
	planted := fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/resourceRef", fixtures.gs_kms_ref("lanekeeper-agent-ca-prod"))
	refused(planted, "only the vault key")
}

test_deny_encrypter_decrypter_for_the_hub_on_the_checkpoint_key if {
	planted := fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/resourceRef", fixtures.gs_kms_ref("lanekeeper-audit-checkpoint-prod"))
	refused(planted, "only the vault key")
}

test_deny_encrypter_decrypter_for_the_hub_on_the_key_ring if {
	planted := fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/resourceRef", {"apiVersion": "kms.cnrm.cloud.google.com/v1beta1", "kind": "KMSKeyRing", "name": "lanekeeper"})
	refused(planted, "on a KMSCryptoKey")
}

test_deny_encrypter_decrypter_for_the_database_on_the_vault_key if {
	refused(fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/member", fixtures.gs_gsa("lanekeeper-db")), "Google-managed service agent")
}

test_deny_encrypter_decrypter_for_a_person_on_the_vault_key if {
	refused(fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/member", "user:someone@example.com"), "service account")
}

test_deny_encrypter_decrypter_for_a_service_agent_on_the_vault_key if {
	refused(fixtures.gs_member_set("lanekeeper-hub-vault", "/spec/member", fixtures.gs_storage_agent), "made here")
}

test_deny_encrypter_decrypter_for_a_service_agent_on_a_ca_key if {
	planted := fixtures.gs_patch("IAMPolicyMember", "lanekeeper-gcs-agent-key1", [{"op": "replace", "path": "/spec/resourceRef", "value": fixtures.gs_kms_ref("lanekeeper-agent-ca-prod")}])
	refused(planted, "made here")
}

# --- the Secret Manager secrets the hub reads ---------------------------------------------------------------------------------------

test_deny_secret_manager_admin if {
	refused(fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/role", "roles/secretmanager.admin"), "not one of the roles")
}

test_deny_secret_manager_version_adder if {
	refused(fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/role", "roles/secretmanager.secretVersionAdder"), "not one of the roles")
}

test_deny_secret_manager_version_manager if {
	refused(fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/role", "roles/secretmanager.secretVersionManager"), "not one of the roles")
}

test_deny_secret_accessor_at_project_level if {
	planted := fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/resourceRef", {"kind": "Project", "external": "projects/example-project"})
	refused(planted, "on a SecretManagerSecret, never on \"Project\"")
}

test_deny_secret_accessor_on_a_bucket if {
	planted := fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/resourceRef", fixtures.gs_bucket_ref("example-project-lanekeeper-docs-prod"))
	refused(planted, "on a SecretManagerSecret")
}

test_deny_secret_accessor_on_a_secret_that_is_not_here if {
	planted := fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/resourceRef", fixtures.gs_secret_ref("teams-destination"))
	refused(planted, "is not in this output")
}

test_deny_secret_accessor_on_an_external_secret if {
	planted := fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/resourceRef", {"kind": "SecretManagerSecret", "external": "projects/example-project/secrets/write-preview-key"})
	refused(planted, "is not in this output")
}

test_deny_secret_accessor_for_the_database if {
	refused(fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/member", fixtures.gs_gsa("lanekeeper-db")), "only the hub's service account reads")
}

test_deny_secret_accessor_for_a_person if {
	refused(fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/member", "user:someone@example.com"), "service account")
}

test_deny_secret_accessor_for_a_group if {
	refused(fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/member", "group:everyone@example.com"), "service account")
}

test_deny_secret_accessor_for_everyone if {
	refused(fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/member", "allUsers"), "which is everyone")
}

test_deny_secret_accessor_for_a_service_agent if {
	refused(fixtures.gs_member_set("lanekeeper-hub-read-write-preview-key", "/spec/member", fixtures.gs_secretmanager_agent), "only the hub's service account reads")
}

# The Secret Manager service agent uses the key of the secrets' payloads, and only that role and only on an external key.
test_the_secret_manager_agent_may_use_the_secrets_key if {
	count(deny) == 0 with input as fixtures.gs
}

test_deny_the_secret_manager_agent_on_a_made_key if {
	planted := fixtures.gs_member_set("lanekeeper-secretmanager-agent-key1", "/spec/resourceRef", fixtures.gs_kms_ref("lanekeeper-vault-prod"))
	refused(planted, "made here")
}

test_deny_an_agent_of_another_service if {
	refused(fixtures.gs_member_set("lanekeeper-secretmanager-agent-key1", "/spec/member", "serviceAccount:service-123456789012@gcp-sa-pubsub.iam.gserviceaccount.com"), "Google-managed service agent")
}

# A binding with no spec at all (`spec:` and nothing under it) names no member and no role, and is refused for both.
test_deny_a_binding_with_no_spec if {
	planted := fixtures.gs_patch("IAMPolicyMember", "lanekeeper-hub-vault", [{"op": "replace", "path": "/spec", "value": null}])
	refused(planted, "names no member")
	refused(planted, "not one of the roles")
}
