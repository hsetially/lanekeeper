# Tests for database.rego (09/T5; S6, S7, S10, S16, S18, D29, D40, D62, D124): the rendered CloudNativePG cluster passes, and each planted
# violation is refused by its own rule. Run with `conftest verify --policy deploy/policy`.
package database

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

set(path, value) := fixtures.db_cluster_set(path, value)

add(path, value) := fixtures.db_cluster_add(path, value)

remove(path) := fixtures.db_cluster_remove(path)

test_good_passes if {
	count(deny) == 0 with input as fixtures.db
	count(warn) == 0 with input as fixtures.db
}

test_good_kind_passes_with_warnings if {
	count(deny) == 0 with input as fixtures.db_kind
	count(warn) > 0 with input as fixtures.db_kind
}

test_no_cluster_is_fine_for_other_charts if {
	count(deny) == 0 with input as fixtures.hub
	count(deny) == 0 with input as fixtures.gs
}

# --- which cluster

test_deny_another_cluster if {
	other := json.patch(fixtures.db_cluster, [{"op": "replace", "path": "/metadata/name", "value": "other-db"}])
	refused(fixtures.db_with_doc(other), "the only database is lanekeeper-db")
}

# --- three instances (D62)

test_deny_two_instances if {
	refused(set("/spec/instances", 2), "3 instances")
}

test_deny_one_instance if {
	refused(set("/spec/instances", 1), "3 instances")
}

test_deny_instances_missing if {
	refused(remove("/spec/instances"), "3 instances")
}

test_deny_pdb_off if {
	refused(set("/spec/enablePDB", false), "enablePDB")
}

# --- the image (S19)

test_deny_image_by_tag if {
	refused(set("/spec/imageName", "ghcr.io/cloudnative-pg/postgresql:16.15-202610050823-system-trixie"), "not pinned by digest")
}

test_deny_image_missing if {
	refused(remove("/spec/imageName"), "not pinned by digest")
}

test_deny_image_with_a_short_digest if {
	refused(set("/spec/imageName", "ghcr.io/cloudnative-pg/postgresql:16@sha256:abc"), "not pinned by digest")
}

test_deny_image_placeholder_digest if {
	planted := set("/spec/imageName", "ghcr.io/cloudnative-pg/postgresql:16@sha256:0000000000000000000000000000000000000000000000000000000000000000")
	refused(planted, "placeholder")
}

test_deny_image_of_another_project if {
	planted := set("/spec/imageName", concat("", ["docker.io/library/postgres:16@", fixtures.db_digest]))
	refused(planted, "ghcr.io/cloudnative-pg/postgresql")
}

# --- TLS (S6)

test_deny_tls12_min if {
	refused(set("/spec/postgresql/parameters/ssl_min_protocol_version", "TLSv1.2"), "ssl_min_protocol_version")
}

test_deny_tls_min_missing if {
	refused(remove("/spec/postgresql/parameters/ssl_min_protocol_version"), "ssl_min_protocol_version")
}

test_deny_tls_max_below_13 if {
	refused(set("/spec/postgresql/parameters/ssl_max_protocol_version", "TLSv1.2"), "ssl_max_protocol_version")
}

test_deny_hostnossl_allowed if {
	refused(set("/spec/postgresql/pg_hba", ["hostnossl all all all scram-sha-256"]), "first rule")
}

test_deny_pg_hba_missing if {
	refused(remove("/spec/postgresql/pg_hba"), "first rule")
}

test_deny_pg_hba_reject_not_first if {
	planted := set("/spec/postgresql/pg_hba", ["hostssl all all all scram-sha-256", "hostnossl all all all reject"])
	refused(planted, "first rule")
}

test_deny_pg_hba_with_trust if {
	planted := set("/spec/postgresql/pg_hba", ["hostnossl all all all reject", "hostssl all all 192.0.2.0/24 trust"])
	refused(planted, "trust")
}

test_deny_pg_hba_with_md5 if {
	planted := set("/spec/postgresql/pg_hba", ["hostnossl all all all reject", "hostssl lanekeeper lanekeeper_app all md5"])
	refused(planted, "md5")
}

test_deny_pg_hba_with_a_password_in_clear if {
	planted := set("/spec/postgresql/pg_hba", ["hostnossl all all all reject", "hostssl lanekeeper lanekeeper_app all password"])
	refused(planted, "password")
}

test_deny_pg_hba_that_lets_a_later_rule_skip_tls if {
	planted := set("/spec/postgresql/pg_hba", ["hostnossl all all all reject", "hostnossl all all 192.0.2.0/24 scram-sha-256"])
	refused(planted, "hostnossl")
}

test_pg_hba_with_a_tls_rule_after_the_reject_passes if {
	planted := set("/spec/postgresql/pg_hba", ["hostnossl all all all reject", "hostssl lanekeeper lanekeeper_app all scram-sha-256"])
	count(deny) == 0 with input as planted
}

test_deny_weak_password_encryption if {
	refused(set("/spec/postgresql/parameters/password_encryption", "md5"), "password_encryption")
}

test_deny_fsync_off if {
	refused(add("/spec/postgresql/parameters/fsync", "off"), "postgresql.parameters.fsync")
}

test_deny_synchronous_commit_off if {
	refused(add("/spec/postgresql/parameters/synchronous_commit", "off"), "synchronous_commit")
}

test_deny_planner_costs if {
	refused(add("/spec/postgresql/parameters/random_page_cost", "1.1"), "random_page_cost")
}

test_deny_superuser_access if {
	refused(set("/spec/enableSuperuserAccess", true), "enableSuperuserAccess")
}

test_deny_superuser_access_missing if {
	refused(remove("/spec/enableSuperuserAccess"), "enableSuperuserAccess")
}

# --- the database and its roles (D124, Q6)

test_deny_owner_other_than_the_migrator if {
	refused(set("/spec/bootstrap/initdb/owner", "lanekeeper_app"), "lanekeeper_migrator")
}

test_deny_database_other_than_lanekeeper if {
	refused(set("/spec/bootstrap/initdb/database", "app"), "database")
}

test_deny_collation_c if {
	refused(set("/spec/bootstrap/initdb/localeCollate", "C"), "en_US.utf8")
}

test_deny_ctype_c if {
	refused(set("/spec/bootstrap/initdb/localeCType", "C"), "en_US.utf8")
}

test_deny_encoding_latin1 if {
	refused(set("/spec/bootstrap/initdb/encoding", "LATIN1"), "UTF8")
}

test_deny_no_pgvector if {
	refused(set("/spec/bootstrap/initdb/postInitApplicationSQL", ["CREATE EXTENSION IF NOT EXISTS pg_trgm"]), "vector")
}

test_deny_no_pg_trgm if {
	refused(set("/spec/bootstrap/initdb/postInitApplicationSQL", ["CREATE EXTENSION IF NOT EXISTS vector"]), "pg_trgm")
}

test_deny_post_init_sql_that_grants_to_the_app if {
	planted := add("/spec/bootstrap/initdb/postInitApplicationSQL/-", "GRANT ALL ON ALL TABLES IN SCHEMA public TO lanekeeper_app")
	refused(planted, "postInitApplicationSQL")
}

test_deny_initdb_secret_missing if {
	refused(remove("/spec/bootstrap/initdb/secret"), "initdb.secret.name is empty")
}

# CloudNativePG would make the owner's credentials in `<cluster>-app` if it were left to name them, which is the name the hub reads
# its own credentials from: the migrator's Secret has a name of its own, and it is not the app role's.
test_deny_migrator_and_app_share_a_secret if {
	refused(set("/spec/bootstrap/initdb/secret/name", "lanekeeper-db-app"), "same Secret")
}

# Even when the app role's Secret has another name, the owner's must not take the one that the operator and the hub use.
test_deny_the_default_owner_secret_name if {
	planted := fixtures.db_patch("Cluster", "lanekeeper-db", [
		{"op": "replace", "path": "/spec/bootstrap/initdb/secret/name", "value": "lanekeeper-db-app"},
		{"op": "replace", "path": "/spec/managed/roles/0/passwordSecret/name", "value": "lanekeeper-db-hub"},
	])
	refused(planted, "the name the operator and the hub use")
}

test_deny_app_role_missing if {
	refused(remove("/spec/managed"), "lanekeeper_app")
}

test_deny_app_role_superuser if {
	refused(set("/spec/managed/roles/0/superuser", true), "superuser")
}

test_deny_app_role_superuser_missing if {
	refused(remove("/spec/managed/roles/0/superuser"), "superuser")
}

test_deny_app_role_createdb if {
	refused(set("/spec/managed/roles/0/createdb", true), "createdb")
}

test_deny_app_role_createrole if {
	refused(set("/spec/managed/roles/0/createrole", true), "createrole")
}

test_deny_app_role_replication if {
	refused(set("/spec/managed/roles/0/replication", true), "replication")
}

test_deny_app_role_bypassrls if {
	refused(set("/spec/managed/roles/0/bypassrls", true), "bypassrls")
}

test_deny_app_role_cannot_log_in if {
	refused(set("/spec/managed/roles/0/login", false), "login")
}

test_deny_app_role_without_a_password_secret if {
	refused(remove("/spec/managed/roles/0/passwordSecret"), "passwordSecret")
}

test_deny_app_role_without_a_password if {
	refused(add("/spec/managed/roles/0/disablePassword", true), "disablePassword")
}

test_deny_app_role_in_a_group_role if {
	refused(add("/spec/managed/roles/0/inRoles", ["pg_write_all_data"]), "inRoles")
}

test_deny_app_role_absent if {
	refused(set("/spec/managed/roles/0/ensure", "absent"), "ensure")
}

test_deny_another_managed_role if {
	planted := add("/spec/managed/roles/-", {"name": "analyst", "ensure": "present", "login": true, "passwordSecret": {"name": "analyst"}})
	refused(planted, "another managed role")
}

# --- backups (D29, D40, S10)

test_deny_no_backup_in_prod if {
	refused(remove("/spec/backup"), "no backup")
}

test_deny_no_backup_in_staging if {
	planted := fixtures.db_patch("Cluster", "lanekeeper-db", [
		{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1environment", "value": "staging"},
		{"op": "remove", "path": "/spec/backup"},
	])
	refused(planted, "no backup")
}

test_deny_backup_to_a_bucket_that_is_not_gcs if {
	refused(set("/spec/backup/barmanObjectStore/destinationPath", "s3://bucket/lanekeeper-db"), "gs://")
}

test_deny_backup_to_the_root_of_a_bucket if {
	refused(set("/spec/backup/barmanObjectStore/destinationPath", "gs://example-project-lanekeeper-backup-prod"), "path")
}

test_deny_backup_with_a_key_file if {
	planted := set("/spec/backup/barmanObjectStore/googleCredentials", {"applicationCredentials": {"name": "gcs-key", "key": "key.json"}})
	refused(planted, "applicationCredentials")
}

test_deny_backup_without_workload_identity if {
	refused(set("/spec/backup/barmanObjectStore/googleCredentials/gkeEnvironment", false), "gkeEnvironment")
}

test_deny_backup_without_credentials if {
	refused(remove("/spec/backup/barmanObjectStore/googleCredentials"), "gkeEnvironment")
}

test_deny_backup_retention_of_a_week if {
	refused(set("/spec/backup/retentionPolicy", "7d"), "30d")
}

test_deny_backup_retention_missing if {
	refused(remove("/spec/backup/retentionPolicy"), "30d")
}

test_deny_backup_without_a_google_service_account if {
	refused(remove("/spec/serviceAccountTemplate"), "iam.gke.io/gcp-service-account")
}

test_deny_backup_with_a_malformed_google_service_account if {
	planted := set("/spec/serviceAccountTemplate/metadata/annotations/iam.gke.io~1gcp-service-account", "someone@example.com")
	refused(planted, "iam.gke.io/gcp-service-account")
}

test_deny_no_scheduled_backup if {
	refused(fixtures.db_without("ScheduledBackup"), "ScheduledBackup")
}

test_deny_scheduled_backup_of_another_cluster if {
	planted := fixtures.db_patch("ScheduledBackup", "lanekeeper-db-daily", [{"op": "replace", "path": "/spec/cluster/name", "value": "other"}])
	refused(planted, "ScheduledBackup")
}

test_deny_scheduled_backup_suspended if {
	planted := fixtures.db_patch("ScheduledBackup", "lanekeeper-db-daily", [{"op": "add", "path": "/spec/suspend", "value": true}])
	refused(planted, "suspend")
}

test_deny_scheduled_backup_by_another_method if {
	planted := fixtures.db_patch("ScheduledBackup", "lanekeeper-db-daily", [{"op": "replace", "path": "/spec/method", "value": "volumeSnapshot"}])
	refused(planted, "spec.method must be barmanObjectStore")
}

test_deny_scheduled_backup_not_daily if {
	planted := fixtures.db_patch("ScheduledBackup", "lanekeeper-db-daily", [{"op": "replace", "path": "/spec/schedule", "value": "0 0 2 * * 1"}])
	refused(planted, "daily")
}

# kind has no Google: no backups, said with a warning.
test_kind_without_backup_warns if {
	warned(fixtures.db_kind, "no backup")
}

test_deny_kind_label_does_not_excuse_a_backup_to_a_key_file if {
	planted := fixtures.db_kind_patch([{"op": "add", "path": "/spec/backup", "value": {"retentionPolicy": "30d", "barmanObjectStore": {"destinationPath": "gs://b/p", "googleCredentials": {"applicationCredentials": {"name": "k", "key": "k"}}}}}])
	refused(planted, "applicationCredentials")
}

# --- the pods (S16)

test_deny_seccomp_missing if {
	refused(remove("/spec/seccompProfile"), "seccompProfile")
}

test_deny_seccomp_unconfined if {
	refused(set("/spec/seccompProfile/type", "Unconfined"), "seccompProfile")
}

test_deny_security_context_missing if {
	refused(remove("/spec/securityContext"), "securityContext")
}

test_deny_privilege_escalation if {
	refused(set("/spec/securityContext/allowPrivilegeEscalation", true), "allowPrivilegeEscalation")
}

test_deny_privileged if {
	refused(set("/spec/securityContext/privileged", true), "privileged")
}

test_deny_writable_root_filesystem if {
	refused(set("/spec/securityContext/readOnlyRootFilesystem", false), "readOnlyRootFilesystem")
}

test_deny_running_as_root if {
	refused(set("/spec/securityContext/runAsNonRoot", false), "runAsNonRoot")
}

test_deny_capabilities_kept if {
	refused(set("/spec/securityContext/capabilities/drop", ["NET_RAW"]), "drop ALL")
}

test_deny_capabilities_added if {
	refused(add("/spec/securityContext/capabilities/add", ["NET_ADMIN"]), "capabilities")
}

test_deny_pod_not_required_to_be_non_root if {
	refused(add("/spec/podSecurityContext", {"runAsNonRoot": false}), "podSecurityContext.runAsNonRoot")
}

test_deny_pod_running_as_root if {
	refused(add("/spec/podSecurityContext", {"runAsUser": 0}), "runAsUser")
}

# --- resources (bounded)

test_deny_requests_cpu_missing if {
	refused(remove("/spec/resources/requests/cpu"), "resources.requests.cpu")
}

test_deny_requests_memory_missing if {
	refused(remove("/spec/resources/requests/memory"), "resources.requests.memory")
}

test_deny_limits_cpu_missing if {
	refused(remove("/spec/resources/limits/cpu"), "resources.limits.cpu")
}

test_deny_limits_memory_missing if {
	refused(remove("/spec/resources/limits/memory"), "resources.limits.memory")
}

test_deny_resources_missing if {
	refused(remove("/spec/resources"), "resources.requests.cpu")
}

test_deny_storage_size_missing if {
	refused(remove("/spec/storage/size"), "storage.size")
}

# --- the spread (D62)

test_deny_zone_spread_preferred if {
	refused(set("/spec/affinity/podAntiAffinityType", "preferred"), "required")
}

test_deny_spread_by_host_in_prod if {
	refused(set("/spec/affinity/topologyKey", "kubernetes.io/hostname"), "topology.kubernetes.io/zone")
}

test_deny_anti_affinity_off if {
	refused(set("/spec/affinity/enablePodAntiAffinity", false), "enablePodAntiAffinity")
}

test_deny_affinity_missing if {
	refused(remove("/spec/affinity"), "topology.kubernetes.io/zone")
}

test_kind_spread_by_host_warns if {
	warned(fixtures.db_kind, "spread over hosts, not zones")
}

test_deny_kind_spread_over_zones if {
	planted := fixtures.db_kind_patch([{"op": "replace", "path": "/spec/affinity/topologyKey", "value": "topology.kubernetes.io/zone"}])
	refused(planted, "kubernetes.io/hostname")
}

test_deny_kind_without_any_spread if {
	refused(fixtures.db_kind_patch([{"op": "replace", "path": "/spec/affinity/enablePodAntiAffinity", "value": false}]), "enablePodAntiAffinity")
}

# --- the environment

test_deny_environment_missing if {
	refused(fixtures.db_patch("Cluster", "lanekeeper-db", [{"op": "remove", "path": "/metadata/labels/lanekeeper.io~1environment"}]), "lanekeeper.io/environment")
}

test_deny_environment_unknown if {
	planted := fixtures.db_patch("Cluster", "lanekeeper-db", [{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1environment", "value": "dev"}])
	refused(planted, "lanekeeper.io/environment")
}

# --- the disks

test_deny_class_that_deletes_the_database_with_its_claim if {
	planted := fixtures.db_patch("StorageClass", "lanekeeper-db-cmek", [{"op": "replace", "path": "/reclaimPolicy", "value": "Delete"}])
	refused(planted, "Retain")
}

test_deny_class_without_a_reclaim_policy if {
	planted := fixtures.db_patch("StorageClass", "lanekeeper-db-cmek", [{"op": "remove", "path": "/reclaimPolicy"}])
	refused(planted, "Retain")
}

# --- the network (S18): who may reach the database

netpol(ops) := fixtures.db_patch("NetworkPolicy", "lanekeeper-db", ops)

test_deny_without_a_network_policy if {
	refused(fixtures.db_without("NetworkPolicy"), "NetworkPolicy/lanekeeper-db is missing")
}

test_deny_without_default_deny if {
	refused(netpol([{"op": "replace", "path": "/spec/policyTypes", "value": []}]), "not default deny")
}

test_deny_policy_that_selects_every_pod if {
	refused(netpol([{"op": "replace", "path": "/spec/podSelector", "value": {}}]), "selects pods other than")
}

test_deny_policy_that_selects_the_hub_instead if {
	refused(netpol([{"op": "replace", "path": "/spec/podSelector/matchLabels", "value": {"app.kubernetes.io/name": "lanekeeper-hub"}}]), "selects pods other than")
}

test_deny_ingress_from_the_internet if {
	planted := netpol([{"op": "add", "path": "/spec/ingress/-", "value": fixtures.db_ingress([{"ipBlock": {"cidr": "0.0.0.0/0"}}], 5432)}])
	refused(planted, "ingress must be exactly")
}

test_deny_ingress_from_an_address_block if {
	planted := netpol([{"op": "add", "path": "/spec/ingress/-", "value": fixtures.db_ingress([{"ipBlock": {"cidr": "192.0.2.0/24"}}], 5432)}])
	refused(planted, "ingress must be exactly")
}

test_deny_ingress_from_every_pod if {
	planted := netpol([{"op": "add", "path": "/spec/ingress/-", "value": fixtures.db_ingress([{"podSelector": {}}], 5432)}])
	refused(planted, "ingress must be exactly")
}

test_deny_ingress_from_every_namespace if {
	planted := netpol([{"op": "add", "path": "/spec/ingress/-", "value": fixtures.db_ingress([{"namespaceSelector": {}}], 5432)}])
	refused(planted, "ingress must be exactly")
}

test_deny_hub_pods_of_every_namespace if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/0/from/0", "value": {"namespaceSelector": {}, "podSelector": {"matchLabels": {"app.kubernetes.io/name": "lanekeeper-hub"}}}}])
	refused(planted, "ingress must be exactly")
}

test_deny_an_operator_rule_without_the_operator_pod if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/1/from/0", "value": {"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "cnpg-system"}}}}])
	refused(planted, "ingress must be exactly")
}

test_deny_the_hub_on_the_status_port if {
	planted := netpol([{"op": "add", "path": "/spec/ingress/-", "value": fixtures.db_ingress(fixtures.db_hub_caller, 8000)}])
	refused(planted, "ingress must be exactly")
}

test_deny_the_hub_on_ssh if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/0/ports/0/port", "value": 22}])
	refused(planted, "ingress must be exactly")
}

test_deny_an_ingress_rule_without_callers if {
	planted := netpol([{"op": "add", "path": "/spec/ingress/-", "value": {"ports": [{"protocol": "TCP", "port": 5432}]}}])
	refused(planted, "names no callers")
}

test_deny_an_ingress_rule_without_ports if {
	planted := netpol([{"op": "add", "path": "/spec/ingress/-", "value": {"from": fixtures.db_hub_caller}}])
	refused(planted, "exactly one port")
}

test_deny_an_ingress_rule_with_two_ports if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/0/ports", "value": [{"protocol": "TCP", "port": 5432}, {"protocol": "TCP", "port": 8000}]}])
	refused(planted, "exactly one port")
}

test_deny_the_hub_missing_from_the_callers if {
	planted := netpol([{"op": "remove", "path": "/spec/ingress/0"}])
	refused(planted, "ingress must be exactly")
}

test_deny_the_operator_missing_from_the_callers if {
	planted := netpol([{"op": "remove", "path": "/spec/ingress/2"}])
	refused(planted, "ingress must be exactly")
}

# The hub image's Jobs (migrate and seed; 09/fix, J3) reach the database from pods that carry a label of their own, on 5432 and nothing else.
test_deny_the_jobs_missing_from_the_callers if {
	planted := netpol([{"op": "remove", "path": "/spec/ingress/5"}])
	refused(planted, "ingress must be exactly")
}

test_deny_the_jobs_on_the_status_port if {
	planted := netpol([{"op": "add", "path": "/spec/ingress/-", "value": fixtures.db_ingress(fixtures.db_jobs_caller, 8000)}])
	refused(planted, "ingress must be exactly")
}

test_deny_the_jobs_pods_of_every_namespace if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/5/from/0", "value": {"namespaceSelector": {}, "podSelector": {"matchLabels": {"app.kubernetes.io/name": "lanekeeper-hub-jobs"}}}}])
	refused(planted, "ingress must be exactly")
}

test_deny_every_pod_of_the_lanekeeper_part if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/5/from/0/podSelector/matchLabels", "value": {"app.kubernetes.io/part-of": "lanekeeper"}}])
	refused(planted, "ingress must be exactly")
}

test_deny_the_jobs_label_with_a_wildcard_selector if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/5/from/0", "value": {"podSelector": {}}}])
	refused(planted, "ingress must be exactly")
}

test_deny_the_jobs_on_ssh if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/5/ports/0/port", "value": 22}])
	refused(planted, "ingress must be exactly")
}

test_deny_a_second_policy_that_opens_the_database if {
	open_policy := {
		"apiVersion": "networking.k8s.io/v1",
		"kind": "NetworkPolicy",
		"metadata": {"name": "lanekeeper-db-open", "namespace": "lanekeeper"},
		"spec": {
			"podSelector": {"matchLabels": {"cnpg.io/cluster": "lanekeeper-db"}},
			"policyTypes": ["Ingress"],
			"ingress": [{"ports": [{"protocol": "TCP", "port": 5432}]}],
		},
	}
	refused(fixtures.db_with_doc(open_policy), "policies add up")
}

test_a_network_policy_of_another_chart_is_not_judged_here if {
	count(deny) == 0 with input as fixtures.hub
}
