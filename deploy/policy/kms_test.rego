# Tests for kms.rego (09/fix, I1; S5, S7, S8, S9, S10): the hub's keys in Cloud KMS and the Secret Manager secrets it reads, as the gcp chart
# makes them. Run with `conftest verify --policy deploy/policy`.
package kms

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

key_name := "lanekeeper-agent-ca-prod"

vault_name := "lanekeeper-vault-prod"

secret_name := "write-preview-key"

test_good_passes if {
	count(deny) == 0 with input as fixtures.gs
	count(warn) == 0 with input as fixtures.gs
}

test_nothing_to_check_in_other_charts if {
	count(deny) == 0 with input as fixtures.hub
	count(deny) == 0 with input as fixtures.db
	count(warn) == 0 with input as fixtures.db
}

# --- the key ring

test_deny_a_ring_without_a_location if {
	refused(fixtures.gs_patch("KMSKeyRing", "lanekeeper", [{"op": "remove", "path": "/spec/location"}]), "names no location")
}

test_deny_a_ring_deleted_with_its_resource if {
	planted := fixtures.gs_patch("KMSKeyRing", "lanekeeper", [{"op": "replace", "path": "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy", "value": "delete"}])
	refused(planted, "deletion-policy: abandon")
}

test_deny_a_ring_that_a_sync_may_prune if {
	planted := fixtures.gs_patch("KMSKeyRing", "lanekeeper", [{"op": "remove", "path": "/metadata/annotations/argocd.argoproj.io~1sync-options"}])
	refused(planted, "Prune=false")
}

# --- the keys: what each is for

test_deny_a_key_of_no_purpose if {
	refused(fixtures.gs_key_set(key_name, "/metadata/labels/lanekeeper.io~1purpose", "scratch"), "lanekeeper.io/purpose")
}

test_deny_a_key_without_a_purpose_label if {
	refused(fixtures.gs_key_remove(key_name, "/metadata/labels/lanekeeper.io~1purpose"), "lanekeeper.io/purpose")
}

test_deny_a_key_of_no_environment if {
	refused(fixtures.gs_key_set(key_name, "/metadata/labels/lanekeeper.io~1environment", "dev"), "lanekeeper.io/environment")
}

test_deny_two_agent_ca_keys if {
	planted := fixtures.gs_key_set("lanekeeper-replica-ca-prod", "/metadata/labels/lanekeeper.io~1purpose", "agent-ca")
	refused(planted, "exactly one agent-ca key")
	refused(planted, "exactly one replica-ca key")
}

test_deny_no_vault_key if {
	planted := fixtures.gs_with([d | some d in fixtures.gs_docs; not d.metadata.name == vault_name])
	refused(planted, "exactly one vault key")
}

test_deny_two_keys_of_one_name if {
	planted := fixtures.gs_with_doc(fixtures.gs_signing_key(key_name, "agent-ca"))
	refused(planted, "two KMSCryptoKey resources are named")
}

# --- the signing keys (the agent CA, the replica CA, the audit checkpoint key): asymmetric, P-256 (S5, S9)

test_deny_a_ca_key_that_encrypts if {
	refused(fixtures.gs_key_set(key_name, "/spec/purpose", "ENCRYPT_DECRYPT"), "must be ASYMMETRIC_SIGN")
}

test_deny_a_ca_key_without_a_purpose if {
	# The Config Connector default is ENCRYPT_DECRYPT, so a key that does not say is not a signing key.
	refused(fixtures.gs_key_remove(key_name, "/spec/purpose"), "must be ASYMMETRIC_SIGN")
}

test_deny_a_ca_key_for_decryption if {
	refused(fixtures.gs_key_set(key_name, "/spec/purpose", "ASYMMETRIC_DECRYPT"), "must be ASYMMETRIC_SIGN")
}

test_deny_a_ca_key_of_the_wrong_algorithm if {
	refused(fixtures.gs_key_set(key_name, "/spec/versionTemplate/algorithm", "RSA_SIGN_PSS_2048_SHA256"), "EC_SIGN_P256_SHA256")
}

test_deny_a_ca_key_of_the_wrong_curve if {
	refused(fixtures.gs_key_set(key_name, "/spec/versionTemplate/algorithm", "EC_SIGN_P384_SHA384"), "EC_SIGN_P256_SHA256")
}

test_deny_a_ca_key_without_an_algorithm if {
	refused(fixtures.gs_key_remove(key_name, "/spec/versionTemplate"), "EC_SIGN_P256_SHA256")
}

test_deny_the_checkpoint_key_of_the_wrong_algorithm if {
	refused(fixtures.gs_key_set("lanekeeper-audit-checkpoint-prod", "/spec/versionTemplate/algorithm", "RSA_SIGN_PKCS1_2048_SHA256"), "audit-checkpoint-signing key")
}

test_deny_a_signing_key_that_rotates if {
	refused(fixtures.gs_key_add(key_name, "/spec/rotationPeriod", "7776000s"), "rotationPeriod")
}

# --- the vault key: symmetric (S8)

test_deny_a_vault_key_that_signs if {
	refused(fixtures.gs_key_set(vault_name, "/spec/purpose", "ASYMMETRIC_SIGN"), "must be ENCRYPT_DECRYPT")
}

test_deny_a_vault_key_without_a_purpose if {
	refused(fixtures.gs_key_remove(vault_name, "/spec/purpose"), "must be ENCRYPT_DECRYPT")
}

test_deny_a_vault_key_of_the_wrong_algorithm if {
	refused(fixtures.gs_key_set(vault_name, "/spec/versionTemplate/algorithm", "EC_SIGN_P256_SHA256"), "GOOGLE_SYMMETRIC_ENCRYPTION")
}

test_a_vault_key_may_rotate if {
	count(deny) == 0 with input as fixtures.gs
	fixtures.gs_vault_key.spec.rotationPeriod == "7776000s"
}

# --- every key: where it is kept, who made the material

test_deny_a_key_without_a_protection_level if {
	refused(fixtures.gs_key_remove(key_name, "/spec/versionTemplate/protectionLevel"), "protectionLevel")
}

test_deny_a_key_held_outside_google if {
	refused(fixtures.gs_key_set(key_name, "/spec/versionTemplate/protectionLevel", "EXTERNAL"), "protectionLevel")
}

test_deny_a_software_key_in_prod if {
	refused(fixtures.gs_key_set(key_name, "/spec/versionTemplate/protectionLevel", "SOFTWARE"), "HSM in prod")
}

test_a_software_key_in_staging_is_allowed if {
	planted := fixtures.gs_patch("KMSCryptoKey", key_name, [
		{"op": "replace", "path": "/spec/versionTemplate/protectionLevel", "value": "SOFTWARE"},
		{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1environment", "value": "staging"},
	])
	not refused(planted, "protectionLevel")
}

test_deny_an_imported_key if {
	refused(fixtures.gs_key_add(key_name, "/spec/importOnly", true), "imported")
}

test_deny_a_key_that_skips_its_first_version if {
	refused(fixtures.gs_key_add(key_name, "/spec/skipInitialVersionCreation", true), "skipInitialVersionCreation")
}

test_deny_a_key_in_a_ring_that_is_not_here if {
	refused(fixtures.gs_key_set(key_name, "/spec/keyRingRef/name", "elsewhere"), "key ring")
}

test_deny_a_key_in_an_external_ring if {
	planted := fixtures.gs_key_set(key_name, "/spec/keyRingRef", {"external": "projects/example-project/locations/example-location/keyRings/elsewhere"})
	refused(planted, "key ring")
}

test_deny_a_key_deleted_with_its_resource if {
	planted := fixtures.gs_key_set(key_name, "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy", "delete")
	refused(planted, "deletion-policy: abandon")
}

test_deny_a_key_without_the_deletion_policy if {
	refused(fixtures.gs_key_remove(key_name, "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy"), "deletion-policy: abandon")
}

test_deny_a_key_that_a_sync_may_prune if {
	refused(fixtures.gs_key_remove(key_name, "/metadata/annotations/argocd.argoproj.io~1sync-options"), "Prune=false")
}

# --- the secrets: containers, never values (S7)

test_deny_a_secret_version if {
	planted := fixtures.gs_with_doc({
		"apiVersion": "secretmanager.cnrm.cloud.google.com/v1beta1",
		"kind": "SecretManagerSecretVersion",
		"metadata": {"name": "write-preview-key-1", "namespace": fixtures.gs_namespace},
		"spec": {"secretRef": {"name": secret_name}, "enabled": true},
	})
	refused(planted, "never a value")
}

test_deny_a_secret_version_whatever_its_group if {
	planted := fixtures.gs_with_doc({
		"apiVersion": "example.com/v1",
		"kind": "SecretManagerSecretVersion",
		"metadata": {"name": "x", "namespace": fixtures.gs_namespace},
		"spec": {},
	})
	refused(planted, "never a value")
}

test_deny_a_secret_with_a_value if {
	refused(fixtures.gs_secret_add(secret_name, "/spec/secretData", {"value": "hunter2"}), "replication and nothing else")
}

test_deny_a_secret_with_a_ttl if {
	refused(fixtures.gs_secret_add(secret_name, "/spec/ttl", "3600s"), "replication and nothing else")
}

test_deny_a_secret_that_expires if {
	refused(fixtures.gs_secret_add(secret_name, "/spec/expireTime", "2030-01-01T00:00:00Z"), "replication and nothing else")
}

test_deny_a_secret_with_a_rotation if {
	refused(fixtures.gs_secret_add(secret_name, "/spec/rotation", {"rotationPeriod": "3600s"}), "replication and nothing else")
}

test_deny_a_secret_that_notifies if {
	planted := fixtures.gs_secret_add(secret_name, "/spec/topics", [{"topicRef": {"external": "projects/example-project/topics/x"}}])
	refused(planted, "replication and nothing else")
}

test_deny_a_secret_with_annotations_that_could_hold_a_value if {
	refused(fixtures.gs_secret_add(secret_name, "/spec/annotations", {"value": "hunter2"}), "replication and nothing else")
}

# 09/fix, round 1 of the review (NB-4): the metadata of a container can hold a value as well as its spec can. Labels and annotations are
# readable by anyone who can list secrets; only the keys the chart writes are allowed.
test_deny_a_secret_with_a_metadata_annotation_that_could_hold_a_value if {
	refused(fixtures.gs_secret_add(secret_name, "/metadata/annotations/value", "hunter2"), "metadata.annotations.value")
}

test_deny_a_secret_with_a_metadata_label_that_could_hold_a_value if {
	refused(fixtures.gs_secret_add(secret_name, "/metadata/labels/password", "hunter2"), "metadata.labels.password")
}

test_deny_a_secret_with_a_note_in_a_namespaced_annotation if {
	refused(fixtures.gs_secret_add(secret_name, "/metadata/annotations/lanekeeper.io~1note", "hunter2"), "metadata.annotations.lanekeeper.io/note")
}

test_deny_a_secret_with_the_fallback_marker_holding_something_else if {
	planted := fixtures.gs_secret_add(secret_name, "/metadata/annotations/lanekeeper.io~1cmek", "hunter2")
	refused(planted, "lanekeeper.io/cmek must be fallback")
}

# 09/fix, round 2 of the review (NB-3): an allowed KEY can still hold a value. The values are the chart's own: the standard labels are
# fixed (the release is the Argo CD Application's name, argocd.rego), the chart label is `lanekeeper-gcp-<semver>`, the project is a project
# id and the one the key ring is in, and nothing in the metadata but the four fields the chart writes.
test_deny_a_secret_whose_instance_label_holds_a_value if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/labels/app.kubernetes.io~1instance", "hunter2"), "app.kubernetes.io/instance is \"hunter2\"")
}

test_deny_a_secret_whose_name_label_holds_a_value if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/labels/app.kubernetes.io~1name", "hunter2"), "app.kubernetes.io/name is \"hunter2\"")
}

test_deny_a_secret_whose_part_of_label_holds_a_value if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/labels/app.kubernetes.io~1part-of", "hunter2"), "app.kubernetes.io/part-of is \"hunter2\"")
}

test_deny_a_secret_whose_managed_by_label_holds_a_value if {
	refused(fixtures.gs_secret_add(secret_name, "/metadata/labels/app.kubernetes.io~1managed-by", "hunter2"), "app.kubernetes.io/managed-by is \"hunter2\"")
}

test_deny_a_secret_whose_chart_label_holds_a_value if {
	refused(fixtures.gs_secret_add(secret_name, "/metadata/labels/helm.sh~1chart", "hunter2"), "helm.sh/chart is \"hunter2\"")
}

test_deny_a_secret_whose_chart_label_has_a_suffix_that_holds_a_value if {
	refused(fixtures.gs_secret_add(secret_name, "/metadata/labels/helm.sh~1chart", "lanekeeper-gcp-0.1.0-hunter2"), "helm.sh/chart is")
}

test_deny_a_secret_whose_project_annotation_holds_a_value if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/annotations/cnrm.cloud.google.com~1project-id", "Hunter 2"), "is not a Google project id")
}

# A word that has the shape of a project id is still not the project of the key ring that the same chart makes.
test_deny_a_secret_in_another_project_than_the_key_ring if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/annotations/cnrm.cloud.google.com~1project-id", "hunter2"), "the key ring is in the project")
}

test_deny_a_secret_in_another_namespace_than_the_key_ring if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/namespace", "hunter2"), "namespace")
}

test_deny_a_secret_whose_deletion_policy_holds_a_value if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy", "abandon-hunter2"), "deletion-policy: abandon")
}

test_deny_a_secret_whose_sync_options_hold_a_value if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/annotations/argocd.argoproj.io~1sync-options", "Prune=false,hunter2"), "Prune=false")
}

test_deny_a_secret_with_a_finalizer if {
	refused(fixtures.gs_secret_add(secret_name, "/metadata/finalizers", ["hunter2"]), "metadata.finalizers")
}

test_deny_a_secret_with_a_generate_name if {
	refused(fixtures.gs_secret_add(secret_name, "/metadata/generateName", "hunter2"), "metadata.generateName")
}

test_deny_a_secret_with_an_owner_reference if {
	planted := fixtures.gs_secret_add(secret_name, "/metadata/ownerReferences", [{"apiVersion": "v1", "kind": "ConfigMap", "name": "hunter2", "uid": "hunter2"}])
	refused(planted, "metadata.ownerReferences")
}

test_deny_a_secret_with_a_field_of_its_own_outside_metadata_and_spec if {
	refused(fixtures.gs_secret_add(secret_name, "/data", {"value": "hunter2"}), "top-level field data")
}

# Everything the chart writes on a secret passes: the standard labels (with managed-by and the chart's), the project, the two keep
# annotations and the fallback marker.
test_a_secret_with_every_key_the_chart_writes_passes if {
	planted := fixtures.gs_patch("SecretManagerSecret", secret_name, [
		{"op": "add", "path": "/metadata/labels/app.kubernetes.io~1managed-by", "value": "Helm"},
		{"op": "add", "path": "/metadata/labels/helm.sh~1chart", "value": "lanekeeper-gcp-0.1.0"},
		{"op": "remove", "path": "/spec/replication/userManaged/replicas/0/customerManagedEncryption"},
		{"op": "add", "path": "/metadata/annotations/lanekeeper.io~1cmek", "value": "fallback"},
	])
	count(deny) == 0 with input as planted
}

# The same label or annotation on a key is not this rule's business: a key holds no value, and its metadata is held to its own rules.
test_a_note_on_a_key_is_not_the_secrets_rule if {
	planted := fixtures.gs_patch("KMSCryptoKey", key_name, [{"op": "add", "path": "/metadata/annotations/note", "value": "x"}])
	msgs := deny with input as planted
	count([m | some m in msgs; contains(m, "metadata.annotations")]) == 0
}

test_deny_a_secret_of_no_purpose if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/labels/lanekeeper.io~1purpose", "docs"), "lanekeeper.io/purpose")
}

test_deny_a_secret_of_no_environment if {
	refused(fixtures.gs_secret_set(secret_name, "/metadata/labels/lanekeeper.io~1environment", "dev"), "lanekeeper.io/environment")
}

test_deny_a_secret_deleted_with_its_resource if {
	planted := fixtures.gs_secret_set(secret_name, "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy", "delete")
	refused(planted, "deletion-policy: abandon")
}

test_deny_a_secret_that_a_sync_may_prune if {
	refused(fixtures.gs_secret_remove(secret_name, "/metadata/annotations/argocd.argoproj.io~1sync-options"), "Prune=false")
}

# --- the secrets: replication and the customer-managed key (S10, Q29)

test_deny_a_secret_without_replication if {
	refused(fixtures.gs_secret_remove(secret_name, "/spec/replication"), "userManaged")
}

test_deny_a_secret_with_automatic_replication if {
	refused(fixtures.gs_secret_set(secret_name, "/spec/replication", {"automatic": true}), "userManaged")
}

test_deny_a_secret_with_no_replica if {
	refused(fixtures.gs_secret_set(secret_name, "/spec/replication/userManaged/replicas", []), "userManaged")
}

test_deny_a_replica_without_a_location if {
	refused(fixtures.gs_secret_remove(secret_name, "/spec/replication/userManaged/replicas/0/location"), "no location")
}

test_deny_a_secret_without_a_key if {
	planted := fixtures.gs_secret_remove(secret_name, "/spec/replication/userManaged/replicas/0/customerManagedEncryption")
	refused(planted, "customer-managed key")
}

test_deny_a_secret_with_a_key_that_is_not_one if {
	planted := fixtures.gs_secret_set(secret_name, "/spec/replication/userManaged/replicas/0/customerManagedEncryption/kmsKeyRef/external", "lanekeeper-secrets")
	refused(planted, "not a Cloud KMS key")
}

test_deny_a_secret_with_a_key_by_name if {
	planted := fixtures.gs_secret_set(secret_name, "/spec/replication/userManaged/replicas/0/customerManagedEncryption/kmsKeyRef", {"name": "lanekeeper-vault-prod"})
	refused(planted, "customer-managed key")
}

# The fallback (Q29): no key renders with a warning, which render.sh makes a failure in prod and staging.
test_a_secret_without_a_key_is_a_fallback_with_a_warning if {
	planted := fixtures.gs_patch("SecretManagerSecret", secret_name, [
		{"op": "remove", "path": "/spec/replication/userManaged/replicas/0/customerManagedEncryption"},
		{"op": "add", "path": "/metadata/annotations/lanekeeper.io~1cmek", "value": "fallback"},
	])
	count(deny) == 0 with input as planted
	warned(planted, "Google-managed encryption")
}

# A secret encrypted with a key needs the Secret Manager service agent to be able to use that key, or it cannot be written.
test_deny_a_secrets_key_that_the_service_agent_may_not_use if {
	planted := fixtures.gs_with([d | some d in fixtures.gs_docs; not d.metadata.name == "lanekeeper-secretmanager-agent-key1"])
	refused(planted, "no IAMPolicyMember grants the Secret Manager service agent")
}

test_deny_a_secrets_key_that_only_another_agent_may_use if {
	planted := fixtures.gs_member_set("lanekeeper-secretmanager-agent-key1", "/spec/member", fixtures.gs_storage_agent)
	refused(planted, "no IAMPolicyMember grants the Secret Manager service agent")
}

test_deny_a_secrets_key_granted_on_another_key if {
	planted := fixtures.gs_member_set("lanekeeper-secretmanager-agent-key1", "/spec/resourceRef/external", fixtures.gs_key("another"))
	refused(planted, "no IAMPolicyMember grants the Secret Manager service agent")
}

# --- a document with no spec at all (`spec:` and nothing under it) is not a way past a rule

test_deny_a_ring_with_no_spec if {
	refused(fixtures.gs_patch("KMSKeyRing", "lanekeeper", [{"op": "replace", "path": "/spec", "value": null}]), "names no location")
}

test_deny_a_key_with_no_spec if {
	planted := fixtures.gs_patch("KMSCryptoKey", key_name, [{"op": "replace", "path": "/spec", "value": null}])
	refused(planted, "must be ASYMMETRIC_SIGN")
	refused(planted, "protectionLevel")
}

test_deny_a_vault_key_with_no_spec if {
	planted := fixtures.gs_patch("KMSCryptoKey", vault_name, [{"op": "replace", "path": "/spec", "value": null}])
	refused(planted, "must be ENCRYPT_DECRYPT")
}

test_deny_a_secret_with_no_spec if {
	planted := fixtures.gs_patch("SecretManagerSecret", secret_name, [{"op": "replace", "path": "/spec", "value": null}])
	refused(planted, "userManaged")
}
