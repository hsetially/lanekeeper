# Storage policy (09/T3; S10). Run with `conftest test --combine --namespace storage`. Every persistent volume is encrypted with
# a customer-managed key (CMEK): its claim names a StorageClass that is rendered next to it and carries a Cloud KMS key. The
# chart's fallback for a cluster without a key (plan 09 A11b, Q29) is the cluster's default class and the annotation
# `lanekeeper.io/cmek: fallback` on the claim; that renders with a warning, and `deploy/tests/render.sh` makes the warning a
# failure for prod and staging.
#
# The buckets of the gcp chart (09/T5; S9, S10) are checked here too: every one is private, has uniform access and a
# customer-managed key, and survives the deletion of its Kubernetes resource; the audit checkpoint bucket has a retention policy
# that is locked in prod; the docs bucket keeps its versions.
package storage

import rego.v1

import data.lk

cmek_classes := {c.metadata.name |
	some c in lk.of_kind("StorageClass")
	key := object.get(c, ["parameters", "disk-encryption-kms-key"], "")
	key != ""
}

# The volumes of a CloudNativePG cluster: the operator makes the claims, from `storage` and, if there is one, `walStorage`. A claim is
# described by the class the cluster names and by the cluster's own annotations (where the fallback marker is).
cluster_claims := [{"owner": lk.who(c), "claim": {"metadata": {"name": field, "annotations": object.get(c.metadata, "annotations", {})}, "spec": {"storageClassName": object.get(c.spec[field], "storageClass", "")}}} |
	some c in lk.of_kind("Cluster")
	startswith(c.apiVersion, "postgresql.cnpg.io/")
	some field in ["storage", "walStorage"]
	c.spec[field]
]

# Every claim in the output: a standalone PersistentVolumeClaim, a StatefulSet's claim template, or a database cluster's volume.
claims := array.concat(
	array.concat(
		[{"owner": lk.who(c), "claim": c} | some c in lk.of_kind("PersistentVolumeClaim")],
		[{"owner": lk.who(s), "claim": t} | some s in lk.of_kind("StatefulSet"); some t in object.get(s.spec, "volumeClaimTemplates", [])],
	),
	cluster_claims,
)

fallback(claim) if object.get(claim.metadata, ["annotations", "lanekeeper.io/cmek"], "") == "fallback"

deny contains msg if {
	some x in claims
	not object.get(x.claim.spec, "storageClassName", "") in cmek_classes
	not fallback(x.claim)
	msg := sprintf("%s, claim %s: its StorageClass is not one with a customer-managed key (disk-encryption-kms-key) rendered with it (S10)", [x.owner, x.claim.metadata.name])
}

warn contains msg if {
	some x in claims
	not object.get(x.claim.spec, "storageClassName", "") in cmek_classes
	fallback(x.claim)
	msg := sprintf("%s, claim %s: Google-managed encryption, not a customer-managed key (S10, Q29): set storage.cmekKey", [x.owner, x.claim.metadata.name])
}

# --- buckets (09/T5) -----------------------------------------------------------------------------------------------------------

buckets := [b | some b in lk.of_kind("StorageBucket"); startswith(b.apiVersion, "storage.cnrm.cloud.google.com/")]

purposes := {"backup", "audit-checkpoint", "docs"}

environments := {"staging", "prod"}

key_pattern := `^projects/[^/]+/locations/[^/]+/keyRings/[^/]+/cryptoKeys/[^/]+$`

purpose(b) := object.get(b.metadata, ["labels", "lanekeeper.io/purpose"], "")

environment(b) := object.get(b.metadata, ["labels", "lanekeeper.io/environment"], "")

key(b) := object.get(b.spec, ["encryption", "kmsKeyRef", "external"], "")

bucket_fallback(b) if object.get(b.metadata, ["annotations", "lanekeeper.io/cmek"], "") == "fallback"

# "the backup bucket", "the audit-checkpoint bucket": what a message calls a bucket by what it is for.
what(b) := sprintf("%s (the %s bucket)", [lk.who(b), purpose(b)]) if purpose(b) in purposes

what(b) := sprintf("%s (a bucket of no known purpose)", [lk.who(b)]) if not purpose(b) in purposes

deny contains msg if {
	some b in buckets
	not purpose(b) in purposes
	msg := sprintf("%s: the label lanekeeper.io/purpose must be one of %v, and is %q (S9, S10)", [lk.who(b), sort(purposes), purpose(b)])
}

deny contains msg if {
	some b in buckets
	not environment(b) in environments
	msg := sprintf("%s: the label lanekeeper.io/environment must be one of %v, and is %q; the audit checkpoint bucket is locked in prod and the policy needs to know which this is (S9)", [lk.who(b), sort(environments), environment(b)])
}

# Exactly one bucket for each purpose, wherever buckets are rendered at all.
deny contains msg if {
	count(buckets) > 0
	some p in purposes
	count([b | some b in buckets; purpose(b) == p]) != 1
	msg := sprintf("there must be exactly one %s bucket, and there are %d (S9, S10)", [p, count([b | some b in buckets; purpose(b) == p])])
}

deny contains msg if {
	some b in buckets
	object.get(b.spec, "uniformBucketLevelAccess", false) != true
	msg := sprintf("%s: uniform bucket-level access is not on, so object ACLs could open single objects (S10)", [what(b)])
}

deny contains msg if {
	some b in buckets
	object.get(b.spec, "publicAccessPrevention", "") != "enforced"
	msg := sprintf("%s: public access prevention is not enforced (publicAccessPrevention), so the bucket could be made public (S10)", [what(b)])
}

deny contains msg if {
	some b in buckets
	object.get(b.spec, "location", "") == ""
	msg := sprintf("%s: it names no location", [what(b)])
}

# A customer-managed key (S10, Q29): a Cloud KMS key, named in full. Without one the bucket falls back to Google-managed
# encryption, which is allowed to render with a warning.
deny contains msg if {
	some b in buckets
	key(b) == ""
	not bucket_fallback(b)
	msg := sprintf("%s: it names no customer-managed key (encryption.kmsKeyRef.external) and is not marked as the fallback (S10)", [what(b)])
}

deny contains msg if {
	some b in buckets
	key(b) != ""
	not regex.match(key_pattern, key(b))
	msg := sprintf("%s: %q is not a Cloud KMS key (projects/P/locations/L/keyRings/R/cryptoKeys/K) (S10)", [what(b), key(b)])
}

warn contains msg if {
	some b in buckets
	key(b) == ""
	bucket_fallback(b)
	msg := sprintf("%s: Google-managed encryption, not a customer-managed key (S10, Q29): set the bucket's cmekKey", [what(b)])
}

# Deleting the Kubernetes resource, or pruning it, must not delete a bucket and what is in it.
deny contains msg if {
	some b in buckets
	object.get(b.metadata, ["annotations", "cnrm.cloud.google.com/deletion-policy"], "") != "abandon"
	msg := sprintf("%s: the annotation cnrm.cloud.google.com/deletion-policy: abandon is missing; removing the resource would delete the bucket and its objects (S9, D40)", [what(b)])
}

deny contains msg if {
	some b in buckets
	object.get(b.metadata, ["annotations", "argocd.argoproj.io/sync-options"], "") != "Prune=false"
	msg := sprintf("%s: the annotation argocd.argoproj.io/sync-options: Prune=false is missing; a sync could prune the resource (S9, D40)", [what(b)])
}

# The audit checkpoint bucket (S9): objects are kept for a period that nobody can shorten. The period is a human's decision
# (plan 09 A19; prod's was decided on 2026-10-26: three months), so it has no default; the lock is irreversible, so it is on only in prod.
audit_buckets := [b | some b in buckets; purpose(b) == "audit-checkpoint"]

retention(b) := b.spec.retentionPolicy.retentionPeriod

deny contains msg if {
	some b in audit_buckets
	not is_number(object.get(b.spec, ["retentionPolicy", "retentionPeriod"], null))
	msg := sprintf("%s: it has no retention period (retentionPolicy.retentionPeriod, in seconds); the checkpoints would not be kept (S9)", [what(b)])
}

deny contains msg if {
	some b in audit_buckets
	is_number(object.get(b.spec, ["retentionPolicy", "retentionPeriod"], null))
	retention(b) <= 0
	msg := sprintf("%s: the retention period is %v seconds; it must be positive (S9)", [what(b), retention(b)])
}

deny contains msg if {
	some b in audit_buckets
	environment(b) == "prod"
	object.get(b.spec, ["retentionPolicy", "isLocked"], false) != true
	msg := sprintf("%s: the retention policy is not locked in prod, so a person with access could shorten or remove it (S9)", [what(b)])
}

deny contains msg if {
	some b in audit_buckets
	environment(b) == "staging"
	object.get(b.spec, ["retentionPolicy", "isLocked"], false) == true
	msg := sprintf("%s: the retention policy is locked in staging; a lock cannot be undone, so it is set in prod only, after a person has confirmed the period (S9, plan 09 A19)", [what(b)])
}

deny contains msg if {
	some b in buckets
	purpose(b) != "audit-checkpoint"
	object.get(b.spec, ["retentionPolicy", "isLocked"], false) == true
	msg := sprintf("%s: only the audit-checkpoint bucket may be locked; a lock cannot be undone, and it would stop the deletion of old objects (S9, D40)", [what(b)])
}

# The docs bucket keeps the versions of what is written to it.
deny contains msg if {
	some b in buckets
	purpose(b) == "docs"
	object.get(b.spec, ["versioning", "enabled"], false) != true
	msg := sprintf("%s: versioning is not on; an overwritten or deleted document could not be recovered", [what(b)])
}

# A bucket encrypted with a key is written by the Cloud Storage service agent, which must be allowed to use that key: another
# IAMPolicyMember in the output grants it cryptoKeyEncrypterDecrypter on exactly that key.
storage_agent(member) if regex.match(`^serviceAccount:service-[0-9]+@gs-project-accounts\.iam\.gserviceaccount\.com$`, member)

key_grants(k) := [m |
	some m in lk.of_kind("IAMPolicyMember")
	m.spec.role == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
	object.get(m.spec, ["resourceRef", "kind"], "") == "KMSCryptoKey"
	object.get(m.spec, ["resourceRef", "external"], "") == k
	storage_agent(object.get(m.spec, "member", ""))
]

deny contains msg if {
	some b in buckets
	key(b) != ""
	regex.match(key_pattern, key(b))
	count(key_grants(key(b))) == 0
	msg := sprintf("%s: no IAMPolicyMember grants the Cloud Storage service agent cryptoKeyEncrypterDecrypter on %s, so the bucket could not be written (S10)", [what(b), key(b)])
}

# --- the agent's spool (09/T8; D74, S10) -------------------------------------------------------------------------------------------
#
# The agent keeps every observed version it has not delivered in a volume of its own, bounded by LK_SPOOL_MAX_BYTES. The claim is on a
# class with a customer-managed key (the rules above cover every claim), is larger than the bound it has to hold, is read and written by
# one pod at a time (the Deployment recreates the pod, topology.rego), and is never pruned: it holds changes the hub has not acknowledged.

spool_claim_name := "lanekeeper-agent-spool"

# The agent's own default (crates/agent/src/config.rs, DEFAULT of LK_SPOOL_MAX_BYTES), used when the chart does not set the variable.
default_spool_max_bytes := 536870912

spool_claims := lk.named("PersistentVolumeClaim", spool_claim_name)

spool_max_bytes := to_number(lk.agent_env("LK_SPOOL_MAX_BYTES")) if regex.match(`^[0-9]+$`, lk.agent_env("LK_SPOOL_MAX_BYTES"))

spool_max_bytes := default_spool_max_bytes if not lk.agent_env("LK_SPOOL_MAX_BYTES")

spool_request(c) := object.get(c.spec, ["resources", "requests", "storage"], "")

deny contains msg if {
	some d in lk.agent_deployments
	count(spool_claims) == 0
	msg := sprintf("%s: PersistentVolumeClaim/%s is not rendered, and the agent stops at start without its spool (D74)", [lk.who(d), spool_claim_name])
}

deny contains msg if {
	some d in lk.agent_deployments
	not regex.match(`^[0-9]+$`, lk.agent_env("LK_SPOOL_MAX_BYTES"))
	msg := sprintf("%s: LK_SPOOL_MAX_BYTES %q is not a number of bytes, so the spool claim cannot be held against it (D74)", [lk.who(d), lk.agent_env("LK_SPOOL_MAX_BYTES")])
}

deny contains msg if {
	some c in spool_claims
	not lk.mebibytes(spool_request(c))
	msg := sprintf("%s: the request %q cannot be compared with LK_SPOOL_MAX_BYTES: only Ki, Mi, Gi and a number of bytes are understood here", [lk.who(c), spool_request(c)])
}

deny contains msg if {
	some c in spool_claims
	bytes := lk.mebibytes(spool_request(c)) * 1048576
	bytes <= spool_max_bytes
	msg := sprintf("%s: it requests %v bytes and LK_SPOOL_MAX_BYTES is %v; the volume must be larger than the bound, which counts the segment files with the small state file on top (crates/agent/HARDENING.md)", [lk.who(c), bytes, spool_max_bytes])
}

deny contains msg if {
	some c in spool_claims
	object.get(c.spec, "accessModes", []) != ["ReadWriteOnce"]
	msg := sprintf("%s: accessModes must be exactly ReadWriteOnce: one agent at a time writes the spool, and the Deployment recreates its pod (D74)", [lk.who(c)])
}

deny contains msg if {
	some c in spool_claims
	object.get(c.metadata, ["annotations", "argocd.argoproj.io/sync-options"], "") != "Prune=false"
	msg := sprintf("%s: the annotation argocd.argoproj.io/sync-options: Prune=false is missing; a sync could prune the claim and the changes the hub has not acknowledged (D74)", [lk.who(c)])
}

deny contains msg if {
	some c in spool_claims
	object.get(c.metadata, ["annotations", "helm.sh/resource-policy"], "") != "keep"
	msg := sprintf("%s: the annotation helm.sh/resource-policy: keep is missing; removing the release would delete the claim and the changes the hub has not acknowledged (D74)", [lk.who(c)])
}
