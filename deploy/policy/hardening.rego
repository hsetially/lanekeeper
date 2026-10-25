# Pod hardening policy (09/T3; S6, S7, S16). Run with `conftest test --combine --namespace hardening` over a chart's rendered
# output. The rules are the fields S16 lists, applied to every workload (StatefulSet, Deployment, Job, CronJob, DaemonSet,
# Pod), plus what the hub's contract (crates/hub/HARDENING.md) says about its own pod.
#
# Each rule has a planted case in hardening_test.rego, so a rule that stops firing fails `conftest verify`.
package hardening

import rego.v1

import data.lk

# --- the pod ----------------------------------------------------------------------------------------------------------

deny contains msg if {
	some p in lk.pods
	object.get(p.spec, "automountServiceAccountToken", true) != false
	msg := sprintf("%s: automountServiceAccountToken must be stated and false (S16)", [lk.who(p.doc)])
}

deny contains msg if {
	some p in lk.pods
	some field in ["hostNetwork", "hostPID", "hostIPC"]
	object.get(p.spec, field, false) == true
	msg := sprintf("%s: %s is set (S16)", [lk.who(p.doc), field])
}

deny contains msg if {
	some p in lk.pods
	some v in object.get(p.spec, "volumes", [])
	"hostPath" in object.keys(v)
	msg := sprintf("%s: volume %s is a hostPath (S16)", [lk.who(p.doc), v.name])
}

# The value in force for a container: its own securityContext, else the pod's, else null.
effective(spec, c, field) := object.get(c, ["securityContext", field], object.get(spec, ["securityContext", field], null))

effective_seccomp(spec, c) := object.get(c, ["securityContext", "seccompProfile", "type"], object.get(spec, ["securityContext", "seccompProfile", "type"], null))

# --- every container, init containers included ------------------------------------------------------------------------

deny contains msg if {
	some x in lk.containers
	effective(x.spec, x.container, "runAsNonRoot") != true
	msg := sprintf("%s, container %s: runAsNonRoot is not true (S16)", [lk.who(x.doc), x.container.name])
}

deny contains msg if {
	some x in lk.containers
	some field in ["runAsUser", "runAsGroup"]
	id := effective(x.spec, x.container, field)
	not is_number(id)
	msg := sprintf("%s, container %s: %s must be a numeric id (S16)", [lk.who(x.doc), x.container.name, field])
}

deny contains msg if {
	some x in lk.containers
	some field in ["runAsUser", "runAsGroup"]
	effective(x.spec, x.container, field) == 0
	msg := sprintf("%s, container %s: %s is root (S16)", [lk.who(x.doc), x.container.name, field])
}

deny contains msg if {
	some x in lk.containers
	effective_seccomp(x.spec, x.container) != "RuntimeDefault"
	msg := sprintf("%s, container %s: the seccomp profile is not RuntimeDefault (S16)", [lk.who(x.doc), x.container.name])
}

deny contains msg if {
	some x in lk.containers
	object.get(x.container, ["securityContext", "allowPrivilegeEscalation"], null) != false
	msg := sprintf("%s, container %s: allowPrivilegeEscalation is not false (S16)", [lk.who(x.doc), x.container.name])
}

deny contains msg if {
	some x in lk.containers
	object.get(x.container, ["securityContext", "privileged"], false) == true
	msg := sprintf("%s, container %s: privileged (S16)", [lk.who(x.doc), x.container.name])
}

deny contains msg if {
	some x in lk.containers
	object.get(x.container, ["securityContext", "readOnlyRootFilesystem"], null) != true
	msg := sprintf("%s, container %s: readOnlyRootFilesystem is not true (S16)", [lk.who(x.doc), x.container.name])
}

deny contains msg if {
	some x in lk.containers
	object.get(x.container, ["securityContext", "capabilities", "drop"], []) != ["ALL"]
	msg := sprintf("%s, container %s: capabilities must drop ALL (S16)", [lk.who(x.doc), x.container.name])
}

deny contains msg if {
	some x in lk.containers
	count(object.get(x.container, ["securityContext", "capabilities", "add"], [])) > 0
	msg := sprintf("%s, container %s: adds capabilities (S16)", [lk.who(x.doc), x.container.name])
}

deny contains msg if {
	some x in lk.containers
	some pair in [["requests", "cpu"], ["requests", "memory"], ["limits", "cpu"], ["limits", "memory"]]
	object.get(x.container, ["resources", pair[0], pair[1]], null) == null
	msg := sprintf("%s, container %s: resources.%s.%s is not set (S16)", [lk.who(x.doc), x.container.name, pair[0], pair[1]])
}

deny contains msg if {
	some x in lk.containers
	some port in object.get(x.container, "ports", [])
	"hostPort" in object.keys(port)
	msg := sprintf("%s, container %s: a hostPort (S16)", [lk.who(x.doc), x.container.name])
}

# --- what reaches the pod from Secrets (S6, S7) ------------------------------------------------------------------------

# CloudNativePG's CA Secret also holds ca.key, which signs server certificates: the pod gets ca.crt and nothing else (plan 09
# A9). Its name ends in -db-ca (`lanekeeper-db-ca`).
ca_only(v) if {
	count(v.secret.items) == 1
	v.secret.items[0].key == "ca.crt"
}

deny contains msg if {
	some p in lk.pods
	some v in object.get(p.spec, "volumes", [])
	endswith(object.get(v, ["secret", "secretName"], ""), "-db-ca")
	not ca_only(v)
	msg := sprintf("%s: volume %s mounts more of the CloudNativePG CA Secret than ca.crt; it also holds ca.key (S6, plan 09 A9)", [lk.who(p.doc), v.name])
}

# The database credentials Secret is mounted for one key, the connection URL, and no world can read it (HARDENING.md).
uri_only(v) if {
	count(v.secret.items) == 1
	v.secret.items[0].key == "uri"
}

deny contains msg if {
	some sts in lk.hub_statefulsets
	some v in sts.spec.template.spec.volumes
	v.name == "db-credentials"
	not uri_only(v)
	msg := sprintf("%s: volume db-credentials must mount only the uri key of the database Secret, not its other keys (S7)", [lk.who(sts)])
}

deny contains msg if {
	some sts in lk.hub_statefulsets
	some v in sts.spec.template.spec.volumes
	v.name == "db-credentials"
	mode := object.get(v, ["secret", "defaultMode"], 420)
	bits.and(mode, 7) != 0
	msg := sprintf("%s: volume db-credentials is world-readable (mode %d); it must be 0440 or tighter (S7)", [lk.who(sts), mode])
}

# --- the hub's own contract (crates/hub/HARDENING.md) ------------------------------------------------------------------

writable_volumes := {"tmp", "mirror"}

deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	some m in c.volumeMounts
	object.get(m, "readOnly", false) != true
	not m.name in writable_volumes
	msg := sprintf("%s: %s is mounted writable; only %v may be (HARDENING.md)", [lk.who(sts), m.name, sort(writable_volumes)])
}

hub_ports := {["web", 8443], ["agent", 8444], ["replica", 8445], ["ops", 9090]}

deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	declared := {[p.name, p.containerPort] | some p in object.get(c, "ports", [])}
	declared != hub_ports
	msg := sprintf("%s: the container ports must be exactly web 8443, agent 8444, replica 8445 and ops 9090 (HARDENING.md)", [lk.who(sts)])
}

probe_path(c, probe) := object.get(c, [probe, "httpGet", "path"], null)

probe_port(c, probe) := object.get(c, [probe, "httpGet", "port"], null)

deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	not probes_ok(c)
	msg := sprintf("%s: the probes must be /healthz (liveness) and /readyz (readiness) on the ops port (HARDENING.md)", [lk.who(sts)])
}

probes_ok(c) if {
	probe_path(c, "livenessProbe") == "/healthz"
	probe_path(c, "readinessProbe") == "/readyz"
	probe_port(c, "livenessProbe") == "ops"
	probe_port(c, "readinessProbe") == "ops"
}
