# Tests for storage.rego: persistent volumes and buckets are encrypted with a customer-managed key (S10), the audit checkpoint bucket
# has a locked retention policy (S9), no bucket is public, and the docs bucket keeps its versions (09/T5).
package storage

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

vct_class(name) := {"op": "replace", "path": "/spec/volumeClaimTemplates/0/spec/storageClassName", "value": name}

test_good_passes if {
	count(deny) == 0 with input as fixtures.hub
	count(warn) == 0 with input as fixtures.hub
}

test_deny_pvc_without_cmek_class if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [vct_class("standard-rwo")])
	refused(planted, "customer-managed")
}

test_deny_pvc_without_a_class if {
	refused(fixtures.sts_remove("/spec/volumeClaimTemplates/0/spec/storageClassName"), "customer-managed")
}

test_deny_class_without_a_key if {
	planted := fixtures.patch("StorageClass", "lanekeeper-hub-cmek", [{"op": "remove", "path": "/parameters/disk-encryption-kms-key"}])
	refused(planted, "customer-managed")
}

test_deny_class_with_an_empty_key if {
	planted := fixtures.patch("StorageClass", "lanekeeper-hub-cmek", [{"op": "replace", "path": "/parameters/disk-encryption-kms-key", "value": ""}])
	refused(planted, "customer-managed")
}

test_deny_standalone_pvc_without_cmek_class if {
	pvc := {
		"apiVersion": "v1", "kind": "PersistentVolumeClaim", "metadata": {"name": "scratch"},
		"spec": {"accessModes": ["ReadWriteOnce"], "storageClassName": "standard-rwo", "resources": {"requests": {"storage": "1Gi"}}},
	}
	refused(fixtures.with_doc(pvc), "PersistentVolumeClaim/scratch")
}

# The fallback (plan 09 A11b): without a key the chart uses the cluster's default class and marks the claim; that is allowed to
# render, with a warning, and render.sh turns the warning into a failure for prod and staging.
test_fallback_warns_instead_of_failing if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		vct_class("standard-rwo"),
		{"op": "add", "path": "/spec/volumeClaimTemplates/0/metadata/annotations", "value": {"lanekeeper.io/cmek": "fallback"}},
	])
	count(deny) == 0 with input as planted
	warned(planted, "Google-managed")
}

test_fallback_marker_does_not_excuse_a_wrong_value if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		vct_class("standard-rwo"),
		{"op": "add", "path": "/spec/volumeClaimTemplates/0/metadata/annotations", "value": {"lanekeeper.io/cmek": "true"}},
	])
	refused(planted, "customer-managed")
}

# --- the database's volumes (09/T5)

test_good_database_storage_passes if {
	count(deny) == 0 with input as fixtures.db
	count(warn) == 0 with input as fixtures.db
}

test_deny_cluster_storage_without_cmek_class if {
	refused(fixtures.db_cluster_set("/spec/storage/storageClass", "standard-rwo"), "Cluster/lanekeeper-db, claim storage")
}

test_deny_cluster_storage_without_a_class if {
	refused(fixtures.db_cluster_remove("/spec/storage/storageClass"), "customer-managed")
}

test_deny_cluster_wal_storage_without_cmek_class if {
	planted := fixtures.db_cluster_add("/spec/walStorage", {"size": "5Gi", "storageClass": "standard-rwo"})
	refused(planted, "claim walStorage")
}

test_cluster_wal_storage_with_the_cmek_class_passes if {
	planted := fixtures.db_cluster_add("/spec/walStorage", {"size": "5Gi", "storageClass": "lanekeeper-db-cmek"})
	count(deny) == 0 with input as planted
}

test_deny_cluster_class_without_a_key if {
	planted := fixtures.db_patch("StorageClass", "lanekeeper-db-cmek", [{"op": "remove", "path": "/parameters/disk-encryption-kms-key"}])
	refused(planted, "customer-managed")
}

test_cluster_fallback_warns_instead_of_failing if {
	planted := fixtures.db_patch("Cluster", "lanekeeper-db", [
		{"op": "replace", "path": "/spec/storage/storageClass", "value": "standard-rwo"},
		{"op": "add", "path": "/metadata/annotations", "value": {"lanekeeper.io/cmek": "fallback"}},
	])
	count(deny) == 0 with input as planted
	warned(planted, "Google-managed")
}

# --- the buckets (09/T5)

test_good_buckets_pass if {
	count(deny) == 0 with input as fixtures.gs
	count(warn) == 0 with input as fixtures.gs
}

test_deny_bucket_without_cmek if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-docs-prod", "/spec/encryption"), "customer-managed")
}

test_deny_backup_without_cmek_bucket if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-backup-prod", "/spec/encryption"), "backup bucket")
}

test_deny_audit_bucket_without_cmek if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-audit-prod", "/spec/encryption"), "audit-checkpoint bucket")
}

test_deny_bucket_with_an_empty_key if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-docs-prod", "/spec/encryption/kmsKeyRef/external", ""), "customer-managed")
}

test_deny_bucket_with_a_malformed_key if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-docs-prod", "/spec/encryption/kmsKeyRef/external", "lanekeeper-docs"), "not a Cloud KMS key")
}

test_deny_bucket_with_a_key_of_a_key_ring if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-docs-prod", "/spec/encryption/kmsKeyRef/external", "projects/p/locations/l/keyRings/r"), "not a Cloud KMS key")
}

# The fallback (plan 09 A11b, Q29): without a key a bucket is allowed to render with Google-managed encryption, with a warning, and
# render.sh turns the warning into a failure for prod and staging.
test_bucket_fallback_warns_instead_of_failing if {
	planted := fixtures.gs_patch("StorageBucket", "example-project-lanekeeper-docs-prod", [
		{"op": "remove", "path": "/spec/encryption"},
		{"op": "add", "path": "/metadata/annotations/lanekeeper.io~1cmek", "value": "fallback"},
	])
	count(deny) == 0 with input as planted
	warned(planted, "Google-managed")
}

test_bucket_fallback_marker_does_not_excuse_a_malformed_key if {
	planted := fixtures.gs_patch("StorageBucket", "example-project-lanekeeper-docs-prod", [
		{"op": "replace", "path": "/spec/encryption/kmsKeyRef/external", "value": "nonsense"},
		{"op": "add", "path": "/metadata/annotations/lanekeeper.io~1cmek", "value": "fallback"},
	])
	refused(planted, "not a Cloud KMS key")
}

test_deny_public_bucket_prevention_inherited if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-docs-prod", "/spec/publicAccessPrevention", "inherited"), "public access prevention")
}

test_deny_public_bucket_prevention_missing if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-docs-prod", "/spec/publicAccessPrevention"), "public access prevention")
}

test_deny_bucket_without_uniform_access if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-docs-prod", "/spec/uniformBucketLevelAccess", false), "uniform bucket-level access")
}

test_deny_bucket_uniform_access_missing if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-docs-prod", "/spec/uniformBucketLevelAccess"), "uniform bucket-level access")
}

test_deny_bucket_that_is_deleted_with_its_resource if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-backup-prod", "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy"), "deletion-policy: abandon")
}

test_deny_bucket_with_another_deletion_policy if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-backup-prod", "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy", "delete"), "deletion-policy: abandon")
}

test_deny_bucket_that_argocd_may_prune if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-backup-prod", "/metadata/annotations/argocd.argoproj.io~1sync-options"), "Prune=false")
}

test_deny_bucket_without_a_location if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-docs-prod", "/spec/location"), "location")
}

# --- the audit checkpoint bucket (S9)

test_deny_missing_retention if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-audit-prod", "/spec/retentionPolicy"), "retention period")
}

test_deny_zero_retention if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-audit-prod", "/spec/retentionPolicy/retentionPeriod", 0), "retention period")
}

test_deny_retention_that_is_not_a_number if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-audit-prod", "/spec/retentionPolicy/retentionPeriod", "1y"), "retention period")
}

test_deny_unlocked_audit_bucket_in_prod if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-audit-prod", "/spec/retentionPolicy/isLocked", false), "locked")
}

test_deny_audit_bucket_lock_missing_in_prod if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-audit-prod", "/spec/retentionPolicy/isLocked"), "locked")
}

# Staging may keep its retention policy unlocked: a lock cannot be undone, and staging is where a period is tried.
test_unlocked_audit_bucket_is_allowed_in_staging if {
	planted := fixtures.gs_patch("StorageBucket", "example-project-lanekeeper-audit-prod", [
		{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1environment", "value": "staging"},
		{"op": "replace", "path": "/spec/retentionPolicy/isLocked", "value": false},
	])
	count(deny) == 0 with input as planted
}

test_deny_locked_audit_bucket_in_staging if {
	planted := fixtures.gs_bucket_set("example-project-lanekeeper-audit-prod", "/metadata/labels/lanekeeper.io~1environment", "staging")
	refused(planted, "locked in staging")
}

test_deny_environment_that_is_neither_staging_nor_prod if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-audit-prod", "/metadata/labels/lanekeeper.io~1environment", "dev"), "lanekeeper.io/environment")
}

test_deny_environment_missing if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-audit-prod", "/metadata/labels/lanekeeper.io~1environment"), "lanekeeper.io/environment")
}

# A locked policy cannot be undone, and only this bucket is meant to keep its objects for good: a lock on the backups would stop
# the retention of 30 days from deleting anything.
test_deny_locked_backup_bucket if {
	planted := fixtures.gs_bucket_add("example-project-lanekeeper-backup-prod", "/spec/retentionPolicy", {"retentionPeriod": 2592000, "isLocked": true})
	refused(planted, "only the audit-checkpoint bucket may be locked")
}

test_deny_locked_docs_bucket if {
	planted := fixtures.gs_bucket_add("example-project-lanekeeper-docs-prod", "/spec/retentionPolicy", {"retentionPeriod": 2592000, "isLocked": true})
	refused(planted, "only the audit-checkpoint bucket may be locked")
}

# --- the docs bucket

test_deny_docs_without_versioning if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-docs-prod", "/spec/versioning"), "versioning")
}

test_deny_docs_with_versioning_off if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-docs-prod", "/spec/versioning/enabled", false), "versioning")
}

# --- which buckets there are

test_deny_a_missing_backup_bucket if {
	planted := fixtures.gs_with([d | some d in fixtures.gs_docs; not d.metadata.name == "example-project-lanekeeper-backup-prod"])
	refused(planted, "backup bucket")
}

test_deny_a_missing_audit_bucket if {
	planted := fixtures.gs_with([d | some d in fixtures.gs_docs; not d.metadata.name == "example-project-lanekeeper-audit-prod"])
	refused(planted, "audit-checkpoint bucket")
}

test_deny_two_backup_buckets if {
	extra := json.patch(fixtures.gs_backup_bucket, [{"op": "replace", "path": "/metadata/name", "value": "example-project-lanekeeper-backup-two"}])
	refused(fixtures.gs_with_doc(extra), "exactly one")
}

test_deny_a_bucket_of_no_known_purpose if {
	refused(fixtures.gs_bucket_set("example-project-lanekeeper-docs-prod", "/metadata/labels/lanekeeper.io~1purpose", "scratch"), "purpose")
}

test_deny_a_bucket_without_a_purpose if {
	refused(fixtures.gs_bucket_remove("example-project-lanekeeper-docs-prod", "/metadata/labels/lanekeeper.io~1purpose"), "purpose")
}

# A chart that renders no bucket at all (the hub chart, the db chart) is not judged for having none.
test_no_buckets_is_fine_for_other_charts if {
	count(deny) == 0 with input as fixtures.hub
}

# --- the key must be usable by the service that encrypts with it

test_deny_cmek_bucket_without_a_grant_on_its_key if {
	planted := fixtures.gs_with([d | some d in fixtures.gs_docs; not d.metadata.name == "lanekeeper-gcs-agent-key1"])
	refused(planted, "no IAMPolicyMember grants the Cloud Storage service agent")
}

test_deny_grant_on_another_key_does_not_count if {
	planted := fixtures.gs_member_set("lanekeeper-gcs-agent-key1", "/spec/resourceRef/external", fixtures.gs_key("other"))
	refused(planted, "no IAMPolicyMember grants the Cloud Storage service agent")
}

test_deny_grant_to_the_wrong_agent_does_not_count if {
	planted := fixtures.gs_member_set("lanekeeper-gcs-agent-key1", "/spec/member", fixtures.gs_compute_agent)
	refused(planted, "no IAMPolicyMember grants the Cloud Storage service agent")
}

test_fallback_bucket_needs_no_grant if {
	planted := fixtures.gs_with([d |
		some d0 in fixtures.gs_docs
		d := fallback_docs(d0)
		not d.metadata.name == "lanekeeper-gcs-agent-key3"
	])
	count(deny) == 0 with input as planted
}

fallback_docs(d) := json.patch(d, [
	{"op": "remove", "path": "/spec/encryption"},
	{"op": "add", "path": "/metadata/annotations/lanekeeper.io~1cmek", "value": "fallback"},
]) if {
	d.metadata.name == "example-project-lanekeeper-docs-prod"
}

fallback_docs(d) := d if d.metadata.name != "example-project-lanekeeper-docs-prod"

# --- the agent's spool (09/T8; D74, S10): one claim of 1 GiB on a class with a customer-managed key, larger than the bound the agent
# puts on the spool (LK_SPOOL_MAX_BYTES, 512 MiB unless it is set), read and written by one pod at a time.

claim_change(ops) := fixtures.agent_patch("PersistentVolumeClaim", "lanekeeper-agent-spool", ops)

claim_request(q) := claim_change([{"op": "replace", "path": "/spec/resources/requests/storage", "value": q}])

raised_bound(n) := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/env/13/value", "value": n}])

test_agent_good_storage_passes if {
	count(deny) == 0 with input as fixtures.agent
	count(warn) == 0 with input as fixtures.agent
}

test_agent_spool_pvc_1gi if {
	pvcs := [d | some d in fixtures.agent_docs; d.kind == "PersistentVolumeClaim"]
	count(pvcs) == 1
	pvcs[0].metadata.name == "lanekeeper-agent-spool"
	pvcs[0].spec.resources.requests.storage == "1Gi"
	count(deny) == 0 with input as fixtures.agent
}

test_deny_agent_spool_pvc_as_big_as_the_bound if {
	refused(claim_request("512Mi"), "must be larger")
}

test_deny_agent_spool_pvc_smaller_than_the_bound if {
	refused(claim_request("256Mi"), "must be larger")
}

test_deny_agent_spool_pvc_smaller_than_a_raised_bound if {
	refused(raised_bound("2147483648"), "must be larger")
}

test_agent_spool_pvc_larger_than_a_raised_bound_passes if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/env/13/value", "value": "2147483648"}])
	bigger := fixtures.agent_with(array.concat(
		[d | some d in planted[0].contents; d.kind != "PersistentVolumeClaim"],
		[json.patch(fixtures.agent_spool_pvc, [{"op": "replace", "path": "/spec/resources/requests/storage", "value": "3Gi"}])],
	))
	count(deny) == 0 with input as bigger
}

# Without the variable the agent's own default is the bound (crates/agent/src/config.rs: 536870912).
test_deny_agent_spool_pvc_smaller_than_the_default_bound if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "remove", "path": "/spec/template/spec/containers/0/env/13"}])
	small := fixtures.agent_with(array.concat(
		[d | some d in planted[0].contents; d.kind != "PersistentVolumeClaim"],
		[json.patch(fixtures.agent_spool_pvc, [{"op": "replace", "path": "/spec/resources/requests/storage", "value": "500Mi"}])],
	))
	refused(small, "must be larger")
}

test_deny_agent_spool_bound_that_is_not_a_number if {
	refused(raised_bound("lots"), "LK_SPOOL_MAX_BYTES")
}

test_deny_agent_spool_size_in_a_unit_the_policy_cannot_compare if {
	refused(claim_request("2G"), "cannot be compared")
}

test_deny_agent_without_its_spool_claim if {
	refused(fixtures.agent_without("PersistentVolumeClaim"), "is not rendered")
}

test_deny_agent_spool_pvc_without_cmek_class if {
	refused(claim_change([{"op": "replace", "path": "/spec/storageClassName", "value": "standard-rwo"}]), "customer-managed")
}

test_deny_agent_spool_class_without_a_key if {
	planted := fixtures.agent_patch("StorageClass", "lanekeeper-agent-cmek", [{"op": "remove", "path": "/parameters/disk-encryption-kms-key"}])
	refused(planted, "customer-managed")
}

test_agent_spool_fallback_warns_instead_of_failing if {
	planted := fixtures.agent_with(array.concat(
		[d | some d in fixtures.agent_docs; d.kind != "PersistentVolumeClaim"; d.kind != "StorageClass"],
		[json.patch(fixtures.agent_spool_pvc, [
			{"op": "replace", "path": "/spec/storageClassName", "value": "standard-rwo"},
			{"op": "add", "path": "/metadata/annotations/lanekeeper.io~1cmek", "value": "fallback"},
		])],
	))
	count(deny) == 0 with input as planted
	warned(planted, "Google-managed")
}

test_deny_agent_spool_pvc_read_write_many if {
	refused(claim_change([{"op": "replace", "path": "/spec/accessModes", "value": ["ReadWriteMany"]}]), "exactly ReadWriteOnce")
}

test_deny_agent_spool_pvc_with_two_access_modes if {
	refused(claim_change([{"op": "replace", "path": "/spec/accessModes", "value": ["ReadWriteOnce", "ReadWriteMany"]}]), "exactly ReadWriteOnce")
}

# The spool holds changes the hub has not acknowledged: a chart refactor or a sync that prunes the claim must not delete it.
test_deny_agent_spool_pvc_that_can_be_pruned if {
	refused(claim_change([{"op": "remove", "path": "/metadata/annotations/argocd.argoproj.io~1sync-options"}]), "Prune=false")
}

test_deny_agent_spool_pvc_that_helm_removes if {
	refused(claim_change([{"op": "remove", "path": "/metadata/annotations/helm.sh~1resource-policy"}]), "helm.sh/resource-policy")
}
