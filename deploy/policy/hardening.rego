# Pod hardening policy (09/T3; S6, S7, S16). Run with `conftest test --combine --namespace hardening` over a chart's rendered
# output. The rules are the fields S16 lists, applied to every workload (StatefulSet, Deployment, Job, CronJob, DaemonSet,
# Pod), plus what the hub's contract (crates/hub/HARDENING.md) says about its own pod.
#
# Each rule has a planted case in hardening_test.rego, so a rule that stops firing fails `conftest verify`.
package hardening

import rego.v1

import data.lk

# --- the pod ----------------------------------------------------------------------------------------------------------

# No pod mounts a service-account token unless its work needs the Kubernetes API, and then it says so. The one pod that does is the
# agent (crates/agent/HARDENING.md, S16): it watches workloads, patches the restart annotation and updates its certificate Secret.
needs_token(doc) if {
	doc.kind == "Deployment"
	doc.metadata.name == lk.agent_name
}

deny contains msg if {
	some p in lk.pods
	not needs_token(p.doc)
	object.get(p.spec, "automountServiceAccountToken", true) != false
	msg := sprintf("%s: automountServiceAccountToken must be stated and false (S16)", [lk.who(p.doc)])
}

deny contains msg if {
	some p in lk.pods
	needs_token(p.doc)
	object.get(p.spec, "automountServiceAccountToken", null) != true
	msg := sprintf("%s: automountServiceAccountToken must be stated and true: the agent calls the Kubernetes API with it (crates/agent/HARDENING.md)", [lk.who(p.doc)])
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

# --- the agent's own contract (crates/agent/HARDENING.md; 09/T4) ----------------------------------------------------------------
#
# One replica, its own service account (the Roles are bound to it), one port (ops 9090), the probes of the contract, volumes from
# the four it has (the NFS export, /tmp, the spool, the hub's CA), writable only where the code writes, and the paths it is given
# are the paths of its mounts. The spool (09/T8; D74) is a PersistentVolumeClaim of its own, writable, outside the export.

agent_volumes := {"nfs", "tmp", "spool", "hub-ca"}

agent_writable_volumes := {"nfs", "tmp", "spool"}

agent_containers := [{"doc": d, "container": c} | some d in lk.agent_deployments; some c in d.spec.template.spec.containers]

deny contains msg if {
	some d in lk.agent_deployments
	object.get(d.spec, "replicas", 0) != 1
	msg := sprintf("%s: the agent runs 1 replica, not %v: two agents would write the same export and share one identity (crates/agent/HARDENING.md)", [lk.who(d), object.get(d.spec, "replicas", "unset")])
}

deny contains msg if {
	some d in lk.agent_deployments
	object.get(d.spec.template.spec, "serviceAccountName", "default") != lk.agent_name
	msg := sprintf("%s: serviceAccountName must be %s, the account its Roles are bound to (crates/agent/HARDENING.md)", [lk.who(d), lk.agent_name])
}

deny contains msg if {
	some x in agent_containers
	declared := {[p.name, p.containerPort] | some p in object.get(x.container, "ports", [])}
	declared != {["ops", 9090]}
	msg := sprintf("%s: the container ports must be exactly ops 9090 (crates/agent/HARDENING.md)", [lk.who(x.doc)])
}

deny contains msg if {
	some x in agent_containers
	not probes_ok(x.container)
	msg := sprintf("%s: the probes must be /healthz (liveness) and /readyz (readiness) on the ops port (crates/agent/HARDENING.md)", [lk.who(x.doc)])
}

agent_volume_list(d) := object.get(d.spec.template.spec, "volumes", [])

deny contains msg if {
	some d in lk.agent_deployments
	some v in agent_volume_list(d)
	not v.name in agent_volumes
	msg := sprintf("%s: volume %s is not one of the agent's %v (crates/agent/HARDENING.md)", [lk.who(d), v.name, sort(agent_volumes)])
}

deny contains msg if {
	some d in lk.agent_deployments
	some v in agent_volume_list(d)
	v.name == "nfs"
	not "nfs" in object.keys(v)
	msg := sprintf("%s: volume nfs must be an NFS volume, the export the agent keeps in sync (crates/agent/HARDENING.md)", [lk.who(d)])
}

deny contains msg if {
	some d in lk.agent_deployments
	some v in agent_volume_list(d)
	v.name == "hub-ca"
	not "configMap" in object.keys(v)
	msg := sprintf("%s: volume hub-ca must be the ConfigMap of the hub's CA, not a Secret or anything else (crates/agent/HARDENING.md)", [lk.who(d)])
}

deny contains msg if {
	some x in agent_containers
	some m in object.get(x.container, "volumeMounts", [])
	object.get(m, "readOnly", false) != true
	not m.name in agent_writable_volumes
	msg := sprintf("%s: %s is mounted writable; only %v may be (crates/agent/HARDENING.md)", [lk.who(x.doc), m.name, sort(agent_writable_volumes)])
}

mount_path(c, volume) := m.mountPath if {
	some m in object.get(c, "volumeMounts", [])
	m.name == volume
}

# LK_NFS_ROOT, LK_TMP_DIR and LK_SPOOL_DIR are the mount paths of the volumes they name, and LK_HUB_CA_FILE is a file inside the hub-ca
# mount. The agent writes where its variables say, so a variable that names another path is a write to the read-only root.
deny contains msg if {
	some x in agent_containers
	some pair in [["LK_NFS_ROOT", "nfs"], ["LK_TMP_DIR", "tmp"], ["LK_SPOOL_DIR", "spool"]]
	not path_is_mount(x.container, pair[0], pair[1])
	msg := sprintf("%s: %s must be the path of the %s mount (crates/agent/HARDENING.md)", [lk.who(x.doc), pair[0], pair[1]])
}

path_is_mount(c, variable, volume) if lk.env_value(c, variable) == mount_path(c, volume)

deny contains msg if {
	some x in agent_containers
	not hub_ca_file_in_mount(x.container)
	msg := sprintf("%s: LK_HUB_CA_FILE must be a file inside the hub-ca mount (crates/agent/HARDENING.md)", [lk.who(x.doc)])
}

hub_ca_file_in_mount(c) if startswith(lk.env_value(c, "LK_HUB_CA_FILE"), concat("", [mount_path(c, "hub-ca"), "/"]))

# --- the spool (09/T8; D74) --------------------------------------------------------------------------------------------------------

spool_claim := "lanekeeper-agent-spool"

# The spool outlives a pod: the changes the hub has not acknowledged are in it. An emptyDir or a host path would lose them, or share them.
deny contains msg if {
	some d in lk.agent_deployments
	some v in agent_volume_list(d)
	v.name == "spool"
	object.get(object.get(v, "persistentVolumeClaim", {}), "claimName", "") != spool_claim
	msg := sprintf("%s: volume spool must be the PersistentVolumeClaim %s (D74): the spool outlives a pod, and any other kind of volume loses it or shares it", [lk.who(d), spool_claim])
}

# The agent stops at start without its spool directory (crates/agent/HARDENING.md).
deny contains msg if {
	some d in lk.agent_deployments
	count([v | some v in agent_volume_list(d); v.name == "spool"]) == 0
	msg := sprintf("%s: it has no volume spool, and the agent stops at start without its spool directory (D74)", [lk.who(d)])
}

deny contains msg if {
	some x in agent_containers
	some m in object.get(x.container, "volumeMounts", [])
	m.name == "spool"
	object.get(m, "readOnly", false) == true
	msg := sprintf("%s: the spool is mounted read-only, and the agent writes its spool there (D74)", [lk.who(x.doc)])
}

# The spool may not be inside the export: its files would be tracked as configuration and sent to the hub again, for ever. The agent
# refuses such a path at start; this refuses it before. Inside is by path component: /mnt/csp-configuration-spool is not inside
# /mnt/csp-configuration.
inside(path, root) if path == root

inside(path, root) if startswith(path, concat("", [trim_suffix(root, "/"), "/"]))

deny contains msg if {
	some x in agent_containers
	spool := lk.env_value(x.container, "LK_SPOOL_DIR")
	root := lk.env_value(x.container, "LK_NFS_ROOT")
	inside(spool, root)
	msg := sprintf("%s: LK_SPOOL_DIR %s is inside the NFS root %s: the spool would be tracked as configuration and sent again, for ever (crates/agent/HARDENING.md)", [lk.who(x.doc), spool, root])
}
