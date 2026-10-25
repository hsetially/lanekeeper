# IAM policy (09/T5; S7, S9, S10, S18). Run with `conftest test --combine --namespace iam` over the gcp chart's output. Every binding the
# chart makes is an IAMPolicyMember (Config Connector) for a service account, with one of a short list of roles on the one resource
# it is for: no person, no group, no public member, no basic role, no role on the whole project but Cloud Trace. A new role is a
# decision, so it goes into `allowed_roles` with a test.
package iam

import rego.v1

import data.lk

members := [m | some m in lk.of_kind("IAMPolicyMember"); startswith(m.apiVersion, "iam.cnrm.cloud.google.com/")]

allowed_roles := {
	"roles/iam.workloadIdentityUser",
	"roles/storage.objectUser",
	"roles/storage.objectCreator",
	"roles/storage.objectViewer",
	"roles/cloudkms.cryptoKeyEncrypterDecrypter",
	"roles/cloudtrace.agent",
}

storage_roles := {"roles/storage.objectUser", "roles/storage.objectCreator", "roles/storage.objectViewer"}

member(m) := object.get(m.spec, "member", "")

role(m) := object.get(m.spec, "role", "")

ref_kind(m) := object.get(m.spec, ["resourceRef", "kind"], "")

# --- who ----------------------------------------------------------------------------------------------------------------------

deny contains msg if {
	some m in members
	member(m) == ""
	msg := sprintf("%s: it names no member (memberFrom is not used here, so that the member can be read) (S18)", [lk.who(m)])
}

deny contains msg if {
	some m in members
	member(m) in {"allUsers", "allAuthenticatedUsers"}
	msg := sprintf("%s: the member is %s, which is everyone (S10)", [lk.who(m), member(m)])
}

deny contains msg if {
	some m in members
	member(m) != ""
	not member(m) in {"allUsers", "allAuthenticatedUsers"}
	not startswith(member(m), "serviceAccount:")
	msg := sprintf("%s: the member %q is not a service account; this chart binds service accounts only (S7)", [lk.who(m), member(m)])
}

# --- which roles --------------------------------------------------------------------------------------------------------------

deny contains msg if {
	some m in members
	not role(m) in allowed_roles
	msg := sprintf("%s: the role %q is not one of the roles this chart grants, %v (S7)", [lk.who(m), role(m), sort(allowed_roles)])
}

# --- where each role goes, and to whom -------------------------------------------------------------------------------------------

deny contains msg if {
	some m in members
	role(m) in storage_roles
	ref_kind(m) != "StorageBucket"
	msg := sprintf("%s: %s is granted on a StorageBucket, never on %q: a role on the project reaches every bucket (S7)", [lk.who(m), role(m), ref_kind(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/cloudtrace.agent"
	ref_kind(m) != "Project"
	msg := sprintf("%s: roles/cloudtrace.agent is granted on the project, not on %q (Cloud Trace has no smaller resource)", [lk.who(m), ref_kind(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
	ref_kind(m) != "KMSCryptoKey"
	msg := sprintf("%s: roles/cloudkms.cryptoKeyEncrypterDecrypter is granted on a KMSCryptoKey, never on %q: a role on the key ring or the project reaches every key (S10)", [lk.who(m), ref_kind(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/iam.workloadIdentityUser"
	ref_kind(m) != "IAMServiceAccount"
	msg := sprintf("%s: roles/iam.workloadIdentityUser is granted on an IAMServiceAccount, never on %q (S5)", [lk.who(m), ref_kind(m)])
}

gsa(name) := sprintf("serviceAccount:%s@", [name])

hub_member(m) if startswith(member(m), gsa("lanekeeper-hub"))

db_member(m) if startswith(member(m), gsa("lanekeeper-db"))

deny contains msg if {
	some m in members
	role(m) == "roles/cloudtrace.agent"
	not hub_member(m)
	msg := sprintf("%s: only the hub's service account writes traces, and the member is %q", [lk.who(m), member(m)])
}

# Workload Identity: the member is a Kubernetes service account of the project's own pool, named in full: one namespace, one name,
# and not the one every pod without a name gets.
wi_member := `^serviceAccount:([a-z][a-z0-9-]+)\.svc\.id\.goog\[([a-z0-9]([a-z0-9-]*[a-z0-9])?)/([a-z0-9]([a-z0-9.-]*[a-z0-9])?)\]$`

deny contains msg if {
	some m in members
	role(m) == "roles/iam.workloadIdentityUser"
	not regex.match(wi_member, member(m))
	msg := sprintf("%s: the member %q is not one Kubernetes service account of a Workload Identity pool (PROJECT.svc.id.goog[NAMESPACE/NAME]) (S5)", [lk.who(m), member(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/iam.workloadIdentityUser"
	parts := regex.find_all_string_submatch_n(wi_member, member(m), 1)
	parts[0][4] == "default"
	msg := sprintf("%s: the member %q is the default service account of its namespace, which every pod without one uses (S5)", [lk.who(m), member(m)])
}

# The pool is the project's own: the project of the annotations on the same resource.
deny contains msg if {
	some m in members
	role(m) == "roles/iam.workloadIdentityUser"
	parts := regex.find_all_string_submatch_n(wi_member, member(m), 1)
	project := object.get(m.metadata, ["annotations", "cnrm.cloud.google.com/project-id"], "")
	parts[0][1] != project
	msg := sprintf("%s: the member %q is in the pool of %q, not in the pool of the project %q (S5)", [lk.who(m), member(m), parts[0][1], project])
}

# The keys of the buckets and disks are used by Google's own service agents, which are named by the project's number.
service_agent := `^serviceAccount:service-[0-9]+@(gs-project-accounts|compute-system)\.iam\.gserviceaccount\.com$`

deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
	startswith(member(m), "serviceAccount:")
	not regex.match(service_agent, member(m))
	msg := sprintf("%s: the member %q is not a Google-managed service agent (Cloud Storage or Compute Engine); a key is used by the service that encrypts with it, not by our own accounts (S10)", [lk.who(m), member(m)])
}

# --- the buckets: each is for one caller, with the least it needs -----------------------------------------------------------------

buckets_by_name := {b.metadata.name: b | some b in lk.of_kind("StorageBucket")}

bucket_purpose(m) := object.get(buckets_by_name[m.spec.resourceRef.name].metadata, ["labels", "lanekeeper.io/purpose"], "")

bucket_binding(m) if {
	role(m) in storage_roles
	ref_kind(m) == "StorageBucket"
}

deny contains msg if {
	some m in members
	bucket_binding(m)
	not buckets_by_name[object.get(m.spec.resourceRef, "name", "")]
	msg := sprintf("%s: the bucket %q is not in this output, so its purpose is unknown and the binding cannot be checked (S7)", [lk.who(m), object.get(m.spec.resourceRef, "name", "")])
}

# The audit checkpoints are written once and read back: nobody can delete or overwrite them, whatever the retention policy says.
deny contains msg if {
	some m in members
	bucket_binding(m)
	bucket_purpose(m) == "audit-checkpoint"
	not role(m) in {"roles/storage.objectCreator", "roles/storage.objectViewer"}
	msg := sprintf("%s: the audit checkpoint bucket may only create and read (objectCreator, objectViewer), and the role is %s (S9)", [lk.who(m), role(m)])
}

deny contains msg if {
	some m in members
	bucket_binding(m)
	bucket_purpose(m) == "audit-checkpoint"
	not hub_member(m)
	msg := sprintf("%s: only the hub's service account writes checkpoints, and the member is %q (S9)", [lk.who(m), member(m)])
}

# The backups hold the whole database: only the database's own service account, and it needs to read, create and delete (barman).
deny contains msg if {
	some m in members
	bucket_binding(m)
	bucket_purpose(m) == "backup"
	not db_member(m)
	msg := sprintf("%s: only the database's service account reaches the backups, and the member is %q (S10)", [lk.who(m), member(m)])
}

deny contains msg if {
	some m in members
	bucket_binding(m)
	bucket_purpose(m) == "backup"
	role(m) != "roles/storage.objectUser"
	msg := sprintf("%s: the backup bucket needs to read, create and delete objects (objectUser), and the role is %s; archiving would fail and the WAL would fill the disk (D40)", [lk.who(m), role(m)])
}

deny contains msg if {
	some m in members
	bucket_binding(m)
	bucket_purpose(m) == "docs"
	not hub_member(m)
	msg := sprintf("%s: only the hub's service account reaches the docs, and the member is %q (S7)", [lk.who(m), member(m)])
}
