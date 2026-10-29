# Tests for images.rego: every image of every container is pinned by digest (S19).
package images

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

image_op(image) := {"op": "replace", "path": "/spec/template/spec/containers/0/image", "value": image}

test_good_passes if {
	count(deny) == 0 with input as fixtures.hub
	count(warn) == 0 with input as fixtures.hub
}

test_deny_tag_only_image if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [image_op("ghcr.io/hsetially/lanekeeper/hub:1.2.3")])
	refused(planted, "not pinned by digest")
}

test_deny_latest_tag if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [image_op("ghcr.io/hsetially/lanekeeper/hub:latest")])
	refused(planted, "not pinned by digest")
}

test_deny_no_tag_no_digest if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [image_op("ghcr.io/hsetially/lanekeeper/hub")])
	refused(planted, "not pinned by digest")
}

test_deny_short_digest if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [image_op("ghcr.io/hsetially/lanekeeper/hub@sha256:abc123")])
	refused(planted, "not pinned by digest")
}

test_deny_other_hash_algorithm if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [image_op("ghcr.io/hsetially/lanekeeper/hub@sha512:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")])
	refused(planted, "not pinned by digest")
}

test_deny_unpinned_init_container if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/initContainers", "value": [{"name": "init", "image": "busybox"}]}])
	refused(planted, "init")
}

test_deny_unpinned_cronjob_image if {
	planted := fixtures.with_doc({
		"apiVersion": "batch/v1", "kind": "CronJob", "metadata": {"name": "nightly"},
		"spec": {"jobTemplate": {"spec": {"template": {"spec": {"containers": [{"name": "c", "image": "alpine:3"}]}}}}},
	})
	refused(planted, "CronJob/nightly")
}

# A digest of 64 zeros is the placeholder that values files carry until the first image is published (plan 09 A29). It cannot
# be pulled, so it fails safe, but a person should see it: a warning, not a failure, so that the values files can be committed.
test_warn_placeholder_digest if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [image_op("ghcr.io/hsetially/lanekeeper/hub@sha256:0000000000000000000000000000000000000000000000000000000000000000")])
	warned(planted, "placeholder")
	count(deny) == 0 with input as planted
}
