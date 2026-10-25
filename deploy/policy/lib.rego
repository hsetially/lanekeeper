# Helpers shared by the manifest policies (09/T3). Package lk has no deny, warn or violation rule, so conftest never runs it as
# a policy; the other packages import it.
#
# The manifest policies run with `conftest test --combine`, so `input` is an array with one entry per file, {path, contents},
# and `contents` is the one document of a file or the array of its documents (a multi-document `helm template` output). The
# same shape carries perf/budgets.toml (a document without `kind`) and the values files (also without `kind`), so a policy
# that needs a budget or a values file finds it by path, and every Kubernetes policy sees only documents that have a kind.
package lk

import rego.v1

documents(file) := file.contents if is_array(file.contents)

documents(file) := [file.contents] if not is_array(file.contents)

docs := [d | some f in input; some d in documents(f); is_object(d); is_string(d.kind)]

of_kind(kind) := [d | some d in docs; d.kind == kind]

named(kind, name) := [d | some d in of_kind(kind); d.metadata.name == name]

# The files of the input whose path ends with a suffix (budgets.toml, a values file).
files_ending(suffix) := [f | some f in input; endswith(f.path, suffix)]

# The pod template of every kind of workload.
pod_spec(d) := d.spec.template.spec if d.kind in {"StatefulSet", "Deployment", "Job", "DaemonSet", "ReplicaSet"}

pod_spec(d) := d.spec.jobTemplate.spec.template.spec if d.kind == "CronJob"

pod_spec(d) := d.spec if d.kind == "Pod"

pod_labels(d) := d.spec.template.metadata.labels if d.kind in {"StatefulSet", "Deployment", "Job", "DaemonSet", "ReplicaSet"}

pod_labels(d) := d.metadata.labels if d.kind == "Pod"

pods := [{"doc": d, "spec": spec} | some d in docs; spec := pod_spec(d)]

who(d) := sprintf("%s/%s", [d.kind, d.metadata.name])

all_containers(spec) := array.concat(object.get(spec, "containers", []), object.get(spec, "initContainers", []))

# Every container of every workload, with the workload it belongs to.
containers := [{"doc": p.doc, "spec": p.spec, "container": c} | some p in pods; some c in all_containers(p.spec)]

hub_name := "lanekeeper-hub"

hub_statefulsets := named("StatefulSet", hub_name)

# A Kubernetes quantity of memory in mebibytes. Only the binary units and plain bytes; anything else is undefined, and the
# policy that asks says so.
mebibytes(q) := n if {
	is_string(q)
	endswith(q, "Gi")
	n := to_number(trim_suffix(q, "Gi")) * 1024
}

mebibytes(q) := n if {
	is_string(q)
	endswith(q, "Mi")
	n := to_number(trim_suffix(q, "Mi"))
}

mebibytes(q) := n if {
	is_string(q)
	endswith(q, "Ki")
	n := to_number(trim_suffix(q, "Ki")) / 1024
}

mebibytes(q) := n if {
	is_number(q)
	n := q / 1048576
}

# The prefix length of a CIDR (undefined for a bare address, which is a single host).
prefix_length(cidr) := to_number(split(cidr, "/")[1])

# A source range that is the whole internet, or so wide that it is close to it: IPv4 shorter than /16, IPv6 shorter than /32.
# Cloud Armor writes "everyone" as "*".
too_wide(cidr) if cidr in {"0.0.0.0/0", "::/0", "*"}

too_wide(cidr) if {
	not contains(cidr, ":")
	prefix_length(cidr) < 16
}

too_wide(cidr) if {
	contains(cidr, ":")
	prefix_length(cidr) < 32
}
