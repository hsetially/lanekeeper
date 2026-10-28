# Database policy (09/T5; S6, S7, S10, S16, S18, S19, D29, D40, D62, D124). Run with `conftest test --combine --namespace database` over
# the db chart's rendered output. The CloudNativePG cluster has three instances spread over zones, takes TLS 1.3 only and refuses a
# connection without TLS before any other rule, has the two roles of db/migrations/README.md and nothing else, and backs up to
# Google Cloud Storage through Workload Identity, for 30 days, with no key file. Who may reach it is its own NetworkPolicy: the hub,
# the operator, and the instances themselves. Pod hardening of the instances is stated in the Cluster, since the operator makes the
# pods (the rules for pods in hardening.rego never see them).
package database

import rego.v1

import data.lk

cluster_name := "lanekeeper-db"

app_role := "lanekeeper_app"

migrator_role := "lanekeeper_migrator"

environments := {"kind", "staging", "prod"}

all_clusters := [c | some c in lk.of_kind("Cluster"); startswith(c.apiVersion, "postgresql.cnpg.io/")]

clusters := [c | some c in all_clusters; c.metadata.name == cluster_name]

environment(c) := object.get(c.metadata, ["labels", "lanekeeper.io/environment"], "")

in_kind(c) if environment(c) == "kind"

spec_get(c, path, default_value) := object.get(c.spec, path, default_value)

# --- which cluster -----------------------------------------------------------------------------------------------------------

deny contains msg if {
	some c in all_clusters
	c.metadata.name != cluster_name
	msg := sprintf("%s: the only database is %s (the hub's egress and this policy name it); another cluster is not part of this chart", [lk.who(c), cluster_name])
}

deny contains msg if {
	some c in clusters
	not environment(c) in environments
	msg := sprintf("%s: the label lanekeeper.io/environment must be one of %v, and is %q; what is required depends on it (kind has no Google)", [lk.who(c), sort(environments), environment(c)])
}

# --- three instances, spread (D62) ---------------------------------------------------------------------------------------------

deny contains msg if {
	some c in clusters
	spec_get(c, "instances", 0) != 3
	msg := sprintf("%s: the database runs 3 instances (D62), not %v", [lk.who(c), spec_get(c, "instances", "unset")])
}

deny contains msg if {
	some c in clusters
	spec_get(c, "enablePDB", false) != true
	msg := sprintf("%s: enablePDB must be stated and true, so that a node drain cannot take the primary and a replica at once (D62)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	spec_get(c, ["affinity", "enablePodAntiAffinity"], false) != true
	msg := sprintf("%s: affinity.enablePodAntiAffinity must be true, so that two instances never share a node (D62)", [lk.who(c)])
}

# Everywhere but kind the instances must repel one another across zones, as a requirement. kind has one zone: its instances prefer
# different hosts, the label says so, and the policy warns every time (plan 09 A13).
deny contains msg if {
	some c in clusters
	not in_kind(c)
	spec_get(c, ["affinity", "topologyKey"], "") != "topology.kubernetes.io/zone"
	msg := sprintf("%s: affinity.topologyKey must be topology.kubernetes.io/zone, so that the instances are in different zones (D62)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	not in_kind(c)
	spec_get(c, ["affinity", "podAntiAffinityType"], "") != "required"
	msg := sprintf("%s: affinity.podAntiAffinityType must be required, not a preference (D62)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	in_kind(c)
	spec_get(c, ["affinity", "topologyKey"], "") != "kubernetes.io/hostname"
	msg := sprintf("%s: on kind the instances must at least prefer different hosts: affinity.topologyKey kubernetes.io/hostname (plan 09 A13)", [lk.who(c)])
}

warn contains msg if {
	some c in clusters
	in_kind(c)
	msg := sprintf("%s: spread over hosts, not zones, because the cluster is kind (plan 09 A13); never use this in a real cluster", [lk.who(c)])
}

# --- the image (S19) -----------------------------------------------------------------------------------------------------------

image_name(c) := spec_get(c, "imageName", "")

digest_pinned(ref) if regex.match(`^[a-z0-9.\-/:_]+@sha256:[0-9a-f]{64}$`, ref)

deny contains msg if {
	some c in clusters
	not digest_pinned(image_name(c))
	msg := sprintf("%s: image %q is not pinned by digest (S19)", [lk.who(c), image_name(c)])
}

deny contains msg if {
	some c in clusters
	endswith(image_name(c), "@sha256:0000000000000000000000000000000000000000000000000000000000000000")
	msg := sprintf("%s: the image digest is the placeholder; put the digest of a published image in the values (S19)", [lk.who(c)])
}

# The image has pgvector and the locales (and barman, for the backups below): CloudNativePG's own, or a custom one that this
# policy is then changed to name (plan 09 A26).
deny contains msg if {
	some c in clusters
	digest_pinned(image_name(c))
	not startswith(image_name(c), "ghcr.io/cloudnative-pg/postgresql:")
	msg := sprintf("%s: image %q is not ghcr.io/cloudnative-pg/postgresql, the image with pgvector (A26)", [lk.who(c), image_name(c)])
}

# --- TLS and authentication (S6) -----------------------------------------------------------------------------------------------

parameters(c) := spec_get(c, ["postgresql", "parameters"], {})

deny contains msg if {
	some c in clusters
	object.get(parameters(c), "ssl_min_protocol_version", "") != "TLSv1.3"
	msg := sprintf("%s: postgresql.parameters.ssl_min_protocol_version must be TLSv1.3 (S6)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	object.get(parameters(c), "ssl_max_protocol_version", "TLSv1.3") != "TLSv1.3"
	msg := sprintf("%s: postgresql.parameters.ssl_max_protocol_version must be TLSv1.3, or left out (S6)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	object.get(parameters(c), "password_encryption", "") != "scram-sha-256"
	msg := sprintf("%s: postgresql.parameters.password_encryption must be scram-sha-256 (S6)", [lk.who(c)])
}

# Only these. Another parameter (fsync, synchronous_commit, ssl, the planner's costs, which the plans of the registry's tests were taken
# with) is a decision, and goes here with a test.
allowed_parameters := {"ssl_min_protocol_version", "ssl_max_protocol_version", "password_encryption", "shared_buffers"}

deny contains msg if {
	some c in clusters
	some name, _ in parameters(c)
	not name in allowed_parameters
	msg := sprintf("%s: postgresql.parameters.%s is not one of the parameters this chart sets, %v (S6, D124)", [lk.who(c), name, sort(allowed_parameters)])
}

hba(c) := spec_get(c, ["postgresql", "pg_hba"], [])

squash(rule) := regex.replace(trim_space(rule), `\s+`, " ")

reject_plain := "hostnossl all all all reject"

# The generated pg_hba.conf has the operator's fixed rules (local peer, and certificates for replication), then these, then the default
# `host all all all scram-sha-256` (CloudNativePG, postgresql_conf). The first user rule refuses every connection without TLS.
deny contains msg if {
	some c in clusters
	count(hba(c)) == 0
	msg := sprintf("%s: postgresql.pg_hba is empty; the first rule must be `%s`, or a connection without TLS would reach the default rule (S6)", [lk.who(c), reject_plain])
}

deny contains msg if {
	some c in clusters
	count(hba(c)) > 0
	squash(hba(c)[0]) != reject_plain
	msg := sprintf("%s: the first rule of postgresql.pg_hba must be `%s`, and is `%s` (S6)", [lk.who(c), reject_plain, hba(c)[0]])
}

weak_methods := {"trust", "md5", "password"}

deny contains msg if {
	some c in clusters
	some rule in hba(c)
	some token in regex.split(`\s+`, trim_space(rule))
	token in weak_methods
	msg := sprintf("%s: the pg_hba rule `%s` uses %s, which does not protect the password or does not ask for one (S6)", [lk.who(c), rule, token])
}

deny contains msg if {
	some c in clusters
	some i, rule in hba(c)
	i > 0
	startswith(squash(rule), "hostnossl ")
	msg := sprintf("%s: the pg_hba rule `%s` is a hostnossl rule after the first; only the first, which rejects, may be (S6)", [lk.who(c), rule])
}

deny contains msg if {
	some c in clusters
	spec_get(c, "enableSuperuserAccess", null) != false
	msg := sprintf("%s: enableSuperuserAccess must be stated and false; the postgres password would otherwise be a Secret (S7)", [lk.who(c)])
}

# --- the database and its roles (D124, db/migrations/README.md) --------------------------------------------------------------------

initdb(c) := spec_get(c, ["bootstrap", "initdb"], {})

deny contains msg if {
	some c in clusters
	object.get(initdb(c), "owner", "") != migrator_role
	msg := sprintf("%s: the owner of the database must be %s, which runs the migrations; the app role owns nothing (db/migrations/README.md)", [lk.who(c), migrator_role])
}

deny contains msg if {
	some c in clusters
	object.get(initdb(c), "database", "") != "lanekeeper"
	msg := sprintf("%s: bootstrap.initdb.database must be lanekeeper", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	some field in ["localeCollate", "localeCType"]
	object.get(initdb(c), field, "") != "en_US.utf8"
	msg := sprintf("%s: bootstrap.initdb.%s must be en_US.utf8, the collation the registry's plans were taken on (D124)", [lk.who(c), field])
}

deny contains msg if {
	some c in clusters
	object.get(initdb(c), "encoding", "") != "UTF8"
	msg := sprintf("%s: bootstrap.initdb.encoding must be UTF8", [lk.who(c)])
}

init_sql(c) := object.get(initdb(c), "postInitApplicationSQL", [])

create_extension(extension) := sprintf("(?i)^\\s*create\\s+extension\\s+if\\s+not\\s+exists\\s+%s\\s*;?\\s*$", [extension])

has_extension(c, extension) if {
	some s in init_sql(c)
	regex.match(create_extension(extension), s)
}

deny contains msg if {
	some c in clusters
	some extension in ["vector", "pg_trgm"]
	not has_extension(c, extension)
	msg := sprintf("%s: bootstrap.initdb.postInitApplicationSQL does not create the extension %s, which the migrations need (db/migrations/README.md)", [lk.who(c), extension])
}

# Only the two extensions. The app role is created after the bootstrap (it is a managed role), so a statement that names it would fail
# the bootstrap, and a grant is what 0001_init.sql and the migrations that follow it decide, table by table.
deny contains msg if {
	some c in clusters
	some s in init_sql(c)
	not regex.match(create_extension("(vector|pg_trgm)"), s)
	msg := sprintf("%s: bootstrap.initdb.postInitApplicationSQL has `%s`; it may only create the extensions vector and pg_trgm, and the grants belong to the migrations (db/migrations/README.md)", [lk.who(c), s])
}

initdb_secret(c) := object.get(initdb(c), ["secret", "name"], "")

managed_roles(c) := spec_get(c, ["managed", "roles"], [])

app_roles(c) := [r | some r in managed_roles(c); r.name == app_role]

deny contains msg if {
	some c in clusters
	initdb_secret(c) == ""
	msg := sprintf("%s: bootstrap.initdb.secret.name is empty: the operator would make the owner's credentials in %s-app, the Secret the hub reads its own from", [lk.who(c), cluster_name])
}

deny contains msg if {
	some c in clusters
	initdb_secret(c) == sprintf("%s-app", [cluster_name])
	msg := sprintf("%s: the migrator's Secret is %s, the name the operator and the hub use for the app role's credentials", [lk.who(c), initdb_secret(c)])
}

deny contains msg if {
	some c in clusters
	some r in app_roles(c)
	initdb_secret(c) != ""
	initdb_secret(c) == object.get(r, ["passwordSecret", "name"], "")
	msg := sprintf("%s: the owner %s and the role %s read their passwords from the same Secret %s, so the app could log in as the owner", [lk.who(c), migrator_role, app_role, initdb_secret(c)])
}

deny contains msg if {
	some c in clusters
	count(app_roles(c)) != 1
	msg := sprintf("%s: %s is not a managed role (exactly one is expected); the hub connects as it (db/migrations/README.md)", [lk.who(c), app_role])
}

deny contains msg if {
	some c in clusters
	some r in managed_roles(c)
	r.name != app_role
	msg := sprintf("%s: another managed role, %q; the roles are %s and %s, and nothing else may log in (db/migrations/README.md)", [lk.who(c), r.name, migrator_role, app_role])
}

deny contains msg if {
	some c in clusters
	some r in app_roles(c)
	some flag in ["superuser", "createdb", "createrole", "replication", "bypassrls"]
	object.get(r, flag, null) != false
	msg := sprintf("%s: the managed role %s must state %s: false (db/migrations/README.md)", [lk.who(c), app_role, flag])
}

deny contains msg if {
	some c in clusters
	some r in app_roles(c)
	object.get(r, "login", false) != true
	msg := sprintf("%s: the managed role %s must have login: true; the hub connects as it", [lk.who(c), app_role])
}

deny contains msg if {
	some c in clusters
	some r in app_roles(c)
	object.get(r, "ensure", "present") != "present"
	msg := sprintf("%s: the managed role %s must be ensure: present", [lk.who(c), app_role])
}

deny contains msg if {
	some c in clusters
	some r in app_roles(c)
	object.get(r, ["passwordSecret", "name"], "") == ""
	msg := sprintf("%s: the managed role %s has no passwordSecret.name; its password comes from a Secret made out of band (plan 09 A7)", [lk.who(c), app_role])
}

deny contains msg if {
	some c in clusters
	some r in app_roles(c)
	object.get(r, "disablePassword", false) == true
	msg := sprintf("%s: the managed role %s has disablePassword: true; it must log in with its password (S6)", [lk.who(c), app_role])
}

deny contains msg if {
	some c in clusters
	some r in app_roles(c)
	count(object.get(r, "inRoles", [])) > 0
	msg := sprintf("%s: the managed role %s is a member of other roles (inRoles); its rights are the table grants of the migrations and nothing else (db/migrations/README.md)", [lk.who(c), app_role])
}

# --- backups (D29, D40, S10) -----------------------------------------------------------------------------------------------------

backup(c) := spec_get(c, "backup", {})

store(c) := object.get(backup(c), "barmanObjectStore", {})

has_backup(c) if count(backup(c)) > 0

deny contains msg if {
	some c in clusters
	not in_kind(c)
	not has_backup(c)
	msg := sprintf("%s: no backup is configured (spec.backup); %s must be backed up to Google Cloud Storage (D29, D40)", [lk.who(c), environment(c)])
}

warn contains msg if {
	some c in clusters
	in_kind(c)
	not has_backup(c)
	msg := sprintf("%s: no backup, because the cluster is kind and has no Google Cloud Storage; never use this in a real cluster (D40)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	has_backup(c)
	not regex.match(`^gs://[a-z0-9][a-z0-9._-]{1,61}[a-z0-9]/[^/ ].*$`, object.get(store(c), "destinationPath", ""))
	msg := sprintf("%s: backup.barmanObjectStore.destinationPath %q must be gs://BUCKET/PATH, a path inside a Google Cloud Storage bucket (D29)", [lk.who(c), object.get(store(c), "destinationPath", "")])
}

deny contains msg if {
	some c in clusters
	has_backup(c)
	"applicationCredentials" in object.keys(object.get(store(c), "googleCredentials", {}))
	msg := sprintf("%s: backup.barmanObjectStore.googleCredentials.applicationCredentials is a key file in a Secret; the backups use Workload Identity (gkeEnvironment) and no key (S7)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	has_backup(c)
	object.get(store(c), ["googleCredentials", "gkeEnvironment"], false) != true
	msg := sprintf("%s: backup.barmanObjectStore.googleCredentials.gkeEnvironment must be true, so that the backups use Workload Identity (S7)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	has_backup(c)
	object.get(backup(c), "retentionPolicy", "") != "30d"
	msg := sprintf("%s: backup.retentionPolicy must be 30d (D40), and is %q", [lk.who(c), object.get(backup(c), "retentionPolicy", "")])
}

gsa_annotation(c) := object.get(c.spec, ["serviceAccountTemplate", "metadata", "annotations", "iam.gke.io/gcp-service-account"], "")

deny contains msg if {
	some c in clusters
	has_backup(c)
	not regex.match(`^[a-z][a-z0-9-]{4,28}[a-z0-9]@[a-z][a-z0-9-]{4,28}[a-z0-9]\.iam\.gserviceaccount\.com$`, gsa_annotation(c))
	msg := sprintf("%s: serviceAccountTemplate.metadata.annotations must name the Google service account in iam.gke.io/gcp-service-account (Workload Identity for the backups), and has %q (S7)", [lk.who(c), gsa_annotation(c)])
}

scheduled := [b | some b in lk.of_kind("ScheduledBackup"); startswith(b.apiVersion, "postgresql.cnpg.io/")]

backups_of(c) := [b | some b in scheduled; b.spec.cluster.name == c.metadata.name]

deny contains msg if {
	some c in clusters
	has_backup(c)
	count(backups_of(c)) != 1
	msg := sprintf("%s: there must be exactly one ScheduledBackup for it, and there are %d (D40)", [lk.who(c), count(backups_of(c))])
}

deny contains msg if {
	some c in clusters
	some b in backups_of(c)
	object.get(b.spec, "suspend", false) == true
	msg := sprintf("%s: the ScheduledBackup is suspended (spec.suspend), so nothing is backed up (D40)", [lk.who(b)])
}

# Six fields, seconds first: one run a day.
deny contains msg if {
	some c in clusters
	some b in backups_of(c)
	not regex.match(`^[0-9]+ [0-9]+ [0-9]+ \* \* \*$`, object.get(b.spec, "schedule", ""))
	msg := sprintf("%s: the schedule %q is not daily (seconds minutes hours * * *) (D40)", [lk.who(b), object.get(b.spec, "schedule", "")])
}

deny contains msg if {
	some c in clusters
	some b in backups_of(c)
	object.get(b.spec, "method", "") != "barmanObjectStore"
	msg := sprintf("%s: spec.method must be barmanObjectStore, the method the Cluster configures", [lk.who(b)])
}

# --- the pods (S16): the operator makes them, the Cluster states how ------------------------------------------------------------------

deny contains msg if {
	some c in clusters
	spec_get(c, ["seccompProfile", "type"], "") != "RuntimeDefault"
	msg := sprintf("%s: seccompProfile.type must be RuntimeDefault (S16)", [lk.who(c)])
}

security_context(c) := spec_get(c, "securityContext", {})

deny contains msg if {
	some c in clusters
	some field in ["allowPrivilegeEscalation", "privileged"]
	object.get(security_context(c), field, null) != false
	msg := sprintf("%s: securityContext.%s must be stated and false (S16)", [lk.who(c), field])
}

deny contains msg if {
	some c in clusters
	some field in ["readOnlyRootFilesystem", "runAsNonRoot"]
	object.get(security_context(c), field, null) != true
	msg := sprintf("%s: securityContext.%s must be stated and true (S16)", [lk.who(c), field])
}

deny contains msg if {
	some c in clusters
	object.get(security_context(c), ["capabilities", "drop"], []) != ["ALL"]
	msg := sprintf("%s: securityContext.capabilities.drop must be [\"ALL\"] (drop ALL) (S16)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	count(object.get(security_context(c), ["capabilities", "add"], [])) > 0
	msg := sprintf("%s: securityContext.capabilities.add adds capabilities back after drop ALL (S16)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	spec_get(c, ["podSecurityContext", "runAsUser"], 1) == 0
	msg := sprintf("%s: podSecurityContext.runAsUser is 0, which is root (S16)", [lk.who(c)])
}

deny contains msg if {
	some c in clusters
	spec_get(c, ["podSecurityContext", "runAsNonRoot"], true) == false
	msg := sprintf("%s: podSecurityContext.runAsNonRoot is false (S16)", [lk.who(c)])
}

# --- bounded: resources and storage ------------------------------------------------------------------------------------------------------

deny contains msg if {
	some c in clusters
	some pair in [["requests", "cpu"], ["requests", "memory"], ["limits", "cpu"], ["limits", "memory"]]
	spec_get(c, array.concat(["resources"], pair), "") == ""
	msg := sprintf("%s: resources.%s.%s is not set; every container is bounded (S16)", [lk.who(c), pair[0], pair[1]])
}

deny contains msg if {
	some c in clusters
	spec_get(c, ["storage", "size"], "") == ""
	msg := sprintf("%s: storage.size is not set", [lk.who(c)])
}

# --- the disks: a deleted claim does not delete the database ---------------------------------------------------------------------------

deny contains msg if {
	some sc in lk.named("StorageClass", "lanekeeper-db-cmek")
	object.get(sc, "reclaimPolicy", "Delete") != "Retain"
	msg := sprintf("%s: reclaimPolicy must be Retain, so that deleting a claim, by a mistake or a prune, does not delete the disk of the database", [lk.who(sc)])
}

# --- the network (S18): who may reach the database ----------------------------------------------------------------------------------------

db_selector := {"cnpg.io/cluster": cluster_name}

hub_selector := {"app.kubernetes.io/name": "lanekeeper-hub"}

# The pods of the hub image's Jobs (`hub migrate`, `hub seed`; charts/hub): a label of their own, so that the hub's NetworkPolicy does not
# select them, and so that nothing wider than they are admitted to the database.
jobs_selector := {"app.kubernetes.io/name": "lanekeeper-hub-jobs"}

operator_selector := {"app.kubernetes.io/name": "cloudnative-pg"}

db_policies := lk.named("NetworkPolicy", cluster_name)

deny contains msg if {
	count(clusters) > 0
	count(db_policies) == 0
	msg := sprintf("NetworkPolicy/%s is missing: nothing denies by default for the database's pods (S18)", [cluster_name])
}

# Network policies add up: a second one that selects the same pods and admits everyone opens what the first closes. The chart makes
# one policy, and any other in its output is refused.
deny contains msg if {
	count(clusters) > 0
	some p in lk.of_kind("NetworkPolicy")
	p.metadata.name != cluster_name
	msg := sprintf("%s: the database chart makes one NetworkPolicy, %s; policies add up, so another could admit callers that this one does not (S18)", [lk.who(p), cluster_name])
}

deny contains msg if {
	some p in db_policies
	not "Ingress" in {t | some t in object.get(p.spec, "policyTypes", [])}
	msg := sprintf("%s: not default deny: policyTypes must list Ingress (S18)", [lk.who(p)])
}

deny contains msg if {
	some p in db_policies
	object.get(p.spec.podSelector, "matchLabels", {}) != db_selector
	msg := sprintf("%s: it selects pods other than the database's; an empty selector is every pod (S18)", [lk.who(p)])
}

ingress_rules(p) := object.get(p.spec, "ingress", [])

deny contains msg if {
	some p in db_policies
	some rule in ingress_rules(p)
	count(object.get(rule, "from", [])) == 0
	msg := sprintf("%s: an ingress rule names no callers (empty or missing `from` admits everyone) (S18)", [lk.who(p)])
}

deny contains msg if {
	some p in db_policies
	some rule in ingress_rules(p)
	count(object.get(rule, "ports", [])) != 1
	msg := sprintf("%s: an ingress rule must name exactly one port, so that each port has its own callers (S18)", [lk.who(p)])
}

# The callers: the hub's pods and the pods of the hub image's Jobs (same namespace), the operator's pod (named namespace and pod), and the
# instances themselves.
caller(from) := "hub" if {
	every f in from {
		f.podSelector.matchLabels == hub_selector
		not f.namespaceSelector
		not f.ipBlock
	}
}

caller(from) := "jobs" if {
	every f in from {
		f.podSelector.matchLabels == jobs_selector
		not f.namespaceSelector
		not f.ipBlock
	}
}

caller(from) := "operator" if {
	every f in from {
		f.podSelector.matchLabels == operator_selector
		object.get(f.namespaceSelector, "matchLabels", {}) != {}
		{k | some k, _ in f.namespaceSelector.matchLabels} == {"kubernetes.io/metadata.name"}
		not f.ipBlock
	}
}

caller(from) := "peers" if {
	every f in from {
		f.podSelector.matchLabels == db_selector
		not f.namespaceSelector
		not f.ipBlock
	}
}

caller_name(from) := caller(from)

caller_name(from) := "UNRECOGNISED" if not caller(from)

expected_ingress := {"5432 hub", "5432 jobs", "5432 operator", "8000 operator", "5432 peers", "8000 peers"}

ingress_pairs(p) := {sprintf("%d %s", [port.port, caller_name(rule.from)]) |
	some rule in ingress_rules(p)
	count(object.get(rule, "from", [])) > 0
	count(object.get(rule, "ports", [])) == 1
	some port in rule.ports
}

deny contains msg if {
	some p in db_policies
	ingress_pairs(p) != expected_ingress
	msg := sprintf("%s: ingress must be exactly %v; found %v (S18)", [lk.who(p), sort(expected_ingress), sort(ingress_pairs(p))])
}
