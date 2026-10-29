# Tests for hardening.rego: the hub's rendered pod passes, and each planted violation (one per field S16 lists) is refused by
# its own rule. Run with `conftest verify --policy deploy/policy`.
package hardening

import rego.v1

import data.fixtures

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

test_good_passes if {
	count(deny) == 0 with input as fixtures.hub
}

# --- the pod's security context (S16)

test_deny_run_as_non_root_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/securityContext/runAsNonRoot"), "runAsNonRoot")
}

test_deny_run_as_non_root_false if {
	refused(fixtures.sts_set("/spec/template/spec/securityContext/runAsNonRoot", false), "runAsNonRoot")
}

test_deny_run_as_root_user if {
	refused(fixtures.sts_set("/spec/template/spec/securityContext/runAsUser", 0), "runAsUser")
}

test_deny_run_as_user_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/securityContext/runAsUser"), "runAsUser")
}

test_deny_run_as_group_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/securityContext/runAsGroup"), "runAsGroup")
}

test_deny_seccomp_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/securityContext/seccompProfile"), "seccomp")
}

test_deny_seccomp_unconfined if {
	refused(fixtures.sts_set("/spec/template/spec/securityContext/seccompProfile/type", "Unconfined"), "seccomp")
}

# A container may not set its own, weaker value either.
test_deny_container_overrides_non_root if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/securityContext/runAsNonRoot", "value": false}])
	refused(planted, "runAsNonRoot")
}

# --- the container's security context (S16)

test_deny_privilege_escalation_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/securityContext/allowPrivilegeEscalation"), "allowPrivilegeEscalation")
}

test_deny_privilege_escalation_true if {
	refused(fixtures.sts_set("/spec/template/spec/containers/0/securityContext/allowPrivilegeEscalation", true), "allowPrivilegeEscalation")
}

test_deny_privileged if {
	refused(fixtures.sts_set("/spec/template/spec/containers/0/securityContext/privileged", true), "privileged")
}

test_deny_writable_root_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/securityContext/readOnlyRootFilesystem"), "readOnlyRootFilesystem")
}

test_deny_writable_root_false if {
	refused(fixtures.sts_set("/spec/template/spec/containers/0/securityContext/readOnlyRootFilesystem", false), "readOnlyRootFilesystem")
}

test_deny_drop_all_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/securityContext/capabilities"), "drop ALL")
}

test_deny_drop_something_else if {
	refused(fixtures.sts_set("/spec/template/spec/containers/0/securityContext/capabilities/drop", ["NET_RAW"]), "drop ALL")
}

test_deny_added_capability if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/securityContext/capabilities/add", "value": ["NET_ADMIN"]}])
	refused(planted, "adds capabilities")
}

# --- resources (S16, P11)

test_deny_requests_cpu_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/resources/requests/cpu"), "resources.requests.cpu")
}

test_deny_requests_memory_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/resources/requests/memory"), "resources.requests.memory")
}

test_deny_limits_cpu_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/resources/limits/cpu"), "resources.limits.cpu")
}

test_deny_limits_memory_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/resources/limits/memory"), "resources.limits.memory")
}

test_deny_resources_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/resources"), "resources.requests.cpu")
}

# --- the service-account token and the host (S16)

test_deny_token_automount_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/automountServiceAccountToken"), "automountServiceAccountToken")
}

test_deny_token_automount_true if {
	refused(fixtures.sts_set("/spec/template/spec/automountServiceAccountToken", true), "automountServiceAccountToken")
}

test_deny_host_network if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [fixtures.pod_op("add", "hostNetwork", true)])
	refused(planted, "hostNetwork")
}

test_deny_host_pid if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [fixtures.pod_op("add", "hostPID", true)])
	refused(planted, "hostPID")
}

test_deny_host_ipc if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [fixtures.pod_op("add", "hostIPC", true)])
	refused(planted, "hostIPC")
}

test_deny_host_path_volume if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/volumes/-", "value": {"name": "host", "hostPath": {"path": "/"}}}])
	refused(planted, "hostPath")
}

test_deny_host_port if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/ports/0/hostPort", "value": 8443}])
	refused(planted, "hostPort")
}

# --- every container, not only the first

test_deny_init_container_without_hardening if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/initContainers", "value": [{"name": "init", "image": "x"}]}])
	refused(planted, "init")
	refused(planted, "readOnlyRootFilesystem")
}

test_deny_second_container_without_hardening if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/-", "value": {"name": "sidecar", "image": "x"}}])
	refused(planted, "sidecar")
}

# Other kinds of workload are held to the same rules.
test_deny_unhardened_deployment if {
	planted := fixtures.with_doc({
		"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "other"},
		"spec": {"template": {"spec": {"containers": [{"name": "c", "image": "x"}]}}},
	})
	refused(planted, "Deployment/other")
}

test_deny_unhardened_cronjob if {
	planted := fixtures.with_doc({
		"apiVersion": "batch/v1", "kind": "CronJob", "metadata": {"name": "nightly"},
		"spec": {"jobTemplate": {"spec": {"template": {"spec": {"containers": [{"name": "c", "image": "x"}]}}}}},
	})
	refused(planted, "CronJob/nightly")
}

# --- the database's CA and credentials reach the pod as narrowly as possible (S6, S7)

# CloudNativePG's CA Secret also holds ca.key, which signs server certificates (plan 09 A9).
test_db_ca_only_ca_crt if {
	refused(fixtures.sts_remove("/spec/template/spec/volumes/2/secret/items"), "ca.key")
}

test_db_ca_with_another_key if {
	planted := fixtures.sts_set("/spec/template/spec/volumes/2/secret/items", [{"key": "ca.crt", "path": "ca.crt"}, {"key": "ca.key", "path": "ca.key"}])
	refused(planted, "ca.key")
}

test_db_ca_with_a_different_key if {
	planted := fixtures.sts_set("/spec/template/spec/volumes/2/secret/items", [{"key": "ca.key", "path": "ca.crt"}])
	refused(planted, "ca.key")
}

test_deny_db_credentials_with_every_key if {
	refused(fixtures.sts_remove("/spec/template/spec/volumes/1/secret/items"), "only the uri")
}

test_deny_db_credentials_with_the_password_key if {
	planted := fixtures.sts_set("/spec/template/spec/volumes/1/secret/items", [{"key": "uri", "path": "uri"}, {"key": "password", "path": "password"}])
	refused(planted, "only the uri")
}

test_deny_db_credentials_world_readable if {
	refused(fixtures.sts_set("/spec/template/spec/volumes/1/secret/defaultMode", 420), "world-readable")
}

test_deny_db_credentials_mode_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/volumes/1/secret/defaultMode"), "world-readable")
}

# --- what the hub may write (HARDENING.md): the mirror volume and /tmp, nothing else

test_deny_secret_mounted_writable if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/volumeMounts/2/readOnly"), "writable")
}

test_deny_extra_writable_volume if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "add", "path": "/spec/template/spec/volumes/-", "value": {"name": "scratch", "emptyDir": {}}},
		{"op": "add", "path": "/spec/template/spec/containers/0/volumeMounts/-", "value": {"name": "scratch", "mountPath": "/scratch"}},
	])
	refused(planted, "scratch")
}

# --- the hub's ports and probes (HARDENING.md)

test_deny_extra_port if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/ports/-", "value": {"name": "debug", "containerPort": 6060, "protocol": "TCP"}}])
	refused(planted, "ports")
}

test_deny_liveness_on_the_wrong_path if {
	refused(fixtures.sts_set("/spec/template/spec/containers/0/livenessProbe/httpGet/path", "/readyz"), "probes")
}

test_deny_readiness_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/readinessProbe"), "probes")
}

# --- what a reference may be in the environment (crates/hub/HARDENING.md; 09/fix, R2)
#
# The pod contract allows one `valueFrom`: the replica's name, from the downward API, as exactly `fieldRef: {fieldPath: metadata.name}`
# on exactly LK_REPLICA_NAME. Anything else that takes its value from elsewhere is a secret or a setting that is not the pod's.

env_add(entry) := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/env/-", "value": entry}])

test_the_replica_name_from_the_pods_own_name_passes if {
	count(deny) == 0 with input as fixtures.hub
}

test_deny_config_map_key_ref if {
	planted := env_add({"name": "LK_WEB_DIST", "valueFrom": {"configMapKeyRef": {"name": "c", "key": "k"}}})
	refused(planted, "valueFrom")
}

test_deny_resource_field_ref if {
	planted := env_add({"name": "LK_BLOB_CACHE_MIB", "valueFrom": {"resourceFieldRef": {"resource": "limits.memory"}}})
	refused(planted, "valueFrom")
}

test_deny_field_ref_on_another_variable if {
	planted := env_add({"name": "LK_OPS_ADDR", "valueFrom": {"fieldRef": {"fieldPath": "metadata.name"}}})
	refused(planted, "LK_OPS_ADDR")
}

test_deny_field_ref_to_another_field if {
	planted := fixtures.sts_set("/spec/template/spec/containers/0/env/12/valueFrom/fieldRef/fieldPath", "metadata.namespace")
	refused(planted, "valueFrom")
}

# The contract test holds the reference to exactly one key, so a fieldRef that adds an apiVersion is another shape.
test_deny_field_ref_with_more_keys if {
	planted := fixtures.sts_set("/spec/template/spec/containers/0/env/12/valueFrom/fieldRef", {"apiVersion": "v1", "fieldPath": "metadata.name"})
	refused(planted, "valueFrom")
}

test_deny_value_from_beside_another_reference if {
	planted := fixtures.sts_set("/spec/template/spec/containers/0/env/12/valueFrom", {"fieldRef": {"fieldPath": "metadata.name"}, "secretKeyRef": {"name": "s", "key": "k"}})
	refused(planted, "valueFrom")
}

# The Jobs of the hub's image are held to the same (their names start with lanekeeper-hub-).
job_with_env(job_name, entry) := fixtures.with_doc({
	"apiVersion": "batch/v1",
	"kind": "Job",
	"metadata": {"name": job_name, "namespace": "lanekeeper"},
	"spec": {"template": {"spec": {"containers": [{"name": "hub", "image": "x", "env": [entry]}]}}},
})

test_deny_config_map_key_ref_in_the_migration_job if {
	planted := job_with_env("lanekeeper-hub-migrate", {"name": "LK_DB_CA_FILE", "valueFrom": {"configMapKeyRef": {"name": "c", "key": "k"}}})
	refused(planted, "Job/lanekeeper-hub-migrate, container hub: LK_DB_CA_FILE takes its value from a valueFrom")
}

test_deny_field_ref_on_another_variable_in_the_seed_job if {
	planted := job_with_env("lanekeeper-hub-seed-0123456789", {"name": "LK_SEED_TIMEOUT_SECS", "valueFrom": {"fieldRef": {"fieldPath": "metadata.name"}}})
	refused(planted, "Job/lanekeeper-hub-seed-0123456789, container hub: LK_SEED_TIMEOUT_SECS takes its value from a valueFrom")
}

test_deny_resource_field_ref_in_the_seed_job if {
	planted := job_with_env("lanekeeper-hub-seed-0123456789", {"name": "LK_DB_CA_FILE", "valueFrom": {"resourceFieldRef": {"resource": "limits.memory"}}})
	refused(planted, "valueFrom")
}

# A Job that is not the hub's is not this rule's business (another policy holds every Job to the hardening fields).
test_a_job_of_another_name_is_not_a_hub_job if {
	planted := job_with_env("other", {"name": "X", "valueFrom": {"configMapKeyRef": {"name": "c", "key": "k"}}})
	msgs := deny with input as planted
	count([m | some m in msgs; contains(m, "valueFrom")]) == 0
}

# Every replica has its own name: a fixed one would give three replicas one identity and one lease holder.
test_deny_replica_name_fixed if {
	planted := fixtures.sts_set("/spec/template/spec/containers/0/env/12", {"name": "LK_REPLICA_NAME", "value": "lanekeeper-hub-0"})
	refused(planted, "LK_REPLICA_NAME must come from the pod's own name")
}

test_deny_replica_name_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/env/12"), "LK_REPLICA_NAME must come from the pod's own name")
}

# --- the Jobs' variables are the Jobs' alone (HARDENING.md, README item 5; 09/fix, E4)

test_deny_migrator_url_file_in_the_hub if {
	refused(env_add({"name": "LK_MIGRATOR_URL_FILE", "value": "/etc/lanekeeper/migrator/uri"}), "LK_MIGRATOR_URL_FILE")
}

test_deny_migrate_timeout_in_the_hub if {
	refused(env_add({"name": "LK_MIGRATE_TIMEOUT_SECS", "value": "600"}), "LK_MIGRATE_TIMEOUT_SECS")
}

test_deny_seed_timeout_in_the_hub if {
	refused(env_add({"name": "LK_SEED_TIMEOUT_SECS", "value": "120"}), "LK_SEED_TIMEOUT_SECS")
}

# --- one name for the users and for Google (LK_PUBLIC_ORIGIN, LK_HUB_AUDIENCE; README item 9)

test_the_origin_and_the_audience_are_one_name if {
	planted := fixtures.sts_set("/spec/template/spec/containers/0/env/15/value", "https://other.example.com")
	refused(planted, "LK_HUB_AUDIENCE")
}

test_deny_origin_that_is_not_https if {
	planted := fixtures.sts_set("/spec/template/spec/containers/0/env/14/value", "http://lanekeeper.example.com")
	refused(planted, "LK_PUBLIC_ORIGIN")
}

test_deny_origin_with_a_path if {
	planted := fixtures.sts_set("/spec/template/spec/containers/0/env/14/value", "https://lanekeeper.example.com/app")
	refused(planted, "LK_PUBLIC_ORIGIN")
}

# The Gateway serves the name the users type; the hub checks Host and Origin against LK_PUBLIC_ORIGIN.
test_deny_origin_other_than_the_gateways_host if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "replace", "path": "/spec/template/spec/containers/0/env/14/value", "value": "https://elsewhere.example.com"},
		{"op": "replace", "path": "/spec/template/spec/containers/0/env/15/value", "value": "https://elsewhere.example.com"},
	])
	refused(planted, "the Gateway serves")
}

# --- the grace period outlasts the shutdown the hub does (README item 5; 09/fix, T1)

test_the_grace_period_of_the_contract_is_enough_for_the_defaults if {
	count(deny) == 0 with input as fixtures.hub
}

test_deny_grace_period_shorter_than_the_shutdown if {
	refused(fixtures.sts_set("/spec/template/spec/terminationGracePeriodSeconds", 39), "terminationGracePeriodSeconds")
}

test_a_grace_period_that_just_fits_passes if {
	planted := fixtures.sts_set("/spec/template/spec/terminationGracePeriodSeconds", 40)
	count(deny) == 0 with input as planted
}

test_deny_a_longer_drain_than_the_grace_period_allows if {
	planted := env_add({"name": "LK_SHUTDOWN_DRAIN_SECS", "value": "40"})
	refused(planted, "terminationGracePeriodSeconds")
}

test_deny_a_longer_grace_than_the_grace_period_allows if {
	planted := env_add({"name": "LK_SHUTDOWN_GRACE_SECS", "value": "31"})
	refused(planted, "terminationGracePeriodSeconds")
}

test_a_shorter_drain_passes_with_a_shorter_grace_period if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "add", "path": "/spec/template/spec/containers/0/env/-", "value": {"name": "LK_SHUTDOWN_DRAIN_SECS", "value": "10"}},
		{"op": "replace", "path": "/spec/template/spec/terminationGracePeriodSeconds", "value": 25},
	])
	count(deny) == 0 with input as planted
}

# ============================================================================================================================
# The hub image's Jobs (crates/hub/HARDENING.md, "The migration Job" and "The seed Job"; 09/fix, J1 to J6): `hub migrate` as the migrator
# and `hub seed` as the application role, from the hub's image. Each has a planted case, so a rule that stops firing fails here.
# ============================================================================================================================

migrate := "lanekeeper-hub-migrate"

seed := fixtures.seed_job_name

job_c := "/spec/template/spec/containers/0/"

job_pod_path := "/spec/template/spec/"

job_set(name, path, value) := fixtures.patch("Job", name, [{"op": "replace", "path": path, "value": value}])

job_add(name, path, value) := fixtures.patch("Job", name, [{"op": "add", "path": path, "value": value}])

job_remove(name, path) := fixtures.patch("Job", name, [{"op": "remove", "path": path}])

test_jobs_good_pass if {
	count(deny) == 0 with input as fixtures.hub
}

# --- which Jobs there are

test_deny_a_hub_job_that_is_neither if {
	other := json.patch(fixtures.migrate_job, [{"op": "replace", "path": "/metadata/name", "value": "lanekeeper-hub-backfill"}])
	refused(fixtures.with_doc(other), "only two Jobs")
}

test_deny_a_hub_without_its_migration_job if {
	refused(fixtures.without_named("Job", migrate), "no migration Job")
}

# --- the image, the command and the ports

test_deny_job_with_another_image if {
	refused(job_set(migrate, concat("", [job_c, "image"]), "ghcr.io/hsetially/lanekeeper/hub@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"), "the hub's image")
}

test_deny_seed_job_with_another_image if {
	refused(job_set(seed, concat("", [job_c, "image"]), "ghcr.io/other/tool@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), "the hub's image")
}

test_deny_migration_job_that_serves if {
	refused(job_set(migrate, concat("", [job_c, "args"]), ["serve"]), "args must be exactly")
}

test_deny_migration_job_without_args if {
	refused(job_remove(migrate, concat("", [job_c, "args"])), "args must be exactly")
}

test_deny_migration_job_that_seeds if {
	refused(job_set(migrate, concat("", [job_c, "args"]), ["seed", "/etc/lanekeeper/seed/seed.yaml"]), "args must be exactly")
}

test_deny_seed_job_with_a_dry_run if {
	refused(job_set(seed, concat("", [job_c, "args"]), ["seed", "--dry-run", "/etc/lanekeeper/seed/seed.yaml"]), "args must be exactly")
}

test_deny_seed_job_with_another_file if {
	refused(job_set(seed, concat("", [job_c, "args"]), ["seed", "/etc/lanekeeper/seed/other.yaml"]), "args must be exactly")
}

test_deny_job_with_a_command if {
	refused(job_add(migrate, concat("", [job_c, "command"]), ["/bin/sh"]), "command")
}

test_deny_job_with_a_port if {
	refused(job_add(migrate, concat("", [job_c, "ports"]), [{"name": "debug", "containerPort": 6060}]), "no ports")
}

test_deny_job_with_a_second_container if {
	refused(job_add(seed, concat("", [job_pod_path, "containers/-"]), {"name": "sidecar", "image": "x"}), "exactly one container")
}

test_deny_job_with_an_init_container if {
	refused(job_add(seed, concat("", [job_pod_path, "initContainers"]), [{"name": "init", "image": "x"}]), "exactly one container")
}

test_deny_job_without_resources if {
	refused(job_remove(migrate, concat("", [job_c, "resources"])), "resources.requests.cpu")
}

# --- the variables: the migrator's, never the application role's, and no other of the hub's

test_deny_migration_job_with_the_application_credential if {
	planted := job_add(migrate, concat("", [job_c, "env/-"]), {"name": "LK_DATABASE_URL_FILE", "value": "/etc/lanekeeper/db/uri"})
	refused(planted, "LK_DATABASE_URL_FILE")
}

test_deny_seed_job_with_the_migrator_credential if {
	planted := job_add(seed, concat("", [job_c, "env/-"]), {"name": "LK_MIGRATOR_URL_FILE", "value": "/etc/lanekeeper/migrator/uri"})
	refused(planted, "LK_MIGRATOR_URL_FILE")
}

test_deny_migration_job_with_a_variable_of_the_hub if {
	planted := job_add(migrate, concat("", [job_c, "env/-"]), {"name": "LK_GCP_PROJECT", "value": "example-project"})
	refused(planted, "LK_GCP_PROJECT")
}

test_deny_seed_job_with_the_migration_timeout if {
	planted := job_add(seed, concat("", [job_c, "env/-"]), {"name": "LK_MIGRATE_TIMEOUT_SECS", "value": "600"})
	refused(planted, "LK_MIGRATE_TIMEOUT_SECS")
}

test_deny_migration_job_without_the_credential if {
	refused(job_remove(migrate, concat("", [job_c, "env/0"])), "LK_MIGRATOR_URL_FILE is not set")
}

test_deny_migration_job_without_the_database_ca if {
	refused(job_remove(migrate, concat("", [job_c, "env/1"])), "LK_DB_CA_FILE is not set")
}

test_deny_migration_credential_outside_its_mount if {
	refused(job_set(migrate, concat("", [job_c, "env/0/value"]), "/tmp/uri"), "LK_MIGRATOR_URL_FILE must be /etc/lanekeeper/migrator/uri")
}

test_deny_seed_credential_outside_its_mount if {
	refused(job_set(seed, concat("", [job_c, "env/0/value"]), "/tmp/uri"), "LK_DATABASE_URL_FILE must be /etc/lanekeeper/db/uri")
}

test_deny_job_ca_outside_its_mount if {
	refused(job_set(seed, concat("", [job_c, "env/1/value"]), "/tmp/ca.crt"), "LK_DB_CA_FILE must be /etc/lanekeeper/db-ca/ca.crt")
}

test_deny_job_credential_from_a_reference if {
	planted := job_set(migrate, concat("", [job_c, "env/0"]), {"name": "LK_MIGRATOR_URL_FILE", "valueFrom": {"configMapKeyRef": {"name": "c", "key": "k"}}})
	refused(planted, "valueFrom")
}

test_deny_a_migration_timeout_that_is_not_a_number if {
	refused(job_set(migrate, concat("", [job_c, "env/2/value"]), "long"), "LK_MIGRATE_TIMEOUT_SECS")
}

test_deny_a_migration_timeout_over_the_hubs_limit if {
	refused(job_set(migrate, concat("", [job_c, "env/2/value"]), "7201"), "LK_MIGRATE_TIMEOUT_SECS")
}

test_deny_a_seed_timeout_over_the_hubs_limit if {
	refused(job_set(seed, concat("", [job_c, "env/2/value"]), "3601"), "LK_SEED_TIMEOUT_SECS")
}

# --- the volumes: four at most, each read-only but /tmp

test_deny_job_with_an_extra_volume if {
	planted := fixtures.patch("Job", migrate, [
		{"op": "add", "path": "/spec/template/spec/volumes/-", "value": {"name": "scratch", "emptyDir": {}}},
		{"op": "add", "path": "/spec/template/spec/containers/0/volumeMounts/-", "value": {"name": "scratch", "mountPath": "/scratch"}},
	])
	refused(planted, "volumeMounts must be exactly")
}

test_deny_job_with_a_volume_it_does_not_mount if {
	refused(job_add(migrate, concat("", [job_pod_path, "volumes/-"]), {"name": "mirror", "emptyDir": {}}), "volumes must be exactly")
}

test_deny_migration_job_with_the_application_secret_volume if {
	planted := fixtures.patch("Job", migrate, [
		{"op": "replace", "path": "/spec/template/spec/volumes/1/name", "value": "db-credentials"},
		{"op": "replace", "path": "/spec/template/spec/containers/0/volumeMounts/1/name", "value": "db-credentials"},
	])
	refused(planted, "volumeMounts must be exactly")
}

test_deny_job_with_a_host_path if {
	planted := job_add(seed, concat("", [job_pod_path, "volumes/-"]), {"name": "host", "hostPath": {"path": "/"}})
	refused(planted, "hostPath")
}

test_deny_migrator_credential_mounted_writable if {
	refused(job_remove(migrate, concat("", [job_c, "volumeMounts/1/readOnly"])), "volumeMounts must be exactly")
}

test_deny_migrator_credential_mounted_elsewhere if {
	refused(job_set(migrate, concat("", [job_c, "volumeMounts/1/mountPath"]), "/etc/lanekeeper/db"), "volumeMounts must be exactly")
}

test_deny_job_with_the_whole_migrator_secret if {
	refused(job_remove(migrate, concat("", [job_pod_path, "volumes/1/secret/items"])), "only the uri")
}

test_deny_job_with_the_migrators_password_key if {
	planted := job_set(migrate, concat("", [job_pod_path, "volumes/1/secret/items"]), [{"key": "uri", "path": "uri"}, {"key": "password", "path": "password"}])
	refused(planted, "only the uri")
}

test_deny_migrator_credential_world_readable if {
	refused(job_set(migrate, concat("", [job_pod_path, "volumes/1/secret/defaultMode"]), 420), "world-readable")
}

test_deny_seed_credential_world_readable if {
	refused(job_set(seed, concat("", [job_pod_path, "volumes/1/secret/defaultMode"]), 420), "world-readable")
}

test_deny_seed_job_with_the_whole_application_secret if {
	refused(job_remove(seed, concat("", [job_pod_path, "volumes/1/secret/items"])), "only the uri")
}

test_deny_job_with_the_whole_ca_secret if {
	refused(job_remove(migrate, concat("", [job_pod_path, "volumes/2/secret/items"])), "ca.key")
}

# The migrator's credential is never in a pod that is given the application role's, and the seed runs as the role the hub runs as.
test_deny_migration_job_given_the_application_secret if {
	refused(job_set(migrate, concat("", [job_pod_path, "volumes/1/secret/secretName"]), "lanekeeper-db-app"), "the application role's Secret")
}

test_deny_seed_job_given_the_migrators_secret if {
	refused(job_set(seed, concat("", [job_pod_path, "volumes/1/secret/secretName"]), "lanekeeper-db-migrator"), "the Secret the hub mounts")
}

test_deny_seed_file_from_a_secret if {
	planted := job_set(seed, concat("", [job_pod_path, "volumes/3"]), {"name": "seed", "secret": {"secretName": "seed", "items": [{"key": "seed.yaml", "path": "seed.yaml"}]}})
	refused(planted, "the seed file must come from a ConfigMap")
}

test_deny_seed_file_of_another_key if {
	planted := job_set(seed, concat("", [job_pod_path, "volumes/3/configMap/items"]), [{"key": "seed.yaml", "path": "seed.yaml"}, {"key": "other", "path": "other"}])
	refused(planted, "the seed file must come from a ConfigMap")
}

# --- the Job's own life: no restart, a few tries, and a deadline after the process's own timeout

test_deny_job_that_restarts_in_place if {
	refused(job_set(migrate, concat("", [job_pod_path, "restartPolicy"]), "OnFailure"), "restartPolicy")
}

test_deny_job_without_a_restart_policy if {
	refused(job_remove(seed, concat("", [job_pod_path, "restartPolicy"])), "restartPolicy")
}

test_deny_job_without_a_backoff_limit if {
	refused(job_remove(migrate, "/spec/backoffLimit"), "backoffLimit")
}

test_deny_job_that_tries_ten_times if {
	refused(job_set(migrate, "/spec/backoffLimit", 10), "backoffLimit")
}

test_deny_job_without_a_deadline if {
	refused(job_remove(migrate, "/spec/activeDeadlineSeconds"), "activeDeadlineSeconds")
}

test_deny_migration_deadline_not_after_the_timeout if {
	refused(job_set(migrate, "/spec/activeDeadlineSeconds", 600), "activeDeadlineSeconds")
}

test_deny_seed_deadline_not_after_the_timeout if {
	refused(job_set(seed, "/spec/activeDeadlineSeconds", 120), "activeDeadlineSeconds")
}

test_deny_a_deadline_of_a_day if {
	refused(job_set(migrate, "/spec/activeDeadlineSeconds", 86400), "activeDeadlineSeconds")
}

test_a_longer_timeout_with_a_longer_deadline_passes if {
	planted := fixtures.patch("Job", migrate, [
		{"op": "replace", "path": "/spec/template/spec/containers/0/env/2/value", "value": "3600"},
		{"op": "replace", "path": "/spec/activeDeadlineSeconds", "value": 3900},
	])
	count(deny) == 0 with input as planted
}

# --- who the Jobs are: their own account, with no Google identity and no token

test_deny_job_as_the_hubs_account if {
	refused(job_set(migrate, concat("", [job_pod_path, "serviceAccountName"]), "lanekeeper-hub"), "ServiceAccount")
}

test_deny_job_as_the_default_account if {
	refused(job_remove(seed, concat("", [job_pod_path, "serviceAccountName"])), "ServiceAccount")
}

test_deny_job_whose_account_is_not_rendered if {
	refused(fixtures.without_named("ServiceAccount", fixtures.jobs_name), "ServiceAccount")
}

test_deny_job_account_with_a_google_identity if {
	planted := fixtures.patch("ServiceAccount", fixtures.jobs_name, [{"op": "add", "path": "/metadata/annotations/iam.gke.io~1gcp-service-account", "value": "lanekeeper-hub@example-project.iam.gserviceaccount.com"}])
	refused(planted, "Google")
}

test_deny_job_account_that_mounts_a_token if {
	refused(fixtures.patch("ServiceAccount", fixtures.jobs_name, [{"op": "replace", "path": "/automountServiceAccountToken", "value": true}]), "automountServiceAccountToken")
}

test_deny_job_account_without_a_token_statement if {
	refused(fixtures.patch("ServiceAccount", fixtures.jobs_name, [{"op": "remove", "path": "/automountServiceAccountToken"}]), "automountServiceAccountToken")
}

test_deny_job_pod_with_a_google_identity_annotation if {
	refused(job_add(migrate, "/spec/template/metadata/annotations", {"iam.gke.io/gcp-service-account": "x"}), "Google")
}

test_deny_job_pod_with_the_hubs_label if {
	planted := fixtures.patch("Job", migrate, [{"op": "replace", "path": "/spec/template/metadata/labels/app.kubernetes.io~1name", "value": "lanekeeper-hub"}])
	refused(planted, "label")
}

test_deny_job_pod_the_jobs_policy_does_not_select if {
	planted := fixtures.patch("Job", seed, [{"op": "replace", "path": "/spec/template/metadata/labels/app.kubernetes.io~1name", "value": "lanekeeper-hub-seed"}])
	refused(planted, "label")
}

# --- the order: the migration first (a hook), the seed after it (not a hook), and what the migration needs before it (hooks)

test_deny_migration_job_that_is_not_a_hook if {
	refused(fixtures.patch("Job", migrate, [{"op": "remove", "path": "/metadata/annotations"}]), "pre-install,pre-upgrade")
}

test_deny_migration_job_that_runs_after_install if {
	refused(job_set(migrate, "/metadata/annotations/helm.sh~1hook", "post-install,post-upgrade"), "pre-install,pre-upgrade")
}

test_deny_seed_job_that_is_a_hook if {
	refused(job_add(seed, "/metadata/annotations", {"helm.sh/hook": "pre-install,pre-upgrade"}), "not a hook")
}

test_deny_seed_job_that_is_an_argo_hook if {
	refused(job_add(seed, "/metadata/annotations", {"argocd.argoproj.io/hook": "PreSync"}), "not a hook")
}

test_deny_job_account_that_is_not_a_hook if {
	refused(fixtures.patch("ServiceAccount", fixtures.jobs_name, [{"op": "remove", "path": "/metadata/annotations"}]), "hook")
}

test_deny_job_account_that_runs_after_the_migration if {
	refused(fixtures.patch("ServiceAccount", fixtures.jobs_name, [{"op": "replace", "path": "/metadata/annotations/helm.sh~1hook-weight", "value": "5"}]), "weight")
}

test_deny_job_policy_that_runs_with_the_migration if {
	refused(fixtures.patch("NetworkPolicy", fixtures.jobs_name, [{"op": "replace", "path": "/metadata/annotations/helm.sh~1hook-weight", "value": "0"}]), "weight")
}

test_deny_job_policy_that_is_not_a_hook if {
	refused(fixtures.patch("NetworkPolicy", fixtures.jobs_name, [{"op": "remove", "path": "/metadata/annotations"}]), "hook")
}

test_deny_job_account_that_is_deleted_when_the_migration_succeeds if {
	refused(fixtures.patch("ServiceAccount", fixtures.jobs_name, [{"op": "replace", "path": "/metadata/annotations/helm.sh~1hook-delete-policy", "value": "hook-succeeded"}]), "hook-succeeded")
}

# ============================================================================================================================
# The agent (09/T4; S16, S17): the same fields as the hub, one planted case per field, and the agent's own contract
# (crates/agent/HARDENING.md): it calls the Kubernetes API, so its token is mounted; one replica; the writable mounts are the NFS
# export, /tmp and the spool; the paths it is given are the paths of its mounts.
# ============================================================================================================================

agent_pod := "/spec/template/spec/"

agent_c := "/spec/template/spec/containers/0/"

test_agent_good_passes if {
	count(deny) == 0 with input as fixtures.agent
}

# --- the pod's security context (S16)

test_agent_deny_run_as_non_root_missing if {
	refused(fixtures.agent_remove(concat("", [agent_pod, "securityContext/runAsNonRoot"])), "runAsNonRoot")
}

test_agent_deny_run_as_non_root_false if {
	refused(fixtures.agent_set(concat("", [agent_pod, "securityContext/runAsNonRoot"]), false), "runAsNonRoot")
}

test_agent_deny_run_as_root_user if {
	refused(fixtures.agent_set(concat("", [agent_pod, "securityContext/runAsUser"]), 0), "runAsUser")
}

test_agent_deny_run_as_user_missing if {
	refused(fixtures.agent_remove(concat("", [agent_pod, "securityContext/runAsUser"])), "runAsUser")
}

test_agent_deny_run_as_root_group if {
	refused(fixtures.agent_set(concat("", [agent_pod, "securityContext/runAsGroup"]), 0), "runAsGroup")
}

test_agent_deny_run_as_group_missing if {
	refused(fixtures.agent_remove(concat("", [agent_pod, "securityContext/runAsGroup"])), "runAsGroup")
}

test_agent_deny_seccomp_missing if {
	refused(fixtures.agent_remove(concat("", [agent_pod, "securityContext/seccompProfile"])), "seccomp")
}

test_agent_deny_seccomp_unconfined if {
	refused(fixtures.agent_set(concat("", [agent_pod, "securityContext/seccompProfile/type"]), "Unconfined"), "seccomp")
}

# --- the container's security context (S16)

test_agent_deny_privilege_escalation_missing if {
	refused(fixtures.agent_remove(concat("", [agent_c, "securityContext/allowPrivilegeEscalation"])), "allowPrivilegeEscalation")
}

test_agent_deny_privilege_escalation_true if {
	refused(fixtures.agent_set(concat("", [agent_c, "securityContext/allowPrivilegeEscalation"]), true), "allowPrivilegeEscalation")
}

test_agent_deny_privileged if {
	refused(fixtures.agent_set(concat("", [agent_c, "securityContext/privileged"]), true), "privileged")
}

test_agent_deny_writable_root_missing if {
	refused(fixtures.agent_remove(concat("", [agent_c, "securityContext/readOnlyRootFilesystem"])), "readOnlyRootFilesystem")
}

test_agent_deny_writable_root_false if {
	refused(fixtures.agent_set(concat("", [agent_c, "securityContext/readOnlyRootFilesystem"]), false), "readOnlyRootFilesystem")
}

test_agent_deny_drop_all_missing if {
	refused(fixtures.agent_remove(concat("", [agent_c, "securityContext/capabilities"])), "drop ALL")
}

test_agent_deny_drop_something_else if {
	refused(fixtures.agent_set(concat("", [agent_c, "securityContext/capabilities/drop"]), ["NET_RAW"]), "drop ALL")
}

test_agent_deny_added_capability if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": concat("", [agent_c, "securityContext/capabilities/add"]), "value": ["NET_ADMIN"]}])
	refused(planted, "adds capabilities")
}

# --- resources (S16)

test_agent_deny_requests_cpu_missing if {
	refused(fixtures.agent_remove(concat("", [agent_c, "resources/requests/cpu"])), "resources.requests.cpu")
}

test_agent_deny_requests_memory_missing if {
	refused(fixtures.agent_remove(concat("", [agent_c, "resources/requests/memory"])), "resources.requests.memory")
}

test_agent_deny_limits_cpu_missing if {
	refused(fixtures.agent_remove(concat("", [agent_c, "resources/limits/cpu"])), "resources.limits.cpu")
}

test_agent_deny_limits_memory_missing if {
	refused(fixtures.agent_remove(concat("", [agent_c, "resources/limits/memory"])), "resources.limits.memory")
}

# --- the token and the host

# The agent is a Kubernetes client (restarts, watches, its certificate Secret), so its token is mounted: that is its contract, and
# a pod that cannot call the API cannot do its job. Every other pod mounts none.
test_agent_deny_token_automount_missing if {
	refused(fixtures.agent_remove(concat("", [agent_pod, "automountServiceAccountToken"])), "must be stated and true")
}

test_agent_deny_token_automount_false if {
	refused(fixtures.agent_set(concat("", [agent_pod, "automountServiceAccountToken"]), false), "must be stated and true")
}

test_deny_token_automount_true_on_another_deployment if {
	planted := fixtures.agent_with_doc(json.patch(fixtures.agent_deployment, [
		{"op": "replace", "path": "/metadata/name", "value": "lanekeeper-other"},
	]))
	refused(planted, "must be stated and false")
}

test_agent_deny_host_network if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [fixtures.agent_pod_op("add", "hostNetwork", true)]), "hostNetwork")
}

test_agent_deny_host_pid if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [fixtures.agent_pod_op("add", "hostPID", true)]), "hostPID")
}

test_agent_deny_host_ipc if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [fixtures.agent_pod_op("add", "hostIPC", true)]), "hostIPC")
}

test_agent_deny_host_path_volume if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": concat("", [agent_pod, "volumes/-"]), "value": {"name": "host", "hostPath": {"path": "/"}}}])
	refused(planted, "hostPath")
}

test_agent_deny_host_port if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": concat("", [agent_c, "ports/0/hostPort"]), "value": 9090}]), "hostPort")
}

# --- the agent's contract (crates/agent/HARDENING.md)

test_agent_deny_two_replicas if {
	refused(fixtures.agent_set("/spec/replicas", 2), "1 replica")
}

test_agent_deny_no_replica_count if {
	refused(fixtures.agent_remove("/spec/replicas"), "1 replica")
}

test_agent_deny_another_service_account if {
	refused(fixtures.agent_set(concat("", [agent_pod, "serviceAccountName"]), "default"), "serviceAccountName")
}

test_agent_deny_the_default_service_account if {
	refused(fixtures.agent_remove(concat("", [agent_pod, "serviceAccountName"])), "serviceAccountName")
}

test_agent_deny_extra_port if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": concat("", [agent_c, "ports/-"]), "value": {"name": "debug", "containerPort": 6060, "protocol": "TCP"}}])
	refused(planted, "ports")
}

test_agent_deny_ops_port_moved if {
	refused(fixtures.agent_set(concat("", [agent_c, "ports/0/containerPort"]), 8080), "ports")
}

test_agent_deny_liveness_on_the_wrong_path if {
	refused(fixtures.agent_set(concat("", [agent_c, "livenessProbe/httpGet/path"]), "/readyz"), "probes")
}

test_agent_deny_readiness_missing if {
	refused(fixtures.agent_remove(concat("", [agent_c, "readinessProbe"])), "probes")
}

test_agent_deny_extra_volume if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": concat("", [agent_pod, "volumes/-"]), "value": {"name": "scratch", "emptyDir": {}}}])
	refused(planted, "scratch")
}

test_agent_deny_secret_volume if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": concat("", [agent_pod, "volumes/-"]), "value": {"name": "keys", "secret": {"secretName": "something"}}}])
	refused(planted, "keys")
}

test_agent_deny_the_export_as_something_else if {
	refused(fixtures.agent_set(concat("", [agent_pod, "volumes/0"]), {"name": "nfs", "emptyDir": {}}), "an NFS volume")
}

test_agent_deny_the_hub_ca_as_a_secret if {
	refused(fixtures.agent_set(concat("", [agent_pod, "volumes/3"]), {"name": "hub-ca", "secret": {"secretName": "lanekeeper-hub-ca"}}), "ConfigMap")
}

test_agent_deny_hub_ca_mounted_writable if {
	refused(fixtures.agent_remove(concat("", [agent_c, "volumeMounts/3/readOnly"])), "writable")
}

test_agent_deny_extra_writable_mount if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [
		{"op": "add", "path": concat("", [agent_pod, "volumes/-"]), "value": {"name": "scratch", "emptyDir": {}}},
		{"op": "add", "path": concat("", [agent_c, "volumeMounts/-"]), "value": {"name": "scratch", "mountPath": "/scratch"}},
	])
	refused(planted, "writable")
}

# The paths the agent is given are the paths of its mounts (the agent test does the same for the contract).
test_agent_deny_nfs_root_that_is_not_the_mount if {
	refused(fixtures.agent_set(concat("", [agent_c, "env/8/value"]), "/mnt/elsewhere"), "LK_NFS_ROOT")
}

test_agent_deny_tmp_dir_that_is_not_the_mount if {
	refused(fixtures.agent_set(concat("", [agent_c, "env/14/value"]), "/var/tmp"), "LK_TMP_DIR")
}

test_agent_deny_hub_ca_file_outside_the_mount if {
	refused(fixtures.agent_set(concat("", [agent_c, "env/2/value"]), "/etc/ssl/ca.pem"), "LK_HUB_CA_FILE")
}

test_agent_deny_nfs_root_missing if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "remove", "path": concat("", [agent_c, "env/8"])}]), "LK_NFS_ROOT")
}

# --- the spool (09/T8; D74): a volume of its own on a claim, writable, outside the export

test_agent_deny_spool_as_an_empty_dir if {
	refused(fixtures.agent_set(concat("", [agent_pod, "volumes/2"]), {"name": "spool", "emptyDir": {}}), "must be the PersistentVolumeClaim")
}

test_agent_deny_spool_claim_of_another_name if {
	refused(fixtures.agent_set(concat("", [agent_pod, "volumes/2"]), {"name": "spool", "persistentVolumeClaim": {"claimName": "lanekeeper-db-1"}}), "must be the PersistentVolumeClaim")
}

test_agent_deny_spool_without_a_volume if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "remove", "path": concat("", [agent_pod, "volumes/2"])}]), "no volume spool")
}

test_agent_deny_spool_mounted_read_only if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": concat("", [agent_c, "volumeMounts/2/readOnly"]), "value": true}]), "spool is mounted read-only")
}

test_agent_deny_spool_dir_that_is_not_the_mount if {
	refused(fixtures.agent_set(concat("", [agent_c, "env/12/value"]), "/var/spool/elsewhere"), "LK_SPOOL_DIR must be the path of the spool mount")
}

test_agent_deny_spool_dir_missing if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "remove", "path": concat("", [agent_c, "env/12"])}]), "LK_SPOOL_DIR must be the path of the spool mount")
}

test_agent_deny_spool_mount_missing if {
	refused(fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "remove", "path": concat("", [agent_c, "volumeMounts/2"])}]), "LK_SPOOL_DIR must be the path of the spool mount")
}

# A spool inside the export would be tracked as configuration and sent again, for ever; the agent refuses it at start, and so does this.
test_agent_deny_spool_inside_the_export if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [
		{"op": "replace", "path": concat("", [agent_c, "env/12/value"]), "value": "/mnt/csp-configuration/spool"},
		{"op": "replace", "path": concat("", [agent_c, "volumeMounts/2/mountPath"]), "value": "/mnt/csp-configuration/spool"},
	])
	refused(planted, "is inside the NFS root")
}

# "Inside" is a path prefix by component: a directory whose name merely starts with the export's is not inside it.
test_agent_spool_next_to_the_export_is_not_inside_it if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [
		{"op": "replace", "path": concat("", [agent_c, "env/12/value"]), "value": "/mnt/csp-configuration-spool"},
		{"op": "replace", "path": concat("", [agent_c, "volumeMounts/2/mountPath"]), "value": "/mnt/csp-configuration-spool"},
	])
	count(deny) == 0 with input as planted
}

# ============================================================================================================================
# 09/fix, round 1 of the review (NB-1): `envFrom` is a second way to put names into the environment, and the rules above read only `env`.
# A ConfigMap or Secret named there adds every key it holds as a variable, so none of the contract's rules about names, values and
# references reaches them. The hub's pod and its Jobs carry none.
# ============================================================================================================================

env_from_config_map := [{"configMapRef": {"name": "evil"}}]

env_from_secret := [{"secretRef": {"name": "evil"}}]

env_from_prefixed := [{"prefix": "LK_", "configMapRef": {"name": "evil"}}]

test_the_hub_and_its_jobs_without_env_from_pass if {
	count(deny) == 0 with input as fixtures.hub
}

test_deny_env_from_a_config_map_in_the_hub if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/envFrom", "value": env_from_config_map}])
	refused(planted, "StatefulSet/lanekeeper-hub, container hub: envFrom")
}

test_deny_env_from_a_secret_in_the_hub if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/envFrom", "value": env_from_secret}])
	refused(planted, "StatefulSet/lanekeeper-hub, container hub: envFrom")
}

test_deny_env_from_with_a_prefix_in_the_hub if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/envFrom", "value": env_from_prefixed}])
	refused(planted, "envFrom")
}

test_deny_env_from_in_the_migration_job if {
	refused(job_add(migrate, concat("", [job_c, "envFrom"]), env_from_config_map), "Job/lanekeeper-hub-migrate, container migrate: envFrom")
}

test_deny_env_from_in_the_seed_job if {
	refused(job_add(seed, concat("", [job_c, "envFrom"]), env_from_config_map), "container seed: envFrom")
}

# An empty list adds nothing; it is not this rule's business.
test_an_empty_env_from_adds_nothing if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/envFrom", "value": []}])
	msgs := deny with input as planted
	count([m | some m in msgs; contains(m, "envFrom")]) == 0
}

# A workload that is not the hub's is not this rule's business.
test_env_from_in_another_workload_is_not_the_hubs_business if {
	planted := fixtures.with_doc({
		"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": "other", "namespace": "lanekeeper"},
		"spec": {"template": {"spec": {"containers": [{"name": "c", "image": "x", "envFrom": env_from_config_map}]}}},
	})
	msgs := deny with input as planted
	count([m | some m in msgs; contains(m, "envFrom")]) == 0
}

# ============================================================================================================================
# 09/fix, round 1 of the review (NB-2): the migrator is the database's owner, and its Secret is mounted by the migration Job and by no
# other workload. The Job rules key on the hub's name prefix, so a Job (or any workload) of another name that mounts the Secret is not held
# to them; this rule keys on the Secret.
# ============================================================================================================================

migrator_secret_volume(secret_name) := {"name": "creds", "secret": {"secretName": secret_name}}

with_workload(doc) := fixtures.with_doc(doc)

other_job(volume) := {
	"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": "other-job", "namespace": "lanekeeper"},
	"spec": {"template": {"spec": {"containers": [{"name": "c", "image": "x"}], "volumes": [volume]}}},
}

test_the_migration_job_alone_mounts_the_migrator_secret if {
	msgs := deny with input as fixtures.hub
	count([m | some m in msgs; contains(m, "migrator")]) == 0
}

test_deny_a_job_of_another_name_that_mounts_the_migrator_secret if {
	planted := with_workload(other_job(migrator_secret_volume("lanekeeper-db-migrator")))
	refused(planted, "Job/other-job: volume creds mounts the Secret lanekeeper-db-migrator")
}

test_deny_a_deployment_that_mounts_the_migrator_secret if {
	planted := with_workload({
		"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "tool", "namespace": "lanekeeper"},
		"spec": {"template": {"spec": {"containers": [{"name": "c", "image": "x"}], "volumes": [migrator_secret_volume("lanekeeper-db-migrator")]}}},
	})
	refused(planted, "Deployment/tool: volume creds mounts the Secret lanekeeper-db-migrator")
}

test_deny_the_seed_job_that_mounts_the_migrator_secret_as_an_extra_volume if {
	refused(job_add(seed, concat("", [job_pod_path, "volumes/-"]), migrator_secret_volume("lanekeeper-db-migrator")), "Job/lanekeeper-hub-seed-0123456789: volume creds mounts the Secret lanekeeper-db-migrator")
}

test_deny_the_hub_that_mounts_the_migrator_secret_as_an_extra_volume if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/volumes/-", "value": migrator_secret_volume("lanekeeper-db-migrator")}])
	refused(planted, "StatefulSet/lanekeeper-hub: volume creds mounts the Secret lanekeeper-db-migrator")
}

# A projected volume can carry a Secret as one of its sources.
test_deny_a_projected_volume_that_carries_the_migrator_secret if {
	planted := with_workload(other_job({"name": "p", "projected": {"sources": [{"secret": {"name": "lanekeeper-db-migrator"}}]}}))
	refused(planted, "Job/other-job: volume p mounts the Secret lanekeeper-db-migrator")
}

# The name is whatever the migration Job mounts, and whatever database.migratorSecretName says.
test_deny_the_migrator_secret_under_the_name_the_migration_job_mounts if {
	renamed := fixtures.patch("Job", migrate, [{"op": "replace", "path": "/spec/template/spec/volumes/1/secret/secretName", "value": "renamed-migrator"}])
	combined := [{"path": "rendered/hub.yaml", "contents": array.concat(renamed[0].contents, [other_job(migrator_secret_volume("renamed-migrator"))])}, renamed[1]]
	refused(combined, "Job/other-job: volume creds mounts the Secret renamed-migrator")
}

test_deny_the_migrator_secret_under_the_name_a_values_file_gives_it if {
	values := {"path": "deploy/values/hub/prod.yaml", "contents": {"database": {"migratorSecretName": "tenant-owner"}}}
	planted := array.concat(with_workload(other_job(migrator_secret_volume("tenant-owner"))), [values])
	refused(planted, "Job/other-job: volume creds mounts the Secret tenant-owner")
}

test_another_secret_is_not_the_migrators if {
	planted := with_workload(other_job(migrator_secret_volume("some-other-secret")))
	msgs := deny with input as planted
	count([m | some m in msgs; contains(m, "migrator")]) == 0
}

# ============================================================================================================================
# 09/fix, round 1 of the review (NB-9): three settings are kept off by the closed values schema alone. Each turns on something the design has
# not confirmed, so a StatefulSet that carries one is refused in every environment, whatever put it there.
# ============================================================================================================================

test_deny_mcp_notify_services_in_the_hub if {
	refused(env_add({"name": "LK_MCP_NOTIFY_SERVICES", "value": "csp-configuration-server"}), "LK_MCP_NOTIFY_SERVICES")
}

test_deny_retention_enabled_in_the_hub if {
	refused(env_add({"name": "LK_RETENTION_ENABLED", "value": "true"}), "LK_RETENTION_ENABLED")
}

test_deny_retention_dry_run_in_the_hub if {
	refused(env_add({"name": "LK_RETENTION_DRY_RUN", "value": "true"}), "LK_RETENTION_DRY_RUN")
}

# Even a value that looks like off: the setting is not the pod's to carry.
test_deny_retention_enabled_set_to_false_in_the_hub if {
	refused(env_add({"name": "LK_RETENTION_ENABLED", "value": "false"}), "LK_RETENTION_ENABLED")
}

# LK_MCP_AUTH is a legitimate setting (oauth or personal_token), and is not pinned.
test_mcp_auth_is_a_setting_the_hub_may_carry if {
	planted := env_add({"name": "LK_MCP_AUTH", "value": "personal_token"})
	msgs := deny with input as planted
	count([m | some m in msgs; contains(m, "LK_MCP_AUTH")]) == 0
}

test_deny_a_withheld_setting_in_the_migration_job if {
	planted := job_add(migrate, concat("", [job_c, "env/-"]), {"name": "LK_RETENTION_ENABLED", "value": "true"})
	refused(planted, "LK_RETENTION_ENABLED")
}

# ============================================================================================================================
# 09/fix, round 1 of the review (NB-7): a seed Job is the record of which file was applied (HARDENING.md, "Why the file is a reviewed
# change"), and a new image, file or revision is a new Job with a new name. The hub's Application prunes, so Argo CD would delete the old Job
# (and the ConfigMap with its file) on every change, and the record with it. Both carry Prune=false, so old seed Jobs accumulate by design.
# ============================================================================================================================

sync_options_path := "/metadata/annotations/argocd.argoproj.io~1sync-options"

test_deny_a_seed_job_that_a_sync_may_prune if {
	refused(job_remove(seed, sync_options_path), "Job/lanekeeper-hub-seed-0123456789: the annotation argocd.argoproj.io/sync-options: Prune=false is missing")
}

test_deny_a_seed_job_that_is_set_to_be_pruned if {
	refused(job_set(seed, sync_options_path, "Prune=true"), "Prune=false is missing")
}

test_deny_a_seed_job_with_the_option_among_others_that_replace if {
	refused(job_set(seed, sync_options_path, "Prune=false,Replace=true"), "Prune=false is missing")
}

test_deny_the_seed_file_that_a_sync_may_prune if {
	refused(fixtures.patch("ConfigMap", seed, [{"op": "remove", "path": sync_options_path}]), "ConfigMap/lanekeeper-hub-seed-0123456789: the annotation argocd.argoproj.io/sync-options: Prune=false is missing")
}

# The migration Job is a hook that is replaced at each sync; the record rule is for the seed Job and its file alone.
test_the_migration_job_is_not_asked_to_keep_old_copies if {
	msgs := deny with input as fixtures.hub
	count([m | some m in msgs; contains(m, "Prune=false")]) == 0
}
