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

# The database credentials Secret is mounted for one key, the connection URL, and no world can read it (HARDENING.md). The hub mounts the
# application role's (db-credentials); the migration Job the migrator's (migrator-credentials); the seed Job the application role's again.
credential_volumes := {"db-credentials", "migrator-credentials"}

uri_only(v) if {
	count(v.secret.items) == 1
	v.secret.items[0].key == "uri"
}

credential_volume_of(w) := [v |
	some v in object.get(lk.pod_spec(w), "volumes", [])
	v.name in credential_volumes
]

deny contains msg if {
	some w in lk.hub_workloads
	some v in credential_volume_of(w)
	not uri_only(v)
	msg := sprintf("%s: volume %s must mount only the uri key of the database Secret, not its other keys (S7)", [lk.who(w), v.name])
}

deny contains msg if {
	some w in lk.hub_workloads
	some v in credential_volume_of(w)
	mode := object.get(v, ["secret", "defaultMode"], 420)
	bits.and(mode, 7) != 0
	msg := sprintf("%s: volume %s is world-readable (mode %d); it must be 0440 or tighter (S7)", [lk.who(w), v.name, mode])
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

# --- what the hub's environment may hold (crates/hub/HARDENING.md, README items 5, 7 and 9; 09/fix) ----------------------------------------
#
# The pod contract allows one reference in the environment: the replica's name, from the downward API, as exactly
# `fieldRef: {fieldPath: metadata.name}` on exactly LK_REPLICA_NAME (crates/hub/tests/hardening.rs holds the Pod block to it). The
# StatefulSet and the Jobs of the hub's image are held to the same: a `configMapKeyRef`, a `resourceFieldRef`, a fieldRef on another
# variable or to another field is a secret or a setting that is not the pod's.

hub_workload_containers := [{"doc": d, "container": c} |
	some d in lk.hub_workloads
	some c in lk.all_containers(lk.pod_spec(d))
]

own_name(e) if {
	e.name == "LK_REPLICA_NAME"
	e.valueFrom == {"fieldRef": {"fieldPath": "metadata.name"}}
}

deny contains msg if {
	some x in hub_workload_containers
	some e in object.get(x.container, "env", [])
	"valueFrom" in object.keys(e)
	not own_name(e)
	msg := sprintf("%s, container %s: %s takes its value from a valueFrom reference; the only one the contract allows is LK_REPLICA_NAME from fieldRef metadata.name, exactly (HARDENING.md)", [lk.who(x.doc), x.container.name, e.name])
}

# `envFrom` is the other way to fill the environment: every key of a ConfigMap or Secret becomes a variable, under a prefix of the
# author's choosing. The rules above and below read `env` and nothing else, so a name that comes in through it is not held to the
# contract's names, values or references. The hub's pod and its Jobs have no use for it: their settings are named one by one in the chart.
deny contains msg if {
	some x in hub_workload_containers
	count(object.get(x.container, "envFrom", [])) > 0
	msg := sprintf("%s, container %s: envFrom fills the environment with every key of a ConfigMap or Secret, past the rules about the hub's variables; name each variable in env instead (HARDENING.md)", [lk.who(x.doc), x.container.name])
}

# Every replica has its own name. A fixed one would give three replicas one identity (the certificate's, the leases', agent_connections').
deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	not replica_name_from_pod(c)
	msg := sprintf("%s, container %s: LK_REPLICA_NAME must come from the pod's own name (fieldRef metadata.name), so that each replica has its own (HARDENING.md)", [lk.who(sts), c.name])
}

replica_name_from_pod(c) if {
	some e in object.get(c, "env", [])
	own_name(e)
}

# The Jobs' variables are the Jobs' alone: the migrator's credential must never be in a pod that faces users, and the seed's timeout
# is not the hub's (README item 5, HARDENING.md "The migration Job" and "The seed Job").
job_only_variables := {"LK_MIGRATOR_URL_FILE", "LK_MIGRATE_TIMEOUT_SECS", "LK_SEED_TIMEOUT_SECS"}

deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	some e in object.get(c, "env", [])
	e.name in job_only_variables
	msg := sprintf("%s, container %s: %s is a variable of the Jobs; the serving hub does not carry it (HARDENING.md)", [lk.who(sts), c.name, e.name])
}

# The users' name and Google's: LK_PUBLIC_ORIGIN is the origin a request may name in Origin and the host Entra sends the browser back
# to; LK_HUB_AUDIENCE is what agents ask Google for a token for. The chart derives both from gateway.hostname, so they cannot diverge, and
# the Gateway serves the same name.
deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	lk.env_value(c, "LK_PUBLIC_ORIGIN") != lk.env_value(c, "LK_HUB_AUDIENCE")
	msg := sprintf("%s, container %s: LK_PUBLIC_ORIGIN and LK_HUB_AUDIENCE must be one name, https://<the hub's host name> (README item 9)", [lk.who(sts), c.name])
}

deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	origin := lk.env_value(c, "LK_PUBLIC_ORIGIN")
	not regex.match(`^https://[a-z0-9]([a-z0-9.-]*[a-z0-9])?$`, origin)
	msg := sprintf("%s, container %s: LK_PUBLIC_ORIGIN must be https://<host> in lower case, with no port, path or credential (HARDENING.md)", [lk.who(sts), c.name])
}

deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	origin := lk.env_value(c, "LK_PUBLIC_ORIGIN")
	some g in lk.of_kind("Gateway")
	some l in object.get(g.spec, "listeners", [])
	origin != concat("", ["https://", l.hostname])
	msg := sprintf("%s: the Gateway serves %q and LK_PUBLIC_ORIGIN is %q; the users type the name the Gateway serves, and the hub checks Host and Origin against LK_PUBLIC_ORIGIN (README item 9)", [lk.who(g), l.hostname, origin])
}

# The kubelet stops a hub that is still draining when terminationGracePeriodSeconds runs out. The hub fails /readyz, waits
# LK_SHUTDOWN_GRACE_SECS, drains for LK_SHUTDOWN_DRAIN_SECS and exits; the period must be longer than the two by 10 s
# (crates/hub/tests/hardening.rs holds the Pod block to the same sum). The defaults are the hub's: 5 and 25.
env_seconds(c, name, fallback) := to_number(lk.env_value(c, name))

env_seconds(c, name, fallback) := fallback if not lk.env_value(c, name)

deny contains msg if {
	some sts in lk.hub_statefulsets
	some c in sts.spec.template.spec.containers
	need := (env_seconds(c, "LK_SHUTDOWN_GRACE_SECS", 5) + env_seconds(c, "LK_SHUTDOWN_DRAIN_SECS", 25)) + 10
	got := object.get(sts.spec.template.spec, "terminationGracePeriodSeconds", 30)
	got < need
	msg := sprintf("%s: terminationGracePeriodSeconds is %v and the hub needs %v (LK_SHUTDOWN_GRACE_SECS + LK_SHUTDOWN_DRAIN_SECS + 10); the kubelet would kill a hub that is still draining (README item 5)", [lk.who(sts), got, need])
}

# --- the hub image's Jobs (crates/hub/HARDENING.md, "The migration Job" and "The seed Job"; 09/fix, J1 to J6) -------------------------------
#
# `hub migrate` runs as the migrator, before the hub rolls out; `hub seed` runs as the application role, after the migration. Each is a Job
# from the hub's own image with `args` only, no port, the hub pod's security context, and the volumes of its table: the migrator's credential
# (or the application role's) and the database CA, read-only, and `/tmp`; the seed adds its file, from a ConfigMap. Neither is given the other's
# credential, and neither has a Google identity, so a Job pod cannot reach Secret Manager or KMS. The code refuses the wrong role as well; this
# is the second wall.

migrate_job_name := "lanekeeper-hub-migrate"

jobs_name := "lanekeeper-hub-jobs"

jobs_hook_events := "pre-install,pre-upgrade"

job_kind(d) := "migrate" if d.metadata.name == migrate_job_name

job_kind(d) := "seed" if startswith(d.metadata.name, "lanekeeper-hub-seed-")

job_contracts := {
	"migrate": {
		"args": ["migrate"],
		"env": {"LK_MIGRATOR_URL_FILE": "/etc/lanekeeper/migrator/uri", "LK_DB_CA_FILE": "/etc/lanekeeper/db-ca/ca.crt"},
		"timeout": "LK_MIGRATE_TIMEOUT_SECS",
		"timeout_default": 600,
		"timeout_max": 7200,
		"mounts": {
			["tmp", "/tmp", false],
			["migrator-credentials", "/etc/lanekeeper/migrator", true],
			["db-ca", "/etc/lanekeeper/db-ca", true],
		},
	},
	"seed": {
		"args": ["seed", "/etc/lanekeeper/seed/seed.yaml"],
		"env": {"LK_DATABASE_URL_FILE": "/etc/lanekeeper/db/uri", "LK_DB_CA_FILE": "/etc/lanekeeper/db-ca/ca.crt"},
		"timeout": "LK_SEED_TIMEOUT_SECS",
		"timeout_default": 120,
		"timeout_max": 3600,
		"mounts": {
			["tmp", "/tmp", false],
			["db-credentials", "/etc/lanekeeper/db", true],
			["db-ca", "/etc/lanekeeper/db-ca", true],
			["seed", "/etc/lanekeeper/seed", true],
		},
	},
}

# The Jobs by kind, with their contract. A Job of the hub's image that is neither is refused below.
hub_job_list := [{"doc": d, "kind": job_kind(d), "spec": d.spec.template.spec, "contract": job_contracts[job_kind(d)]} |
	some d in lk.hub_jobs
	job_kind(d)
]

job_containers := [{"job": j, "container": c} |
	some j in hub_job_list
	some c in lk.all_containers(j.spec)
]

deny contains msg if {
	some d in lk.hub_jobs
	not job_kind(d)
	msg := sprintf("%s: the hub's image has only two Jobs, %s and lanekeeper-hub-seed-<hash> (HARDENING.md)", [lk.who(d), migrate_job_name])
}

# The hub refuses to start on an unmigrated database and nothing else changes the schema (S9, S21).
deny contains msg if {
	count(lk.hub_statefulsets) > 0
	count(lk.named("Job", migrate_job_name)) == 0
	msg := "Job/lanekeeper-hub-migrate is missing: there is no migration Job, and nothing else changes the schema; the hub exits 1 on an unmigrated database (HARDENING.md)"
}

deny contains msg if {
	some j in hub_job_list
	count(lk.all_containers(j.spec)) != 1
	msg := sprintf("%s: a Job has exactly one container and no init container (HARDENING.md)", [lk.who(j.doc)])
}

# --- the image, the command and the ports

hub_images := {c.image | some sts in lk.hub_statefulsets; some c in sts.spec.template.spec.containers}

deny contains msg if {
	some x in job_containers
	not x.container.image in hub_images
	msg := sprintf("%s, container %s: it must run the hub's image, the very digest the StatefulSet runs (HARDENING.md)", [lk.who(x.job.doc), x.container.name])
}

deny contains msg if {
	some x in job_containers
	object.get(x.container, "args", null) != x.job.contract.args
	msg := sprintf("%s, container %s: args must be exactly %v (HARDENING.md)", [lk.who(x.job.doc), x.container.name, x.job.contract.args])
}

deny contains msg if {
	some x in job_containers
	"command" in object.keys(x.container)
	msg := sprintf("%s, container %s: command is set; the image's ENTRYPOINT is the hub, and a Job passes only args (HARDENING.md)", [lk.who(x.job.doc), x.container.name])
}

deny contains msg if {
	some x in job_containers
	count(object.get(x.container, "ports", [])) > 0
	msg := sprintf("%s, container %s: a Job has no ports; it listens on nothing (HARDENING.md)", [lk.who(x.job.doc), x.container.name])
}

# --- the volumes: only the table's, each read-only but /tmp

deny contains msg if {
	some x in job_containers
	got := {[m.name, m.mountPath, object.get(m, "readOnly", false)] | some m in object.get(x.container, "volumeMounts", [])}
	got != x.job.contract.mounts
	msg := sprintf("%s, container %s: volumeMounts must be exactly %v (name, path, readOnly); found %v (HARDENING.md)", [lk.who(x.job.doc), x.container.name, sort(x.job.contract.mounts), sort(got)])
}

deny contains msg if {
	some j in hub_job_list
	got := {v.name | some v in object.get(j.spec, "volumes", [])}
	want := {m[0] | some m in j.contract.mounts}
	got != want
	msg := sprintf("%s: volumes must be exactly %v; found %v (HARDENING.md)", [lk.who(j.doc), sort(want), sort(got)])
}

# The migrator's credential is never in a pod that is given the application role's, and the seed runs as the role the hub runs as: the
# Secret it mounts is the one the hub mounts.
hub_app_secrets := {v.secret.secretName |
	some sts in lk.hub_statefulsets
	some v in sts.spec.template.spec.volumes
	v.name == "db-credentials"
}

deny contains msg if {
	some j in hub_job_list
	j.kind == "migrate"
	some v in object.get(j.spec, "volumes", [])
	v.name == "migrator-credentials"
	v.secret.secretName in hub_app_secrets
	msg := sprintf("%s: volume migrator-credentials is the application role's Secret; the migrator has a Secret of its own, and the hub's credential must not run migrations (S9, S21)", [lk.who(j.doc)])
}

deny contains msg if {
	some j in hub_job_list
	j.kind == "seed"
	some v in object.get(j.spec, "volumes", [])
	v.name == "db-credentials"
	not v.secret.secretName in hub_app_secrets
	msg := sprintf("%s: volume db-credentials must be the Secret the hub mounts, the application role's; the seed refuses any other role, and the migrator's credential does not belong in it (HARDENING.md)", [lk.who(j.doc)])
}

seed_items := [{"key": "seed.yaml", "path": "seed.yaml"}]

deny contains msg if {
	some j in hub_job_list
	j.kind == "seed"
	some v in object.get(j.spec, "volumes", [])
	v.name == "seed"
	not seed_file_volume(v)
	msg := sprintf("%s: the seed file must come from a ConfigMap, through the one key seed.yaml (it names service accounts and holds no secret; HARDENING.md)", [lk.who(j.doc)])
}

seed_file_volume(v) if {
	v.configMap.items == seed_items
	not v.secret
}

# --- the variables: the Job's own, with the values of its mounts

deny contains msg if {
	some x in job_containers
	some e in object.get(x.container, "env", [])
	allowed := object.union({n: true | some n, _ in x.job.contract.env}, {x.job.contract.timeout: true})
	not e.name in object.keys(allowed)
	msg := sprintf("%s, container %s: %s is not a variable this Job may carry; only %v (HARDENING.md)", [lk.who(x.job.doc), x.container.name, e.name, sort(object.keys(allowed))])
}

deny contains msg if {
	some x in job_containers
	some name, _ in x.job.contract.env
	not lk.env_value(x.container, name)
	msg := sprintf("%s, container %s: %s is not set to a value (HARDENING.md)", [lk.who(x.job.doc), x.container.name, name])
}

deny contains msg if {
	some x in job_containers
	some name, want in x.job.contract.env
	got := lk.env_value(x.container, name)
	got != want
	msg := sprintf("%s, container %s: %s must be %s, the file its volume mounts (HARDENING.md)", [lk.who(x.job.doc), x.container.name, name, want])
}

deny contains msg if {
	some x in job_containers
	value := lk.env_value(x.container, x.job.contract.timeout)
	not job_timeout_ok(value, x.job.contract.timeout_max)
	msg := sprintf("%s, container %s: %s must be a whole number of seconds from 1 to %d, as the hub reads it (HARDENING.md)", [lk.who(x.job.doc), x.container.name, x.job.contract.timeout, x.job.contract.timeout_max])
}

job_timeout_ok(value, max) if {
	regex.match(`^[0-9]{1,5}$`, value)
	n := to_number(value)
	n >= 1
	n <= max
}

# --- the Job's own life: no restart in place, a few tries, and a deadline after the process's own timeout

deny contains msg if {
	some j in hub_job_list
	object.get(j.spec, "restartPolicy", "") != "Never"
	msg := sprintf("%s: restartPolicy must be Never: a retry is a new pod with its own log, and the Job counts the tries (HARDENING.md)", [lk.who(j.doc)])
}

max_backoff := 3

deny contains msg if {
	some j in hub_job_list
	n := object.get(j.doc.spec, "backoffLimit", null)
	not is_number(n)
	msg := sprintf("%s: backoffLimit is not set, and Kubernetes would try 6 times; set at most %d", [lk.who(j.doc), max_backoff])
}

deny contains msg if {
	some j in hub_job_list
	n := object.get(j.doc.spec, "backoffLimit", null)
	is_number(n)
	n > max_backoff
	msg := sprintf("%s: backoffLimit is %v; a few tries are enough, at most %d", [lk.who(j.doc), n, max_backoff])
}

# What the process is told it may take (its own timeout, which ends it with a message that names the step) and what the Job allows: the
# Job's deadline is longer, so that the process's message is what the log shows, and bounded, so that a stuck Job ends.
max_deadline := 10800

job_timeout_secs(j) := to_number(lk.env_value(c, j.contract.timeout)) if {
	some c in j.spec.containers
	lk.env_value(c, j.contract.timeout)
}

job_timeout_secs(j) := j.contract.timeout_default if {
	every c in object.get(j.spec, "containers", []) {
		not lk.env_value(c, j.contract.timeout)
	}
}

deny contains msg if {
	some j in hub_job_list
	not is_number(object.get(j.doc.spec, "activeDeadlineSeconds", null))
	msg := sprintf("%s: activeDeadlineSeconds is not set, so a stuck Job would run for ever (HARDENING.md)", [lk.who(j.doc)])
}

deny contains msg if {
	some j in hub_job_list
	deadline := j.doc.spec.activeDeadlineSeconds
	is_number(deadline)
	timeout := job_timeout_secs(j)
	deadline <= timeout
	msg := sprintf("%s: activeDeadlineSeconds (%v) must be longer than %s (%v), so that the process ends itself with a message that names the step", [lk.who(j.doc), deadline, j.contract.timeout, timeout])
}

deny contains msg if {
	some j in hub_job_list
	deadline := j.doc.spec.activeDeadlineSeconds
	is_number(deadline)
	deadline > max_deadline
	msg := sprintf("%s: activeDeadlineSeconds is %v; a Job that has not finished in %d s is stuck", [lk.who(j.doc), deadline, max_deadline])
}

# --- who the Jobs are: their own account, with no Google identity and no token (S5, S7, S16)

job_accounts := lk.named("ServiceAccount", jobs_name)

deny contains msg if {
	some j in hub_job_list
	object.get(j.spec, "serviceAccountName", "default") != jobs_name
	msg := sprintf("%s: serviceAccountName must be %s, the ServiceAccount of the Jobs; the hub's has a Google identity (Secret Manager and KMS), which a Job must not have (S5, S7)", [lk.who(j.doc), jobs_name])
}

deny contains msg if {
	count(lk.hub_jobs) > 0
	count(job_accounts) == 0
	msg := sprintf("ServiceAccount/%s is missing: the Jobs run as it, and on a first install it must exist before the migration pod is made (HARDENING.md)", [jobs_name])
}

deny contains msg if {
	some sa in job_accounts
	some key, _ in object.get(sa.metadata, "annotations", {})
	startswith(key, "iam.gke.io/")
	msg := sprintf("%s: the annotation %s gives it a Google identity; a Job reaches the database and nothing in Google (S5, S7)", [lk.who(sa), key])
}

deny contains msg if {
	some j in hub_job_list
	some key, _ in object.get(j.doc.spec.template.metadata, "annotations", {})
	startswith(key, "iam.gke.io/")
	msg := sprintf("%s: the pod's annotation %s gives it a Google identity (S5, S7)", [lk.who(j.doc), key])
}

deny contains msg if {
	some sa in job_accounts
	object.get(sa, "automountServiceAccountToken", null) != false
	msg := sprintf("%s: automountServiceAccountToken must be stated and false; the Jobs never call the Kubernetes API (S16)", [lk.who(sa)])
}

# The label the Jobs' NetworkPolicy selects and the database admits. It is not the hub's, or the hub's policy (the other replicas, the
# metadata server, the external hosts) would select the Jobs' pods too.
deny contains msg if {
	some j in hub_job_list
	object.get(j.doc.spec.template.metadata, ["labels", "app.kubernetes.io/name"], "") != jobs_name
	msg := sprintf("%s: the pod's label app.kubernetes.io/name must be %s, which the Jobs' NetworkPolicy selects and the database admits; the hub's label would give it the hub's network (S18)", [lk.who(j.doc), jobs_name])
}

# --- the order: the account and the policy first, then the migration (hooks), then the seed (not a hook) ---------------------------------------

hook_weight(d) := to_number(object.get(d.metadata, ["annotations", "helm.sh/hook-weight"], "0"))

deny contains msg if {
	some j in hub_job_list
	j.kind == "migrate"
	object.get(j.doc.metadata, ["annotations", "helm.sh/hook"], "") != jobs_hook_events
	msg := sprintf("%s: helm.sh/hook must be %s (Argo CD runs it as PreSync): the schema is complete before the hub rolls out (HARDENING.md)", [lk.who(j.doc), jobs_hook_events])
}

deny contains msg if {
	some j in hub_job_list
	j.kind == "seed"
	some key, _ in object.get(j.doc.metadata, "annotations", {})
	startswith(key, "helm.sh/hook")
	msg := sprintf("%s: the seed Job is a Job, not a hook (%s): it runs after the migration, and a missing seed must not stop a deploy (README item 2, step f)", [lk.who(j.doc), key])
}

deny contains msg if {
	some j in hub_job_list
	j.kind == "seed"
	"argocd.argoproj.io/hook" in object.keys(object.get(j.doc.metadata, "annotations", {}))
	msg := sprintf("%s: the seed Job is a Job, not a hook (argocd.argoproj.io/hook): it runs after the migration, and a missing seed must not stop a deploy (README item 2, step f)", [lk.who(j.doc)])
}

# What the Jobs need before the migration pod is made: on a first install nothing of the chart's other resources exists yet. Each is a hook
# of the same events with a lower weight. It is kept (before-hook-creation): the seed Job, which is not a hook, runs as it and under its policy.
jobs_prerequisites := array.concat(job_accounts, lk.named("NetworkPolicy", jobs_name))

deny contains msg if {
	count(lk.named("Job", migrate_job_name)) > 0
	some d in jobs_prerequisites
	object.get(d.metadata, ["annotations", "helm.sh/hook"], "") != jobs_hook_events
	msg := sprintf("%s: it must be a hook of %s, like the migration Job, so that it exists on a first install when the migration pod is made (helm.sh/hook)", [lk.who(d), jobs_hook_events])
}

deny contains msg if {
	some d in jobs_prerequisites
	some job in lk.named("Job", migrate_job_name)
	hook_weight(d) >= hook_weight(job)
	msg := sprintf("%s: its helm.sh/hook-weight (%v) must be lower than the migration Job's (%v), so that it exists when the migration pod is made", [lk.who(d), hook_weight(d), hook_weight(job)])
}

deny contains msg if {
	some d in jobs_prerequisites
	object.get(d.metadata, ["annotations", "helm.sh/hook"], "") == jobs_hook_events
	object.get(d.metadata, ["annotations", "helm.sh/hook-delete-policy"], "") != "before-hook-creation"
	msg := sprintf("%s: helm.sh/hook-delete-policy must be before-hook-creation; hook-succeeded would delete it after the migration, before the seed Job (which is not a hook) uses it", [lk.who(d)])
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
