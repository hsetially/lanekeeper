# Tests for binauthz.rego (09/T6; S20, plan 09 A5): the project's admission policy requires an attestation from the CI signing key, whatever else
# is right, and the attestor trusts a Cloud KMS key and nothing else. Run with `conftest verify --policy deploy/policy`.
package binauthz

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

test_good_passes if {
	count(deny) == 0 with input as fixtures.ba
	count(warn) == 0 with input as fixtures.ba
}

test_no_admission_resources_is_fine_for_other_charts if {
	count(deny) == 0 with input as fixtures.hub
	count(deny) == 0 with input as fixtures.db
	count(deny) == 0 with input as fixtures.gs
	count(warn) == 0 with input as fixtures.hub
}

# --- the default rule: every image needs an attestation

test_deny_allow_all_default if {
	refused(fixtures.ba_policy_set("/spec/defaultAdmissionRule/evaluationMode", "ALWAYS_ALLOW"), "REQUIRE_ATTESTATION")
}

test_deny_always_deny_default if {
	refused(fixtures.ba_policy_set("/spec/defaultAdmissionRule/evaluationMode", "ALWAYS_DENY"), "REQUIRE_ATTESTATION")
}

test_deny_no_evaluation_mode if {
	refused(fixtures.ba_policy_remove("/spec/defaultAdmissionRule/evaluationMode"), "REQUIRE_ATTESTATION")
}

test_deny_no_default_rule if {
	refused(fixtures.ba_policy_remove("/spec/defaultAdmissionRule"), "REQUIRE_ATTESTATION")
}

test_deny_no_required_attestor if {
	refused(fixtures.ba_policy_set("/spec/defaultAdmissionRule/requireAttestationsBy", []), "requireAttestationsBy")
}

test_deny_required_attestor_not_in_the_output if {
	refused(fixtures.ba_policy_set("/spec/defaultAdmissionRule/requireAttestationsBy", [{"name": "somebody-else"}]), "not an attestor in this output")
}

test_deny_attestor_in_another_project if {
	refused(fixtures.ba_policy_set("/spec/defaultAdmissionRule/requireAttestationsBy", [{"external": "projects/other-project/attestors/ci"}]), "not an attestor in this output")
}

test_deny_unspecified_enforcement_mode if {
	refused(fixtures.ba_policy_set("/spec/defaultAdmissionRule/enforcementMode", "ENFORCEMENT_MODE_UNSPECIFIED"), "enforcementMode")
}

test_deny_no_enforcement_mode if {
	refused(fixtures.ba_policy_remove("/spec/defaultAdmissionRule/enforcementMode"), "enforcementMode")
}

test_deny_an_enforcement_mode_that_is_not_one if {
	refused(fixtures.ba_policy_set("/spec/defaultAdmissionRule/enforcementMode", "WARN"), "enforcementMode")
}

# --- dry run is the fallback until Q24 is answered, and it says so

test_dryrun_passes_with_a_warning if {
	count(deny) == 0 with input as fixtures.ba_dryrun
	warned(fixtures.ba_dryrun, "dry-run")
}

test_enforcing_has_no_warning if {
	count(warn) == 0 with input as fixtures.ba
}

# --- no other rule may say something different from the default

test_deny_cluster_rule if {
	planted := fixtures.ba_policy_add("/spec/clusterAdmissionRules", {"us-west1-a.some-cluster": {"evaluationMode": "ALWAYS_ALLOW", "enforcementMode": "ENFORCED_BLOCK_AND_AUDIT_LOG"}})
	refused(planted, "clusterAdmissionRules")
}

test_deny_namespace_rule if {
	planted := fixtures.ba_policy_add("/spec/kubernetesNamespaceAdmissionRules", {"lanekeeper": {"evaluationMode": "ALWAYS_ALLOW", "enforcementMode": "ENFORCED_BLOCK_AND_AUDIT_LOG"}})
	refused(planted, "kubernetesNamespaceAdmissionRules")
}

test_deny_service_account_rule if {
	planted := fixtures.ba_policy_add("/spec/kubernetesServiceAccountAdmissionRules", {"lanekeeper:lanekeeper-hub": {"evaluationMode": "ALWAYS_ALLOW", "enforcementMode": "ENFORCED_BLOCK_AND_AUDIT_LOG"}})
	refused(planted, "kubernetesServiceAccountAdmissionRules")
}

test_deny_istio_rule if {
	planted := fixtures.ba_policy_add("/spec/istioServiceIdentityAdmissionRules", {"spiffe://x/ns/y/sa/z": {"evaluationMode": "ALWAYS_ALLOW", "enforcementMode": "ENFORCED_BLOCK_AND_AUDIT_LOG"}})
	refused(planted, "istioServiceIdentityAdmissionRules")
}

test_an_empty_rule_map_is_not_a_rule if {
	planted := fixtures.ba_policy_add("/spec/clusterAdmissionRules", {})
	not refused(planted, "clusterAdmissionRules")
}

# --- Google's own images

test_deny_global_policy_disabled if {
	refused(fixtures.ba_policy_set("/spec/globalPolicyEvaluationMode", "DISABLE"), "globalPolicyEvaluationMode")
}

test_deny_global_policy_missing if {
	refused(fixtures.ba_policy_remove("/spec/globalPolicyEvaluationMode"), "globalPolicyEvaluationMode")
}

# --- images that skip the check: never ours

test_whitelist_of_a_third_party_path_is_allowed if {
	count(deny) == 0 with input as fixtures.ba_whitelist(["ghcr.io/cloudnative-pg/*", "registry.k8s.io/*"])
}

test_deny_whitelist_of_everything if {
	refused(fixtures.ba_whitelist(["*"]), "admissionWhitelistPatterns")
}

test_deny_whitelist_with_no_registry if {
	refused(fixtures.ba_whitelist(["hub"]), "admissionWhitelistPatterns")
}

test_deny_whitelist_empty_pattern if {
	refused(fixtures.ba_whitelist([""]), "admissionWhitelistPatterns")
}

test_deny_whitelist_of_the_whole_registry if {
	refused(fixtures.ba_whitelist(["ghcr.io/*"]), "covers")
}

test_deny_whitelist_of_our_organisation if {
	refused(fixtures.ba_whitelist(["ghcr.io/hsetially/*"]), "covers")
}

test_deny_whitelist_of_our_images if {
	refused(fixtures.ba_whitelist(["ghcr.io/hsetially/lanekeeper/*"]), "covers")
}

test_deny_whitelist_of_our_hub_image if {
	refused(fixtures.ba_whitelist(["ghcr.io/hsetially/lanekeeper/hub"]), "covers")
}

test_deny_whitelist_of_our_agent_image_by_prefix if {
	refused(fixtures.ba_whitelist(["ghcr.io/hsetially/lanekeeper/ag*"]), "covers")
}

test_whitelist_of_another_organisation_with_a_similar_name_is_allowed if {
	count(deny) == 0 with input as fixtures.ba_whitelist(["ghcr.io/hsetially2/*"])
}

# --- one policy per project, and an attestor needs it

test_deny_two_policies if {
	second := json.patch(fixtures.ba_policy, [{"op": "replace", "path": "/metadata/name", "value": "second"}])
	refused(fixtures.ba_with_doc(second), "exactly one")
}

test_deny_attestor_without_a_policy if {
	refused(fixtures.ba_without("BinaryAuthorizationPolicy"), "without a BinaryAuthorizationPolicy")
}

test_deny_a_policy_without_an_attestor if {
	refused(fixtures.ba_without("BinaryAuthorizationAttestor"), "not an attestor in this output")
}

# --- removing the resource from Git must not remove the policy from the project

test_deny_policy_deleted_with_its_resource if {
	refused(fixtures.ba_patch("BinaryAuthorizationPolicy", "lanekeeper", [{"op": "replace", "path": "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy", "value": "delete"}]), "deletion-policy: abandon")
}

test_deny_policy_without_deletion_policy if {
	refused(fixtures.ba_patch("BinaryAuthorizationPolicy", "lanekeeper", [{"op": "remove", "path": "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy"}]), "deletion-policy: abandon")
}

test_deny_policy_that_a_sync_may_prune if {
	refused(fixtures.ba_patch("BinaryAuthorizationPolicy", "lanekeeper", [{"op": "replace", "path": "/metadata/annotations/argocd.argoproj.io~1sync-options", "value": "Prune=true"}]), "Prune=false")
}

test_deny_attestor_deleted_with_its_resource if {
	refused(fixtures.ba_patch("BinaryAuthorizationAttestor", "lanekeeper-ci", [{"op": "remove", "path": "/metadata/annotations/cnrm.cloud.google.com~1deletion-policy"}]), "deletion-policy: abandon")
}

test_deny_note_that_a_sync_may_prune if {
	refused(fixtures.ba_patch("ContainerAnalysisNote", "lanekeeper-ci", [{"op": "remove", "path": "/metadata/annotations/argocd.argoproj.io~1sync-options"}]), "Prune=false")
}

# --- the attestor trusts one Cloud KMS key and nothing else

test_deny_attestor_without_keys if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys", []), "no public key")
}

test_deny_attestor_without_a_key_list if {
	refused(fixtures.ba_attestor_remove("/spec/userOwnedDrydockNote/publicKeys"), "no public key")
}

test_deny_pgp_key if {
	planted := fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys", [{"asciiArmoredPgpPublicKey": "-----BEGIN PGP PUBLIC KEY BLOCK-----\nAAAA\n-----END PGP PUBLIC KEY BLOCK-----\n"}])
	refused(planted, "PGP")
}

test_deny_private_key_as_the_public_key if {
	planted := fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/publicKeyPem", "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n")
	refused(planted, "private key")
}

test_deny_ec_private_key_as_the_public_key if {
	planted := fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/publicKeyPem", "-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----\n")
	refused(planted, "private key")
}

test_deny_a_private_key_after_the_public_one if {
	planted := fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/publicKeyPem", concat("", [fixtures.ba_pem, "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"]))
	refused(planted, "private key")
}

test_deny_a_pem_that_is_not_a_public_key if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/publicKeyPem", "hello"), "PEM public key")
}

test_deny_a_pem_that_is_cut_short if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/publicKeyPem", "-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE\n"), "does not end with")
}

test_deny_a_certificate_as_the_public_key if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/publicKeyPem", "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"), "PEM public key")
}

test_deny_a_key_that_is_not_in_cloud_kms if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/id", "ni:///sha-256;abcdef"), "Cloud KMS")
}

test_deny_a_key_without_an_id if {
	refused(fixtures.ba_attestor_remove("/spec/userOwnedDrydockNote/publicKeys/0/id"), "Cloud KMS")
}

test_deny_a_kms_key_without_a_version if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/id", "//cloudkms.googleapis.com/v1/projects/p/locations/l/keyRings/r/cryptoKeys/k"), "Cloud KMS")
}

test_deny_a_weak_signature_algorithm if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/signatureAlgorithm", "RSA_SIGN_PKCS1_2048_SHA256"), "signatureAlgorithm")
}

test_deny_an_unspecified_signature_algorithm if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/signatureAlgorithm", "SIGNATURE_ALGORITHM_UNSPECIFIED"), "signatureAlgorithm")
}

test_deny_no_signature_algorithm if {
	refused(fixtures.ba_attestor_remove("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/signatureAlgorithm"), "signatureAlgorithm")
}

test_p384_and_rsa_4096_are_allowed if {
	count(deny) == 0 with input as fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/signatureAlgorithm", "ECDSA_P384_SHA384")
	count(deny) == 0 with input as fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/publicKeys/0/pkixPublicKey/signatureAlgorithm", "RSA_PSS_4096_SHA256")
}

# --- the note and the project

test_deny_attestor_without_its_note if {
	refused(fixtures.ba_without("ContainerAnalysisNote"), "is not in this output")
}

test_deny_attestor_that_names_another_note if {
	refused(fixtures.ba_attestor_set("/spec/userOwnedDrydockNote/noteRef/name", "other-note"), "is not in this output")
}

test_deny_note_without_a_name_for_the_authority if {
	refused(fixtures.ba_note_remove("/spec/attestation/hint/humanReadableName"), "humanReadableName")
}

test_deny_note_that_is_not_an_attestation_note if {
	refused(fixtures.ba_note_remove("/spec/attestation"), "humanReadableName")
}

test_deny_policy_for_another_project if {
	refused(fixtures.ba_policy_set("/spec/projectRef/external", "projects/other-project"), "projectRef")
}

test_deny_attestor_for_another_project if {
	refused(fixtures.ba_attestor_set("/spec/projectRef/external", "projects/other-project"), "projectRef")
}

test_deny_policy_without_a_project if {
	refused(fixtures.ba_policy_remove("/spec/projectRef"), "projectRef")
}
