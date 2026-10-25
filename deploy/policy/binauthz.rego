# Binary Authorization policy (09/T6; S19, S20, plan 09 A5). Run with `conftest test --combine --namespace binauthz` over the gcp chart's
# output. The project has one admission policy, and it says one thing: an image runs only with an attestation from the attestor that
# trusts the CI signing key, a Cloud KMS key that cannot be exported. Anything that would let an image of ours skip that check is
# refused: a rule for a cluster or a namespace (they override the default), a name pattern that covers our images, an attestor that
# trusts a key kept somewhere. Whether the policy blocks or only audits is a value (Q24): `warn` says so while it audits.
package binauthz

import rego.v1

import data.lk

policies := [p |
	some p in lk.of_kind("BinaryAuthorizationPolicy")
	startswith(p.apiVersion, "binaryauthorization.cnrm.cloud.google.com/")
]

attestors := [a |
	some a in lk.of_kind("BinaryAuthorizationAttestor")
	startswith(a.apiVersion, "binaryauthorization.cnrm.cloud.google.com/")
]

notes := [n |
	some n in lk.of_kind("ContainerAnalysisNote")
	startswith(n.apiVersion, "containeranalysis.cnrm.cloud.google.com/")
]

attestor_names := {a.metadata.name | some a in attestors}

note_names := {n.metadata.name | some n in notes}

admission_resources := array.concat(policies, array.concat(attestors, notes))

default_rule(p) := object.get(p.spec, "defaultAdmissionRule", {})

# --- one policy per project, and an attestor needs it ------------------------------------------------------------------------

deny contains msg if {
	count(policies) > 1
	msg := sprintf("the project has exactly one admission policy, and this output has %d BinaryAuthorizationPolicy resources (S20)", [count(policies)])
}

deny contains msg if {
	some a in attestors
	count(policies) == 0
	msg := sprintf("%s: an attestor without a BinaryAuthorizationPolicy that requires it checks nothing (S20)", [lk.who(a)])
}

# --- the default rule: every image needs an attestation ----------------------------------------------------------------------

deny contains msg if {
	some p in policies
	mode := object.get(default_rule(p), "evaluationMode", "")
	mode != "REQUIRE_ATTESTATION"
	msg := sprintf("%s: defaultAdmissionRule.evaluationMode is %q; it must be REQUIRE_ATTESTATION, or an image without an attestation is admitted (S20)", [lk.who(p), mode])
}

enforcement_modes := {"DRYRUN_AUDIT_LOG_ONLY", "ENFORCED_BLOCK_AND_AUDIT_LOG"}

deny contains msg if {
	some p in policies
	mode := object.get(default_rule(p), "enforcementMode", "")
	not mode in enforcement_modes
	msg := sprintf("%s: defaultAdmissionRule.enforcementMode is %q; it must be one of %v (plan 09 A5)", [lk.who(p), mode, sort(enforcement_modes)])
}

deny contains msg if {
	some p in policies
	count(object.get(default_rule(p), "requireAttestationsBy", [])) == 0
	msg := sprintf("%s: defaultAdmissionRule.requireAttestationsBy is empty, so no attestor is required (S20)", [lk.who(p)])
}

deny contains msg if {
	some p in policies
	some ref in object.get(default_rule(p), "requireAttestationsBy", [])
	name := object.get(ref, "name", "")
	not name in attestor_names
	msg := sprintf("%s: requireAttestationsBy names %q, which is not an attestor in this output; the attestor that trusts the CI key is made by this chart and no other is required (S20)", [lk.who(p), name])
}

# The per-cluster, per-namespace, per-service-account and per-Istio-identity rules each override the default rule for what they
# match, so one with ALWAYS_ALLOW opens a door the default rule closes. This policy has the default rule only.
other_rules := {
	"clusterAdmissionRules",
	"kubernetesNamespaceAdmissionRules",
	"kubernetesServiceAccountAdmissionRules",
	"istioServiceIdentityAdmissionRules",
}

deny contains msg if {
	some p in policies
	some field in other_rules
	count(object.get(p.spec, field, {})) > 0
	msg := sprintf("%s: %s overrides the default rule for what it matches; this policy has the default rule only (S20)", [lk.who(p), field])
}

# Google's own images (the cluster's system components) are let through by Google's global policy; without it a system image would
# need our attestation, and the cluster would stop once the policy enforces.
deny contains msg if {
	some p in policies
	object.get(p.spec, "globalPolicyEvaluationMode", "") != "ENABLE"
	msg := sprintf("%s: globalPolicyEvaluationMode must be ENABLE, so that Google's system images are admitted by Google's policy and not by a pattern of ours", [lk.who(p)])
}

# --- images that skip the check: never ours ----------------------------------------------------------------------------------

our_images := {
	"ghcr.io/hsetially/lanekeeper/hub",
	"ghcr.io/hsetially/lanekeeper/agent",
}

patterns(p) := [x.namePattern | some x in object.get(p.spec, "admissionWhitelistPatterns", [])]

# A pattern is `registry/path`, with at most one `*` and only at the end.
well_formed(pattern) if {
	pattern != ""
	contains(pattern, "/")
	not contains(trim_suffix(pattern, "*"), "*")
}

covers(pattern, image) if {
	endswith(pattern, "*")
	startswith(image, trim_suffix(pattern, "*"))
}

covers(pattern, image) if pattern == image

deny contains msg if {
	some p in policies
	some pattern in patterns(p)
	not well_formed(pattern)
	msg := sprintf("%s: admissionWhitelistPatterns has %q; a pattern is registry/path with at most one * at the end, and a bare * would skip the check for every image (S20)", [lk.who(p), pattern])
}

deny contains msg if {
	some p in policies
	some pattern in patterns(p)
	well_formed(pattern)
	some image in our_images
	covers(pattern, image)
	msg := sprintf("%s: the pattern %q in admissionWhitelistPatterns covers %s, an image of ours, which would run without an attestation (S20)", [lk.who(p), pattern, image])
}

# --- the project -------------------------------------------------------------------------------------------------------------

project_ref(r) := object.get(r.spec, ["projectRef", "external"], "")

deny contains msg if {
	some r in array.concat(policies, attestors)
	want := sprintf("projects/%s", [object.get(r.metadata, ["annotations", "cnrm.cloud.google.com/project-id"], "")])
	project_ref(r) != want
	msg := sprintf("%s: spec.projectRef.external is %q, and the project of its annotation is %q; the policy and the attestor belong to one project", [lk.who(r), project_ref(r), want])
}

# --- removing a resource from Git must not remove the policy from the project ------------------------------------------------------
# Deleting the policy resets the project to the default policy, which admits everything. Config Connector leaves what it made alone
# when it sees `abandon`, and Argo CD does not prune what it sees `Prune=false` on.

deny contains msg if {
	some r in admission_resources
	object.get(r.metadata, ["annotations", "cnrm.cloud.google.com/deletion-policy"], "") != "abandon"
	msg := sprintf("%s: the annotation cnrm.cloud.google.com/deletion-policy: abandon is missing; deleting this resource from Git would delete it from the project, and without the policy every image is admitted (S20)", [lk.who(r)])
}

deny contains msg if {
	some r in admission_resources
	object.get(r.metadata, ["annotations", "argocd.argoproj.io/sync-options"], "") != "Prune=false"
	msg := sprintf("%s: the annotation argocd.argoproj.io/sync-options: Prune=false is missing; a sync that prunes it would delete it from the project (S20)", [lk.who(r)])
}

# --- the attestor trusts one Cloud KMS key, and nothing else -----------------------------------------------------------------

attestor_keys(a) := object.get(a.spec, ["userOwnedDrydockNote", "publicKeys"], [])

deny contains msg if {
	some a in attestors
	count(attestor_keys(a)) == 0
	msg := sprintf("%s: the attestor has no public key, so no signature can verify against it (S20)", [lk.who(a)])
}

deny contains msg if {
	some a in attestors
	some i, k in attestor_keys(a)
	object.get(k, "asciiArmoredPgpPublicKey", "") != ""
	msg := sprintf("%s: public key %d is a PGP key; the attestor trusts the Cloud KMS key of CI only, as a PKIX public key (S20)", [lk.who(a), i])
}

pem(k) := object.get(object.get(k, "pkixPublicKey", {}), "publicKeyPem", "")

deny contains msg if {
	some a in attestors
	some i, k in attestor_keys(a)
	regex.match(`PRIVATE KEY-----`, pem(k))
	msg := sprintf("%s: public key %d holds a private key; a private key is never in Git (S7, S20)", [lk.who(a), i])
}

deny contains msg if {
	some a in attestors
	some i, k in attestor_keys(a)
	object.get(k, "asciiArmoredPgpPublicKey", "") == ""
	text := trim_space(pem(k))
	not startswith(text, "-----BEGIN PUBLIC KEY-----")
	msg := sprintf("%s: public key %d is not a PEM public key (-----BEGIN PUBLIC KEY-----); a certificate or anything else cannot be trusted as the signing key (S20)", [lk.who(a), i])
}

deny contains msg if {
	some a in attestors
	some i, k in attestor_keys(a)
	object.get(k, "asciiArmoredPgpPublicKey", "") == ""
	text := trim_space(pem(k))
	startswith(text, "-----BEGIN PUBLIC KEY-----")
	not endswith(text, "-----END PUBLIC KEY-----")
	msg := sprintf("%s: public key %d is not a PEM public key: it does not end with -----END PUBLIC KEY----- (S20)", [lk.who(a), i])
}

kms_key_version := `^//cloudkms\.googleapis\.com/v1/projects/[^/]+/locations/[^/]+/keyRings/[^/]+/cryptoKeys/[^/]+/cryptoKeyVersions/[0-9]+$`

deny contains msg if {
	some a in attestors
	some i, k in attestor_keys(a)
	object.get(k, "asciiArmoredPgpPublicKey", "") == ""
	id := object.get(k, "id", "")
	not regex.match(kms_key_version, id)
	msg := sprintf("%s: public key %d has the id %q, which is not a Cloud KMS key version (//cloudkms.googleapis.com/v1/projects/P/locations/L/keyRings/R/cryptoKeys/K/cryptoKeyVersions/N); the attestor trusts a key that cannot be exported, never one kept somewhere (S19, S20)", [lk.who(a), i, id])
}

signature_algorithms := {
	"ECDSA_P256_SHA256",
	"EC_SIGN_P256_SHA256",
	"ECDSA_P384_SHA384",
	"EC_SIGN_P384_SHA384",
	"RSA_PSS_3072_SHA256",
	"RSA_PSS_4096_SHA256",
	"RSA_PSS_4096_SHA512",
	"RSA_SIGN_PKCS1_3072_SHA256",
	"RSA_SIGN_PKCS1_4096_SHA256",
	"RSA_SIGN_PKCS1_4096_SHA512",
}

deny contains msg if {
	some a in attestors
	some i, k in attestor_keys(a)
	object.get(k, "asciiArmoredPgpPublicKey", "") == ""
	algorithm := object.get(object.get(k, "pkixPublicKey", {}), "signatureAlgorithm", "")
	not algorithm in signature_algorithms
	msg := sprintf("%s: public key %d has the signatureAlgorithm %q; it must be one of %v (no RSA below 3072 bits)", [lk.who(a), i, algorithm, sort(signature_algorithms)])
}

# --- the attestor's note --------------------------------------------------------------------------------------------------------

deny contains msg if {
	some a in attestors
	name := object.get(a.spec, ["userOwnedDrydockNote", "noteRef", "name"], "")
	not name in note_names
	msg := sprintf("%s: its note %q is not in this output; the attestor reads attestations from a note this chart makes (S20)", [lk.who(a), name])
}

deny contains msg if {
	some n in notes
	object.get(n.spec, ["attestation", "hint", "humanReadableName"], "") == ""
	msg := sprintf("%s: spec.attestation.hint.humanReadableName is empty; a Container Analysis note is an attestation authority only with it", [lk.who(n)])
}

# --- the fallback: dry run -----------------------------------------------------------------------------------------------------

# Until Q24 is answered the policy audits and does not block (plan 09 A5): an image without an attestation is logged as one that
# would have been blocked. That is allowed to render in every environment, and it is said on every run.
warn contains msg if {
	some p in policies
	object.get(default_rule(p), "enforcementMode", "") == "DRYRUN_AUDIT_LOG_ONLY"
	msg := sprintf("%s is in dry-run (DRYRUN_AUDIT_LOG_ONLY): an image without an attestation is written to the audit log and admitted, until Q24 is answered and the mode is set to ENFORCED_BLOCK_AND_AUDIT_LOG (plan 09 A5)", [lk.who(p)])
}
