# IAM policy (09/T5, 09/fix I1; S5, S7, S8, S9, S10, S18). Run with `conftest test --combine --namespace iam` over the gcp chart's output. Every
# binding the chart makes is an IAMPolicyMember (Config Connector) for a service account, with one of a short list of roles on the one
# resource it is for: no person, no group, no public member, no basic role, no role on the whole project but Cloud Trace. A new role is
# a decision, so it goes into `allowed_roles` with a test.
#
# The hub's keys and secrets (crates/hub/HARDENING.md): the hub's account signs with the agent CA, the replica CA and the audit checkpoint
# keys (signerVerifier, each on its key), uses the vault key to wrap the GitHub token data keys (cryptoKeyEncrypterDecrypter on that one key),
# and reads each secret it needs (secretAccessor, one binding on each secret). The keys and the secrets are in the same output, so that a
# binding is checked against what the key or the secret is for (kms.rego checks the objects).
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
	"roles/cloudkms.signerVerifier",
	"roles/secretmanager.secretAccessor",
	"roles/cloudtrace.agent",
}

storage_roles := {"roles/storage.objectUser", "roles/storage.objectCreator", "roles/storage.objectViewer"}

member(m) := object.get(m, ["spec", "member"], "")

role(m) := object.get(m, ["spec", "role"], "")

ref_kind(m) := object.get(m, ["spec", "resourceRef", "kind"], "")

ref_name(m) := object.get(m, ["spec", "resourceRef", "name"], "")

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

deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.signerVerifier"
	ref_kind(m) != "KMSCryptoKey"
	msg := sprintf("%s: roles/cloudkms.signerVerifier is granted on a KMSCryptoKey, never on %q: a role on the key ring or the project signs with every key in it (S5, S9)", [lk.who(m), ref_kind(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/secretmanager.secretAccessor"
	ref_kind(m) != "SecretManagerSecret"
	msg := sprintf("%s: roles/secretmanager.secretAccessor is granted on a SecretManagerSecret, never on %q: a role on the project reads every secret in it (S7)", [lk.who(m), ref_kind(m)])
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

# The keys of the buckets, disks and secrets are used by Google's own service agents, which are named by the project's number.
service_agent := `^serviceAccount:service-[0-9]+@(gs-project-accounts|compute-system|gcp-sa-secretmanager)\.iam\.gserviceaccount\.com$`

# The keys and the secrets that this output makes, by name: what a binding by name (not external) refers to.
keys_by_name := {k.metadata.name: k |
	some k in lk.of_kind("KMSCryptoKey")
	startswith(k.apiVersion, "kms.cnrm.cloud.google.com/")
}

secrets_by_name := {s.metadata.name: s |
	some s in lk.of_kind("SecretManagerSecret")
	startswith(s.apiVersion, "secretmanager.cnrm.cloud.google.com/")
}

# A reference names a resource of this output when it is by name (not external) and stays in the binding's own namespace.
local_ref(m) if {
	object.get(m, ["spec", "resourceRef", "external"], "") == ""
	object.get(m, ["spec", "resourceRef", "namespace"], object.get(m, ["metadata", "namespace"], "")) == object.get(m, ["metadata", "namespace"], "")
}

key_of(m) := keys_by_name[ref_name(m)] if local_ref(m)

secret_of(m) := secrets_by_name[ref_name(m)] if local_ref(m)

key_purpose(m) := object.get(key_of(m).metadata, ["labels", "lanekeeper.io/purpose"], "")

signing_purposes := {"agent-ca", "replica-ca", "audit-checkpoint-signing"}

# The one key of ours that an account of ours uses to encrypt: the vault key, by the hub, to wrap the data keys of the GitHub tokens (S8).
# A key that a Google service encrypts with (the CMEK of a bucket, a disk or a secret) is made by a person and named by `external`.
vault_use(m) if {
	hub_member(m)
	key_purpose(m) == "vault"
}

deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
	startswith(member(m), "serviceAccount:")
	not regex.match(service_agent, member(m))
	not vault_use(m)
	msg := sprintf("%s: the member %q is not a Google-managed service agent (Cloud Storage, Compute Engine or Secret Manager); a key is used by the service that encrypts with it, not by our own accounts, but for the hub's use of the vault key (S10)", [lk.who(m), member(m)])
}

# The hub wraps and unwraps with the vault key and with no other key: not a signing key, which it only signs with (S5, S8, S9).
deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
	ref_kind(m) == "KMSCryptoKey"
	hub_member(m)
	key_of(m)
	key_purpose(m) != "vault"
	msg := sprintf("%s: the hub's service account may use only the vault key to encrypt and decrypt, and this is the %q key (S5, S8, S9)", [lk.who(m), key_purpose(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
	ref_kind(m) == "KMSCryptoKey"
	hub_member(m)
	not key_of(m)
	msg := sprintf("%s: the key %q is not in this output, so the hub's use of it cannot be checked: the hub may use only the vault key (S8)", [lk.who(m), ref_name(m)])
}

# A Google service agent uses the customer-managed keys that a person made, not the keys made here for the hub.
deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
	ref_kind(m) == "KMSCryptoKey"
	regex.match(service_agent, member(m))
	key_of(m)
	msg := sprintf("%s: %q is a key made here for the hub; a Google service agent uses customer-managed keys that are made elsewhere and named by external, and a key made here is used by the hub only (S5, S8, S9)", [lk.who(m), ref_name(m)])
}

# --- the signing keys: the hub signs, and with the key for the purpose -------------------------------------------------------------

deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.signerVerifier"
	not hub_member(m)
	msg := sprintf("%s: only the hub's service account signs with these keys, and the member is %q (S5, S9)", [lk.who(m), member(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.signerVerifier"
	ref_kind(m) == "KMSCryptoKey"
	not key_of(m)
	msg := sprintf("%s: the key %q is not in this output, so its purpose is unknown and the binding cannot be checked (S5, S9)", [lk.who(m), ref_name(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/cloudkms.signerVerifier"
	ref_kind(m) == "KMSCryptoKey"
	key_of(m)
	not key_purpose(m) in signing_purposes
	msg := sprintf("%s: roles/cloudkms.signerVerifier is for the agent CA, the replica CA and the audit checkpoint keys only, and this is the %q key; the vault key wraps and unwraps and does not sign (S5, S8, S9)", [lk.who(m), key_purpose(m)])
}

# --- the secrets: the hub reads each one it needs ----------------------------------------------------------------------------------

deny contains msg if {
	some m in members
	role(m) == "roles/secretmanager.secretAccessor"
	not hub_member(m)
	msg := sprintf("%s: only the hub's service account reads these secrets, and the member is %q (S7)", [lk.who(m), member(m)])
}

deny contains msg if {
	some m in members
	role(m) == "roles/secretmanager.secretAccessor"
	ref_kind(m) == "SecretManagerSecret"
	not secret_of(m)
	msg := sprintf("%s: the secret %q is not in this output, so the binding cannot be checked against the secrets the chart makes (S7)", [lk.who(m), ref_name(m)])
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
