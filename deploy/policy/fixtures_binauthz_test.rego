# The documents the Binary Authorization policy tests start from (09/T6). `ba` is what `helm template` makes of the gcp chart's
# admission resources (the Container Analysis note, the attestor that holds the public key of the CI signing key, and the project's
# admission policy), written out as conftest's `--combine` input. They take the same helpers as the other fixtures: `ba_patch`
# changes one document with a JSON patch, `ba_without` drops the documents of a kind, `ba_with_doc` adds one.
#
# `deploy/tests/render.sh` runs the real chart through the same policy, so a document here that stops resembling the chart's output
# does not hide a gap: the real output is checked too. Keep them alike; when the chart changes shape, change this. The key is
# SYNTHETIC (deploy/tests/testdata/synthetic-attestor-key.yaml): a throwaway public key whose private half never existed on disk.
package fixtures

import rego.v1

ba_attestor_name := "lanekeeper-ci"

ba_key_version := "projects/example-project/locations/example-location/keyRings/example/cryptoKeys/ci-cosign/cryptoKeyVersions/1"

ba_pem := "-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAERM+mzO2UnKpik7yzIC9TGU3dK1LA\nI4sALn4Knjtcr4qLEKSuVj3EAVW7ny5nBPUrQAljkAjQOtdQW1p1K14cAw==\n-----END PUBLIC KEY-----\n"

ba_labels := {
	"app.kubernetes.io/name": "lanekeeper-gcp",
	"app.kubernetes.io/instance": "lanekeeper-gcp",
	"app.kubernetes.io/part-of": "lanekeeper",
	"lanekeeper.io/purpose": "admission",
	"lanekeeper.io/environment": "prod",
}

# Removing one of these from Git must not remove what it made in the project: without the policy, every image is admitted.
ba_annotations := {
	"cnrm.cloud.google.com/project-id": gs_project,
	"cnrm.cloud.google.com/deletion-policy": "abandon",
	"argocd.argoproj.io/sync-options": "Prune=false",
}

ba_note := {
	"apiVersion": "containeranalysis.cnrm.cloud.google.com/v1beta1",
	"kind": "ContainerAnalysisNote",
	"metadata": {"name": ba_attestor_name, "namespace": gs_namespace, "labels": ba_labels, "annotations": ba_annotations},
	"spec": {
		"shortDescription": "Lanekeeper CI image signature",
		"attestation": {"hint": {"humanReadableName": "Lanekeeper CI"}},
	},
}

ba_attestor := {
	"apiVersion": "binaryauthorization.cnrm.cloud.google.com/v1beta1",
	"kind": "BinaryAuthorizationAttestor",
	"metadata": {"name": ba_attestor_name, "namespace": gs_namespace, "labels": ba_labels, "annotations": ba_annotations},
	"spec": {
		"projectRef": {"external": "projects/example-project"},
		"description": "Images signed by the Lanekeeper CI Cloud KMS key",
		"userOwnedDrydockNote": {
			"noteRef": {"name": ba_attestor_name},
			"publicKeys": [{
				"comment": concat("", ["Cloud KMS key version ", ba_key_version]),
				"id": concat("", ["//cloudkms.googleapis.com/v1/", ba_key_version]),
				"pkixPublicKey": {"publicKeyPem": ba_pem, "signatureAlgorithm": "ECDSA_P256_SHA256"},
			}],
		},
	},
}

ba_policy := {
	"apiVersion": "binaryauthorization.cnrm.cloud.google.com/v1beta1",
	"kind": "BinaryAuthorizationPolicy",
	"metadata": {"name": "lanekeeper", "namespace": gs_namespace, "labels": ba_labels, "annotations": ba_annotations},
	"spec": {
		"projectRef": {"external": "projects/example-project"},
		"description": "Only images signed by the Lanekeeper CI key",
		"globalPolicyEvaluationMode": "ENABLE",
		"defaultAdmissionRule": {
			"evaluationMode": "REQUIRE_ATTESTATION",
			"enforcementMode": "ENFORCED_BLOCK_AND_AUDIT_LOG",
			"requireAttestationsBy": [{"name": ba_attestor_name}],
		},
	},
}

ba_docs := [ba_note, ba_attestor, ba_policy]

# The input of a conftest run over the gcp chart's admission resources, enforcing (the end state of Q24).
ba := [{"path": "rendered/gcp-admission.yaml", "contents": ba_docs}]

# The same in dry-run, which is what the committed values say until Q24 is answered (plan 09 A5).
ba_dryrun := ba_patch("BinaryAuthorizationPolicy", "lanekeeper", [{"op": "replace", "path": "/spec/defaultAdmissionRule/enforcementMode", "value": "DRYRUN_AUDIT_LOG_ONLY"}])

ba_patch(kind, doc_name, ops) := [{"path": "rendered/gcp-admission.yaml", "contents": changed(ba_docs, kind, doc_name, ops)}]

ba_without(kind) := [{"path": "rendered/gcp-admission.yaml", "contents": [d | some d in ba_docs; d.kind != kind]}]

ba_with_doc(doc) := [{"path": "rendered/gcp-admission.yaml", "contents": array.concat(ba_docs, [doc])}]

ba_policy_set(path, value) := ba_patch("BinaryAuthorizationPolicy", "lanekeeper", [{"op": "replace", "path": path, "value": value}])

ba_policy_add(path, value) := ba_patch("BinaryAuthorizationPolicy", "lanekeeper", [{"op": "add", "path": path, "value": value}])

ba_policy_remove(path) := ba_patch("BinaryAuthorizationPolicy", "lanekeeper", [{"op": "remove", "path": path}])

ba_attestor_set(path, value) := ba_patch("BinaryAuthorizationAttestor", ba_attestor_name, [{"op": "replace", "path": path, "value": value}])

ba_attestor_remove(path) := ba_patch("BinaryAuthorizationAttestor", ba_attestor_name, [{"op": "remove", "path": path}])

ba_note_remove(path) := ba_patch("ContainerAnalysisNote", ba_attestor_name, [{"op": "remove", "path": path}])

# The policy with a list of name patterns that skip the check.
ba_whitelist(patterns) := ba_policy_add("/spec/admissionWhitelistPatterns", [{"namePattern": p} | some p in patterns])
