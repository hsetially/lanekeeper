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
