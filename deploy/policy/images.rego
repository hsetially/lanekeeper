# Image policy (09/T3; S19): every container image is pinned by digest, so a tag that moves cannot change what runs. Run with
# `conftest test --combine --namespace images`. The Dockerfiles have their own policy (dockerfile.rego).
package images

import rego.v1

import data.lk

digest_pinned(ref) if regex.match(`^[a-z0-9.\-/:_]+@sha256:[0-9a-f]{64}$`, ref)

deny contains msg if {
	some x in lk.containers
	not digest_pinned(object.get(x.container, "image", ""))
	msg := sprintf("%s, container %s: image %q is not pinned by digest (S19)", [lk.who(x.doc), x.container.name, object.get(x.container, "image", "")])
}

warn contains msg if {
	some x in lk.containers
	endswith(object.get(x.container, "image", ""), "@sha256:0000000000000000000000000000000000000000000000000000000000000000")
	msg := sprintf("%s, container %s: the image digest is the placeholder; put the published digest in the values (plan 09 A29)", [lk.who(x.doc), x.container.name])
}
