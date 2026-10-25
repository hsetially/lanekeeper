# Secrets policy (09/T3; S7). Run with `conftest test --combine --namespace secrets` over a chart's rendered output and its
# values files. Secrets are never in Git, never in the chart's output and never in a pod's environment: the hub reads its
# database URL from a mounted file and everything else from Secret Manager (crates/hub/HARDENING.md).
package secrets

import rego.v1

import data.lk

# --- Secret objects ----------------------------------------------------------------------------------------------------

# The only Secret a chart may render: the agent's certificate Secret (plan 09 A14), empty, so that ArgoCD's self-heal has
# nothing to restore. CloudNativePG's credentials are created by the operator or by a runbook step, not rendered.
allowed_secrets := {"lanekeeper-agent-cert"}

deny contains msg if {
	some s in lk.of_kind("Secret")
	not s.metadata.name in allowed_secrets
	msg := sprintf("Secret/%s: charts render no Secret; secrets come from Secret Manager, and CloudNativePG creates its own (S7)", [s.metadata.name])
}

deny contains msg if {
	some s in lk.of_kind("Secret")
	s.metadata.name in allowed_secrets
	some field in ["data", "stringData"]
	count(object.get(s, field, {})) > 0
	msg := sprintf("Secret/%s must be rendered empty: the agent writes its content at run time (S7, plan 09 A14)", [s.metadata.name])
}

# --- the environment ----------------------------------------------------------------------------------------------------

deny contains msg if {
	some x in lk.containers
	some e in object.get(x.container, "env", [])
	"secretKeyRef" in object.keys(object.get(e, "valueFrom", {}))
	msg := sprintf("%s, container %s: %s takes its value from a secretKeyRef; secrets reach a pod as files or through Secret Manager (S7)", [lk.who(x.doc), x.container.name, e.name])
}

deny contains msg if {
	some x in lk.containers
	some from in object.get(x.container, "envFrom", [])
	"secretRef" in object.keys(from)
	msg := sprintf("%s, container %s: envFrom takes a whole Secret (secretRef) into the environment (S7)", [lk.who(x.doc), x.container.name])
}

secret_markers := ["PASSWORD", "TOKEN", "SECRET", "CREDENTIAL"]

secret_looking(name) if {
	some marker in secret_markers
	contains(upper(name), marker)
}

secret_looking(name) if endswith(upper(name), "_KEY")

deny contains msg if {
	some x in lk.containers
	some e in object.get(x.container, "env", [])
	secret_looking(e.name)
	msg := sprintf("%s, container %s: %s looks like a secret in the environment (S7)", [lk.who(x.doc), x.container.name, e.name])
}

deny contains msg if {
	some x in lk.containers
	some e in object.get(x.container, "env", [])
	e.name == "LK_DATABASE_URL"
	msg := sprintf("%s, container %s: LK_DATABASE_URL puts the database password in the environment; use LK_DATABASE_URL_FILE (S7)", [lk.who(x.doc), x.container.name])
}

# --- values files -------------------------------------------------------------------------------------------------------

# A values file is `values.yaml` of a chart or anything under a `values/` directory. Only these are scanned for secrets, so a
# rendered document that has a field called `token` (a Role's resource name, say) is not mistaken for one.
values_file(f) if regex.match(`(^|/)values(\.ya?ml$|/)`, f.path)

secret_key_names := {
	"password", "passwd", "pwd", "token", "secret", "clientsecret", "privatekey", "apikey", "accesskey", "secretkey",
	"authtoken", "accesstoken", "credential", "credentials",
}

path_text(path) := concat(".", [sprintf("%v", [p]) | some p in path])

deny contains msg if {
	some f in input
	values_file(f)
	walk(f.contents, [path, value])
	count(path) > 0
	key := path[count(path) - 1]
	is_string(key)
	lower(key) in secret_key_names
	is_string(value)
	value != ""
	msg := sprintf("%s: %s holds a value; the key %q is where a secret goes, and secrets do not belong in values files (S7)", [f.path, path_text(path), key])
}

secret_patterns := {
	`-----BEGIN [A-Z ]*PRIVATE KEY-----`: "a private key",
	`\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}`: "a GitHub token",
	`\bgithub_pat_[A-Za-z0-9_]{20,}`: "a GitHub token",
	`\bAIza[0-9A-Za-z_\-]{35}`: "a Google API key",
	`\bAKIA[0-9A-Z]{16}\b`: "an AWS access key",
	`\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]+`: "a signed token (JWT)",
	`\bxox[abprs]-[A-Za-z0-9-]{10,}`: "a Slack token",
}

deny contains msg if {
	some f in input
	values_file(f)
	walk(f.contents, [path, value])
	is_string(value)
	some pattern, what in secret_patterns
	regex.match(pattern, value)
	msg := sprintf("%s: %s looks like %s; secrets do not belong in values files (S7)", [f.path, path_text(path), what])
}
