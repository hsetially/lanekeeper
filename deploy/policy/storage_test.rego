# Tests for storage.rego: persistent volumes are encrypted with a customer-managed key (S10). The buckets are T5's.
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
