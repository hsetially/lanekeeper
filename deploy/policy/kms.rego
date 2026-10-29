# Keys and secrets policy (09/fix, I1; S5, S7, S8, S9, S10). Run with `conftest test --combine --namespace kms` over the gcp chart's output.
# The hub holds no private key and no secret value (crates/hub/HARDENING.md); it asks Google for both, with Workload Identity. This chart
# makes what it asks for, and nothing more:
#
#   - a key ring and four Cloud KMS keys. The agent CA, the replica CA and the audit checkpoint keys are asymmetric signing keys on the
#     P-256 curve; the vault key is a symmetric key that wraps the data keys of the GitHub tokens. Each is used for one thing (the
#     bindings, in iam.rego, give the hub's account the one role that thing needs on that key), the key material is made by Cloud KMS
#     and never imported, and removing a resource from Git never destroys a key.
#   - the Secret Manager secrets the hub reads: containers, never values. A value is put in by a person (the runbook), and no
#     SecretManagerSecretVersion is rendered. What this file holds on a container, so that a value has no field to hide in:
#       * the kind SecretManagerSecretVersion, of any group, is refused;
#       * the top level is apiVersion, kind, metadata and spec; the spec is replication; the metadata is name, namespace, labels and annotations;
#       * the labels and annotations are the keys the chart writes, and their values are the chart's own (the standard labels fixed, helm.sh/chart
#         a bounded version, the project and the namespace the key ring's, the keep annotations and the fallback marker compared whole);
#       * the replication is userManaged, replicas, and each replica a location (the key ring's) and customerManagedEncryption, kmsKeyRef, external:
#         at every one of these levels a key of another name is refused, and so is a level that is not a mapping (or replicas that is not a list).
#     What it does not hold: the NAME of a container (render.sh contract compares the names with the hub chart's), and the text of the external
#     key (a Cloud KMS key name is held to its shape and to a grant for the Secret Manager agent in this output; it is the one input, secrets.cmekKey,
#     that a person sets). The key ring and the keys are not held to a list of fields or of metadata keys, only to the rules below: a key
#     is a name and settings, and holds no secret, but free text in one of its fields is not refused.
package kms

import rego.v1

import data.lk

rings := [r |
	some r in lk.of_kind("KMSKeyRing")
	startswith(r.apiVersion, "kms.cnrm.cloud.google.com/")
]

keys := [k |
	some k in lk.of_kind("KMSCryptoKey")
	startswith(k.apiVersion, "kms.cnrm.cloud.google.com/")
]

secrets := [s |
	some s in lk.of_kind("SecretManagerSecret")
	startswith(s.apiVersion, "secretmanager.cnrm.cloud.google.com/")
]

signing_purposes := {"agent-ca", "replica-ca", "audit-checkpoint-signing"}

key_purposes := {"agent-ca", "replica-ca", "audit-checkpoint-signing", "vault"}

environments := {"staging", "prod"}

key_pattern := `^projects/[^/]+/locations/[^/]+/keyRings/[^/]+/cryptoKeys/[^/]+$`

purpose(d) := object.get(d.metadata, ["labels", "lanekeeper.io/purpose"], "")

environment(d) := object.get(d.metadata, ["labels", "lanekeeper.io/environment"], "")

# "the agent-ca key", "a key of no known purpose": what a message calls a key by what it is for.
what(k) := sprintf("%s (the %s key)", [lk.who(k), purpose(k)]) if purpose(k) in key_purposes

what(k) := sprintf("%s (a key of no known purpose)", [lk.who(k)]) if not purpose(k) in key_purposes

# --- nothing that holds a value --------------------------------------------------------------------------------------------------------

# A secret's value is put in by a person and is never in Git, in a chart or in a policy's input (S7). The kind is refused whatever its group.
deny contains msg if {
	some v in lk.of_kind("SecretManagerSecretVersion")
	msg := sprintf("%s: a secret version holds a value, and this chart makes containers, never a value; a person puts the value in (S7)", [lk.who(v)])
}

# --- what is kept: nothing here is destroyed by a sync or by deleting its Kubernetes resource ---------------------------------------------

kept := array.concat(rings, array.concat(keys, secrets))

deny contains msg if {
	some d in kept
	object.get(d.metadata, ["annotations", "cnrm.cloud.google.com/deletion-policy"], "") != "abandon"
	msg := sprintf("%s: the annotation cnrm.cloud.google.com/deletion-policy: abandon is missing; removing the resource could destroy the key or the secret and every version in it (S5, S8, S9)", [lk.who(d)])
}

deny contains msg if {
	some d in kept
	object.get(d.metadata, ["annotations", "argocd.argoproj.io/sync-options"], "") != "Prune=false"
	msg := sprintf("%s: the annotation argocd.argoproj.io/sync-options: Prune=false is missing; a sync could prune the resource (S5, S8, S9)", [lk.who(d)])
}

# --- the key ring --------------------------------------------------------------------------------------------------------------------

deny contains msg if {
	some r in rings
	object.get(r, ["spec", "location"], "") == ""
	msg := sprintf("%s: it names no location", [lk.who(r)])
}

ring_names := {r.metadata.name | some r in rings}

# One ring, and it is in the output. The chart makes one; the project, the namespace and the location of the secrets are compared with it (below),
# and with two rings that differ in one of them every secret would be refused against the other, which says nothing about the secret.
deny contains msg if {
	count(rings) + count(keys) + count(secrets) > 0
	count(rings) != 1
	msg := sprintf("there must be exactly one KMSKeyRing in the output, and there are %d; the project, the namespace and the location of the secrets are compared with the key ring's (S5, S7)", [count(rings)])
}

# --- the keys: one for each thing they are for ----------------------------------------------------------------------------------------

deny contains msg if {
	some k in keys
	not purpose(k) in key_purposes
	msg := sprintf("%s: the label lanekeeper.io/purpose must be one of %v, and is %q; a binding is checked against what its key is for (S5, S8, S9)", [lk.who(k), sort(key_purposes), purpose(k)])
}

deny contains msg if {
	some k in keys
	not environment(k) in environments
	msg := sprintf("%s: the label lanekeeper.io/environment must be one of %v, and is %q; the protection level of a prod key is checked against it (S5)", [lk.who(k), sort(environments), environment(k)])
}

# Exactly one key for each purpose, wherever keys are rendered at all: the two CAs are two keys (an agent's certificate must never be a
# replica's, S5), and the keys are not shared between purposes.
deny contains msg if {
	count(keys) > 0
	some p in key_purposes
	count([k | some k in keys; purpose(k) == p]) != 1
	msg := sprintf("there must be exactly one %s key, and there are %d (S5, S8, S9)", [p, count([k | some k in keys; purpose(k) == p])])
}

deny contains msg if {
	some k in keys
	count([x | some x in keys; x.metadata.name == k.metadata.name]) > 1
	msg := sprintf("two KMSCryptoKey resources are named %q: one key cannot be for two things (S5)", [k.metadata.name])
}

deny contains msg if {
	some k in keys
	not object.get(k, ["spec", "keyRingRef", "name"], "") in ring_names
	msg := sprintf("%s: the key ring %q is not in this output; a key is made in the ring this chart makes, named by keyRingRef.name", [what(k), object.get(k, ["spec", "keyRingRef", "name"], "")])
}

# The signing keys (S5, S9). Cloud KMS signs and the hub never holds the key. The algorithm is the one the hub's code uses for the CAs and
# the checkpoints (crates/hub/HARDENING.md: EC_SIGN_P256_SHA256); the Config Connector default for a key without a purpose is
# ENCRYPT_DECRYPT, so the purpose is written out.
deny contains msg if {
	some k in keys
	purpose(k) in signing_purposes
	object.get(k, ["spec", "purpose"], "") != "ASYMMETRIC_SIGN"
	msg := sprintf("%s: spec.purpose is %q; it must be ASYMMETRIC_SIGN (the Config Connector default is ENCRYPT_DECRYPT) (S5, S9)", [what(k), object.get(k, ["spec", "purpose"], "")])
}

deny contains msg if {
	some k in keys
	purpose(k) in signing_purposes
	object.get(k, ["spec", "versionTemplate", "algorithm"], "") != "EC_SIGN_P256_SHA256"
	msg := sprintf("%s: versionTemplate.algorithm is %q; the hub signs with EC_SIGN_P256_SHA256 and rejects a key of another algorithm (S5, S9)", [what(k), object.get(k, ["spec", "versionTemplate", "algorithm"], "")])
}

# The hub pins a key version (LK_*_KMS_REF names one), so a new primary version changes nothing it uses. Rotating a signing key is a
# person's step, with a new certificate for a CA and the old version kept for verifying a checkpoint.
deny contains msg if {
	some k in keys
	purpose(k) in signing_purposes
	object.get(k, ["spec", "rotationPeriod"], null) != null
	msg := sprintf("%s: it has a rotationPeriod; the hub pins a key version and a new primary version changes nothing it uses, so a signing key is rotated by a person (S5, S9)", [what(k)])
}

# The vault key (S8): symmetric, for wrapping the data key of each stored GitHub token. The hub names the key and not a version, so KMS
# picks the primary version and a rotation changes nothing the hub is told.
deny contains msg if {
	some k in keys
	purpose(k) == "vault"
	object.get(k, ["spec", "purpose"], "") != "ENCRYPT_DECRYPT"
	msg := sprintf("%s: spec.purpose is %q; it must be ENCRYPT_DECRYPT (S8)", [what(k), object.get(k, ["spec", "purpose"], "")])
}

deny contains msg if {
	some k in keys
	purpose(k) == "vault"
	object.get(k, ["spec", "versionTemplate", "algorithm"], "") != "GOOGLE_SYMMETRIC_ENCRYPTION"
	msg := sprintf("%s: versionTemplate.algorithm is %q; it must be GOOGLE_SYMMETRIC_ENCRYPTION (S8)", [what(k), object.get(k, ["spec", "versionTemplate", "algorithm"], "")])
}

# Where the key material lives (S5: "the CA key lives in Cloud KMS and is never exported"). Both levels keep it in Cloud KMS; the level is
# immutable, so it is written out and not left to the default. External keys are not offered. A human relaxes the prod rule, in
# docs/decisions.md.
protection_levels := {"HSM", "SOFTWARE"}

deny contains msg if {
	some k in keys
	not object.get(k, ["spec", "versionTemplate", "protectionLevel"], "") in protection_levels
	msg := sprintf("%s: versionTemplate.protectionLevel is %q; it must be one of %v, written out because it cannot be changed afterwards (S5)", [what(k), object.get(k, ["spec", "versionTemplate", "protectionLevel"], ""), sort(protection_levels)])
}

deny contains msg if {
	some k in keys
	environment(k) == "prod"
	object.get(k, ["spec", "versionTemplate", "protectionLevel"], "") == "SOFTWARE"
	msg := sprintf("%s: the protection level is SOFTWARE; the keys of prod are HSM in prod (S5)", [what(k)])
}

# The material is made by Cloud KMS. An imported key was known outside it first, and a key without a first version has no key to read
# off, so `hub ca-init` has nothing to certify.
deny contains msg if {
	some k in keys
	object.get(k, ["spec", "importOnly"], false) == true
	msg := sprintf("%s: importOnly is true; the key material must be made by Cloud KMS, not imported, because an imported key was known outside it (S5)", [what(k)])
}

deny contains msg if {
	some k in keys
	object.get(k, ["spec", "skipInitialVersionCreation"], false) == true
	msg := sprintf("%s: skipInitialVersionCreation is true; the key has no version to name in the hub's settings or to make a CA certificate for", [what(k)])
}

# --- the secrets: containers, replicated into the chart's location, encrypted with a key of ours ------------------------------------------

deny contains msg if {
	some s in secrets
	purpose(s) != "secret"
	msg := sprintf("%s: the label lanekeeper.io/purpose must be secret, and is %q", [lk.who(s), purpose(s)])
}

deny contains msg if {
	some s in secrets
	not environment(s) in environments
	msg := sprintf("%s: the label lanekeeper.io/environment must be one of %v, and is %q", [lk.who(s), sort(environments), environment(s)])
}

# A container has a replication policy and nothing else. Every other field is a way for a value, a lifetime or a notification to get in:
# a ttl or an expireTime deletes the secret on a date the hub does not know, annotations and labels (of the spec) are readable by anyone
# who can list secrets, a rotation or a topic starts something that nobody asked for. The metadata is held to its own list, below.
allowed_secret_fields := {"replication"}

deny contains msg if {
	some s in secrets
	some field, _ in s.spec
	not field in allowed_secret_fields
	msg := sprintf("%s: spec.%s is set; a secret container is its replication and nothing else: no value, no ttl or expiry, no rotation, no topic, no annotation (S7)", [lk.who(s), field])
}

# The metadata of a container is a way in as well: labels and annotations are free-form strings, readable by anyone who can list secrets in
# the namespace, and nothing else reads them. Only the keys the chart writes are allowed (the standard labels and the two the policies read;
# the project, the two keep annotations, and the marker of the fallback), so a key that is not one of them is refused whatever its name.
allowed_secret_labels := {
	"app.kubernetes.io/name", "app.kubernetes.io/instance", "app.kubernetes.io/part-of", "app.kubernetes.io/managed-by",
	"helm.sh/chart", "lanekeeper.io/purpose", "lanekeeper.io/environment",
}

allowed_secret_annotations := {
	"cnrm.cloud.google.com/project-id", "cnrm.cloud.google.com/deletion-policy", "argocd.argoproj.io/sync-options", "lanekeeper.io/cmek",
}

deny contains msg if {
	some s in secrets
	some key, _ in object.get(s.metadata, "labels", {})
	not key in allowed_secret_labels
	msg := sprintf("%s: metadata.labels.%s is not a label a secret container carries (%v); a label is readable by anyone who can list secrets, and is no place for a value (S7)", [lk.who(s), key, sort(allowed_secret_labels)])
}

deny contains msg if {
	some s in secrets
	some key, _ in object.get(s.metadata, "annotations", {})
	not key in allowed_secret_annotations
	msg := sprintf("%s: metadata.annotations.%s is not an annotation a secret container carries (%v); an annotation is readable by anyone who can list secrets, and is no place for a value (S7)", [lk.who(s), key, sort(allowed_secret_annotations)])
}

# The VALUES of the allowed keys are held as well: a key from the list can carry a value as easily as one that is not on it. The standard labels
# are the chart's own, fixed: the release is the name of the Argo CD Application (argocd.rego holds helm.releaseName to it, and the gcp
# Application is lanekeeper-gcp), the chart label is its name and a version, and the project is a project id and the key ring's. The
# deletion-policy, the sync-options and the fallback marker are compared whole (rules above and below). Nothing else is in the metadata
# (a finalizer, a generateName or an owner reference is a free string that the chart does not write) or outside it and the spec.
fixed_secret_labels := {
	"app.kubernetes.io/name": "lanekeeper-gcp",
	"app.kubernetes.io/instance": "lanekeeper-gcp",
	"app.kubernetes.io/part-of": "lanekeeper",
	"app.kubernetes.io/managed-by": "Helm",
}

chart_label_pattern := `^lanekeeper-gcp-[0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,4}(-(alpha|beta|rc)\.[0-9]{1,4})?$`

project_id_pattern := `^[a-z][a-z0-9-]{4,28}[a-z0-9]$`

allowed_secret_metadata := {"name", "namespace", "labels", "annotations"}

allowed_secret_fields_outside_metadata := {"apiVersion", "kind", "metadata", "spec"}

deny contains msg if {
	some s in secrets
	some key, want in fixed_secret_labels
	got := s.metadata.labels[key]
	got != want
	msg := sprintf("%s: the label %s is %q and the chart writes %q; a label is readable by anyone who can list secrets, and its value is no place for anything else (S7)", [lk.who(s), key, got, want])
}

deny contains msg if {
	some s in secrets
	got := s.metadata.labels["helm.sh/chart"]
	not is_string(got)
	msg := sprintf("%s: the label helm.sh/chart is not text (S7)", [lk.who(s)])
}

deny contains msg if {
	some s in secrets
	got := s.metadata.labels["helm.sh/chart"]
	is_string(got)
	not regex.match(chart_label_pattern, got)
	msg := sprintf("%s: the label helm.sh/chart is %q; the chart writes lanekeeper-gcp- and its version: three numbers of at most four digits, and at most a prerelease (alpha, beta or rc, a dot and one number of at most four digits), nothing else (S7)", [lk.who(s), got])
}

deny contains msg if {
	some s in secrets
	got := s.metadata.annotations["cnrm.cloud.google.com/project-id"]
	not is_string(got)
	msg := sprintf("%s: the annotation cnrm.cloud.google.com/project-id is not text (S7)", [lk.who(s)])
}

deny contains msg if {
	some s in secrets
	got := s.metadata.annotations["cnrm.cloud.google.com/project-id"]
	is_string(got)
	not regex.match(project_id_pattern, got)
	msg := sprintf("%s: the annotation cnrm.cloud.google.com/project-id is %q, which is not a Google project id (6 to 30 lower case letters, digits and hyphens, starting with a letter); an annotation is readable by anyone who can list secrets (S7)", [lk.who(s), got])
}

# The shape of a project id is not enough (a short word has it): the secrets are in the project of the key ring that the same chart makes, and in its
# namespace.
deny contains msg if {
	some s in secrets
	some r in rings
	got := s.metadata.annotations["cnrm.cloud.google.com/project-id"]
	want := object.get(r.metadata, ["annotations", "cnrm.cloud.google.com/project-id"], "")
	got != want
	msg := sprintf("%s: the annotation cnrm.cloud.google.com/project-id is %q, and the key ring is in the project %q; one chart, one project (S7)", [lk.who(s), got, want])
}

deny contains msg if {
	some s in secrets
	some r in rings
	object.get(s.metadata, "namespace", "") != object.get(r.metadata, "namespace", "")
	msg := sprintf("%s: the namespace is %q, and the key ring's is %q; one chart, one namespace (S7)", [lk.who(s), object.get(s.metadata, "namespace", ""), object.get(r.metadata, "namespace", "")])
}

deny contains msg if {
	some s in secrets
	some key, _ in s.metadata
	not key in allowed_secret_metadata
	msg := sprintf("%s: metadata.%s is set; a secret container's metadata is %v and nothing else, because anything else is a free string that anyone who can list secrets can read (S7)", [lk.who(s), key, sort(allowed_secret_metadata)])
}

deny contains msg if {
	some s in secrets
	some key, _ in s
	not key in allowed_secret_fields_outside_metadata
	msg := sprintf("%s: the top-level field %s is set; a secret container is %v and nothing else (S7)", [lk.who(s), key, sort(allowed_secret_fields_outside_metadata)])
}

deny contains msg if {
	some s in secrets
	marker := object.get(s.metadata, ["annotations", "lanekeeper.io/cmek"], "fallback")
	marker != "fallback"
	msg := sprintf("%s: the annotation lanekeeper.io/cmek must be fallback or absent, and is %q (S7, S10)", [lk.who(s), marker])
}

# The replicas, when they are a list (a string, a mapping or a number is none: the rule below says so, and the count is of a list only).
replicas_value(s) := object.get(s, ["spec", "replication", "userManaged", "replicas"], [])

replicas(s) := replicas_value(s) if is_array(replicas_value(s))

replicas(s) := [] if not is_array(replicas_value(s))

deny contains msg if {
	some s in secrets
	count(replicas(s)) == 0
	msg := sprintf("%s: replication.userManaged.replicas is empty or missing; a secret is replicated into the chart's location, with a key of ours in that location (S10)", [lk.who(s)])
}

deny contains msg if {
	some s in secrets
	not is_array(replicas_value(s))
	msg := sprintf("%s: spec.replication.userManaged.replicas is not a list (S7)", [lk.who(s)])
}

deny contains msg if {
	some s in secrets
	some r in replicas(s)
	object.get(r, "location", "") == ""
	msg := sprintf("%s: a replica names no location", [lk.who(s)])
}

# The location of a replica is the key ring's: the chart writes one value for both, and any other string in this place is a string that nothing
# else reads (a replica in a region that the chart did not choose, or a note). Where the ring is not in the output the rule above asks for it.
deny contains msg if {
	some s in secrets
	some r in replicas(s)
	some ring in rings
	got := object.get(r, "location", "")
	got != ""
	want := object.get(ring, ["spec", "location"], "")
	got != want
	msg := sprintf("%s: a replica's location is %v and the key ring's is %v; the chart replicates a secret into the one location of its key ring (S10)", [lk.who(s), got, want])
}

# What a container's replication may hold: the keys the chart writes at each level, and no other. A key of another name (a note, a label, a
# name) is a place for a value that nothing reads, as one in the metadata is. The levels are named by where they are.
replication_keys := {
	"spec.replication": {"userManaged"},
	"spec.replication.userManaged": {"replicas"},
	"spec.replication.userManaged.replicas[]": {"location", "customerManagedEncryption"},
	"spec.replication.userManaged.replicas[].customerManagedEncryption": {"kmsKeyRef"},
	"spec.replication.userManaged.replicas[].customerManagedEncryption.kmsKeyRef": {"external"},
}

# The nodes of the replication that are there, each with where it is. A node that is not a mapping is refused by its own rule (below), and the
# levels under it are read only when it is one.
replication_nodes(s) := array.concat(
	[{"at": "spec.replication", "node": n} |
		n := object.get(s, ["spec", "replication"], null)
		n != null
	],
	array.concat(
		[{"at": "spec.replication.userManaged", "node": n} |
			n := object.get(s, ["spec", "replication", "userManaged"], null)
			n != null
		],
		array.concat(
			[{"at": "spec.replication.userManaged.replicas[]", "node": r} | some r in replicas(s)],
			array.concat(
				[{"at": "spec.replication.userManaged.replicas[].customerManagedEncryption", "node": n} |
					some r in replicas(s)
					n := object.get(r, "customerManagedEncryption", null)
					n != null
				],
				[{"at": "spec.replication.userManaged.replicas[].customerManagedEncryption.kmsKeyRef", "node": n} |
					some r in replicas(s)
					n := object.get(r, ["customerManagedEncryption", "kmsKeyRef"], null)
					n != null
				],
			),
		),
	),
)

deny contains msg if {
	some s in secrets
	some n in replication_nodes(s)
	not is_object(n.node)
	msg := sprintf("%s: %s is not a mapping (S7)", [lk.who(s), n.at])
}

deny contains msg if {
	some s in secrets
	some n in replication_nodes(s)
	is_object(n.node)
	some key, _ in n.node
	not key in replication_keys[n.at]
	msg := sprintf("%s: %s.%s is set; the replication of a secret container is %v at this level and nothing else, because any other key is a place for a value that nothing reads (S7)", [lk.who(s), n.at, key, sort(replication_keys[n.at])])
}

secret_key(r) := object.get(r, ["customerManagedEncryption", "kmsKeyRef", "external"], "")

secret_fallback(s) if object.get(s.metadata, ["annotations", "lanekeeper.io/cmek"], "") == "fallback"

deny contains msg if {
	some s in secrets
	some r in replicas(s)
	secret_key(r) == ""
	not secret_fallback(s)
	msg := sprintf("%s: a replica names no customer-managed key (customerManagedEncryption.kmsKeyRef.external) and the secret is not marked as the fallback (S10)", [lk.who(s)])
}

deny contains msg if {
	some s in secrets
	some r in replicas(s)
	secret_key(r) != ""
	not regex.match(key_pattern, secret_key(r))
	msg := sprintf("%s: %q is not a Cloud KMS key (projects/P/locations/L/keyRings/R/cryptoKeys/K) (S10)", [lk.who(s), secret_key(r)])
}

warn contains msg if {
	some s in secrets
	some r in replicas(s)
	secret_key(r) == ""
	secret_fallback(s)
	msg := sprintf("%s: Google-managed encryption, not a customer-managed key (S10, Q29): set secrets.cmekKey", [lk.who(s)])
}

# Secret Manager encrypts a payload with the key through its own service agent (created by Google for the project, named by its number):
# another IAMPolicyMember in the output grants that agent cryptoKeyEncrypterDecrypter on exactly that key.
secretmanager_agent(member) if regex.match(`^serviceAccount:service-[0-9]+@gcp-sa-secretmanager\.iam\.gserviceaccount\.com$`, member)

key_grants(k) := [m |
	some m in lk.of_kind("IAMPolicyMember")
	m.spec.role == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
	object.get(m, ["spec", "resourceRef", "kind"], "") == "KMSCryptoKey"
	object.get(m, ["spec", "resourceRef", "external"], "") == k
	secretmanager_agent(object.get(m, ["spec", "member"], ""))
]

secret_keys := {secret_key(r) |
	some s in secrets
	some r in replicas(s)
	secret_key(r) != ""
	regex.match(key_pattern, secret_key(r))
}

deny contains msg if {
	some k in secret_keys
	count(key_grants(k)) == 0
	msg := sprintf("no IAMPolicyMember grants the Secret Manager service agent cryptoKeyEncrypterDecrypter on %s, so a secret encrypted with it could not be written (S10)", [k])
}
