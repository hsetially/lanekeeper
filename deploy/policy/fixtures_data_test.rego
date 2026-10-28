# The documents the data-layer policy tests start from (09/T5). `db` is what `helm template` makes of the db chart (the CloudNativePG
# cluster, its scheduled backup, its StorageClass and its NetworkPolicy), `gs` is what the gcp chart makes of the buckets and the IAM
# around them (with Cloud Armor off, see deploy/charts/gcp/README.md), both written out as conftest's `--combine` input (lib.rego).
# They take the same helpers as the hub's documents in fixtures_test.rego: `db_patch` and `gs_patch` change one document with a JSON
# patch, `db_without` and `gs_without` drop the documents of a kind, `db_with_doc` and `gs_with_doc` add one.
#
# `deploy/tests/render.sh` runs the real charts through the same policies, so a document here that stops resembling the chart's
# output does not hide a gap: the real output is checked too. Keep them alike; when a chart changes shape, change this.
# Example names and documentation addresses only (RFC 5737), never a real project, key or address.
package fixtures

import rego.v1

# --- the database -------------------------------------------------------------------------------------------------------------

db_name := "lanekeeper-db"

db_digest := "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

db_disk_key := "projects/example-project/locations/example-location/keyRings/example/cryptoKeys/disk"

db_labels := {
	"app.kubernetes.io/name": "lanekeeper-db",
	"app.kubernetes.io/instance": "lanekeeper",
	"app.kubernetes.io/part-of": "lanekeeper",
	"lanekeeper.io/environment": "prod",
}

db_storageclass := {
	"apiVersion": "storage.k8s.io/v1",
	"kind": "StorageClass",
	"metadata": {"name": "lanekeeper-db-cmek", "labels": db_labels},
	"provisioner": "pd.csi.storage.gke.io",
	"parameters": {"type": "pd-ssd", "disk-encryption-kms-key": db_disk_key},
	"reclaimPolicy": "Retain",
	"volumeBindingMode": "WaitForFirstConsumer",
	"allowVolumeExpansion": true,
}

db_cluster := {
	"apiVersion": "postgresql.cnpg.io/v1",
	"kind": "Cluster",
	"metadata": {"name": db_name, "namespace": namespace, "labels": db_labels},
	"spec": {
		"instances": 3,
		"imageName": concat("", ["ghcr.io/cloudnative-pg/postgresql:16.15-202610050823-system-trixie@", db_digest]),
		"imagePullPolicy": "IfNotPresent",
		"enableSuperuserAccess": false,
		"enablePDB": true,
		"seccompProfile": {"type": "RuntimeDefault"},
		"securityContext": {
			"allowPrivilegeEscalation": false,
			"privileged": false,
			"readOnlyRootFilesystem": true,
			"runAsNonRoot": true,
			"capabilities": {"drop": ["ALL"]},
		},
		"serviceAccountTemplate": {"metadata": {"annotations": {"iam.gke.io/gcp-service-account": "lanekeeper-db@example-project.iam.gserviceaccount.com"}}},
		"bootstrap": {"initdb": {
			"database": "lanekeeper",
			"owner": "lanekeeper_migrator",
			"secret": {"name": "lanekeeper-db-migrator"},
			"encoding": "UTF8",
			"localeCollate": "en_US.utf8",
			"localeCType": "en_US.utf8",
			"postInitApplicationSQL": [
				"CREATE EXTENSION IF NOT EXISTS vector",
				"CREATE EXTENSION IF NOT EXISTS pg_trgm",
			],
		}},
		"postgresql": {
			"parameters": {
				"ssl_min_protocol_version": "TLSv1.3",
				"ssl_max_protocol_version": "TLSv1.3",
				"password_encryption": "scram-sha-256",
				"shared_buffers": "512MB",
			},
			"pg_hba": ["hostnossl all all all reject"],
		},
		"managed": {"roles": [{
			"name": "lanekeeper_app",
			"ensure": "present",
			"login": true,
			"superuser": false,
			"createdb": false,
			"createrole": false,
			"replication": false,
			"bypassrls": false,
			"passwordSecret": {"name": "lanekeeper-db-app"},
		}]},
		"storage": {"size": "20Gi", "storageClass": "lanekeeper-db-cmek"},
		"resources": {
			"requests": {"cpu": "1", "memory": "2Gi"},
			"limits": {"cpu": "2", "memory": "2Gi"},
		},
		"affinity": {
			"enablePodAntiAffinity": true,
			"topologyKey": "topology.kubernetes.io/zone",
			"podAntiAffinityType": "required",
		},
		"backup": {
			"retentionPolicy": "30d",
			"barmanObjectStore": {
				"destinationPath": "gs://example-project-lanekeeper-backup-prod/lanekeeper-db",
				"googleCredentials": {"gkeEnvironment": true},
				"wal": {"compression": "gzip"},
				"data": {"compression": "gzip"},
			},
		},
	},
}

db_scheduledbackup := {
	"apiVersion": "postgresql.cnpg.io/v1",
	"kind": "ScheduledBackup",
	"metadata": {"name": "lanekeeper-db-daily", "namespace": namespace, "labels": db_labels},
	"spec": {
		"schedule": "0 0 2 * * *",
		"cluster": {"name": db_name},
		"method": "barmanObjectStore",
		"backupOwnerReference": "self",
		"immediate": true,
	},
}

db_ingress(from, port) := {"from": from, "ports": [{"protocol": "TCP", "port": port}]}

db_hub_caller := [{"podSelector": {"matchLabels": {"app.kubernetes.io/name": "lanekeeper-hub"}}}]

# The pods of the hub image's Jobs (charts/hub): a label of their own, so that the hub's policy does not select them.
db_jobs_caller := [{"podSelector": {"matchLabels": {"app.kubernetes.io/name": "lanekeeper-hub-jobs"}}}]

db_operator_caller := [{
	"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "cnpg-system"}},
	"podSelector": {"matchLabels": {"app.kubernetes.io/name": "cloudnative-pg"}},
}]

db_peer_caller := [{"podSelector": {"matchLabels": {"cnpg.io/cluster": "lanekeeper-db"}}}]

db_networkpolicy := {
	"apiVersion": "networking.k8s.io/v1",
	"kind": "NetworkPolicy",
	"metadata": {"name": db_name, "namespace": namespace, "labels": db_labels},
	"spec": {
		"podSelector": {"matchLabels": {"cnpg.io/cluster": "lanekeeper-db"}},
		"policyTypes": ["Ingress"],
		"ingress": [
			db_ingress(db_hub_caller, 5432),
			db_ingress(db_operator_caller, 5432),
			db_ingress(db_operator_caller, 8000),
			db_ingress(db_peer_caller, 5432),
			db_ingress(db_peer_caller, 8000),
			db_ingress(db_jobs_caller, 5432),
		],
	},
}

db_docs := [db_storageclass, db_cluster, db_scheduledbackup, db_networkpolicy]

# The input of a conftest run over the db chart's output.
db := [{"path": "rendered/db.yaml", "contents": db_docs}]

db_patch(kind, doc_name, ops) := [{"path": "rendered/db.yaml", "contents": changed(db_docs, kind, doc_name, ops)}]

db_without(kind) := [{"path": "rendered/db.yaml", "contents": [d | some d in db_docs; d.kind != kind]}]

db_with_doc(doc) := [{"path": "rendered/db.yaml", "contents": array.concat(db_docs, [doc])}]

# kind: no zones, no Google (so no Workload Identity, no backup and no customer-managed key), one cluster of three on its workers.
db_kind_cluster := json.patch(db_cluster, [
	{"op": "replace", "path": "/metadata/labels/lanekeeper.io~1environment", "value": "kind"},
	{"op": "add", "path": "/metadata/annotations", "value": {"lanekeeper.io/cmek": "fallback"}},
	{"op": "replace", "path": "/spec/storage/storageClass", "value": "standard"},
	{"op": "replace", "path": "/spec/affinity/topologyKey", "value": "kubernetes.io/hostname"},
	{"op": "replace", "path": "/spec/affinity/podAntiAffinityType", "value": "preferred"},
	{"op": "remove", "path": "/spec/serviceAccountTemplate"},
	{"op": "remove", "path": "/spec/backup"},
])

db_kind := [{"path": "rendered/db.yaml", "contents": [db_kind_cluster, db_networkpolicy]}]

db_kind_patch(ops) := [{"path": "rendered/db.yaml", "contents": [json.patch(db_kind_cluster, ops), db_networkpolicy]}]

db_cluster_set(path, value) := db_patch("Cluster", db_name, [{"op": "replace", "path": path, "value": value}])

db_cluster_add(path, value) := db_patch("Cluster", db_name, [{"op": "add", "path": path, "value": value}])

db_cluster_remove(path) := db_patch("Cluster", db_name, [{"op": "remove", "path": path}])

# --- the Google Cloud resources around the data (the gcp chart without Cloud Armor) -------------------------------------------------

gs_project := "example-project"

gs_namespace := "lanekeeper-gcp"

gs_key(name) := concat("", ["projects/example-project/locations/example-location/keyRings/example/cryptoKeys/", name])

gs_labels(purpose) := {
	"app.kubernetes.io/name": "lanekeeper-gcp",
	"app.kubernetes.io/instance": "lanekeeper-gcp",
	"app.kubernetes.io/part-of": "lanekeeper",
	"lanekeeper.io/purpose": purpose,
	"lanekeeper.io/environment": "prod",
}

gs_annotations := {
	"cnrm.cloud.google.com/project-id": gs_project,
	"cnrm.cloud.google.com/deletion-policy": "abandon",
	"argocd.argoproj.io/sync-options": "Prune=false",
}

gs_bucket(name, purpose, extra) := {
	"apiVersion": "storage.cnrm.cloud.google.com/v1beta1",
	"kind": "StorageBucket",
	"metadata": {"name": name, "namespace": gs_namespace, "labels": gs_labels(purpose), "annotations": gs_annotations},
	"spec": object.union({
		"location": "example-location",
		"storageClass": "STANDARD",
		"uniformBucketLevelAccess": true,
		"publicAccessPrevention": "enforced",
		"encryption": {"kmsKeyRef": {"external": gs_key(concat("-", ["lanekeeper", purpose]))}},
	}, extra),
}

gs_backup_bucket := gs_bucket("example-project-lanekeeper-backup-prod", "backup", {})

gs_audit_bucket := gs_bucket("example-project-lanekeeper-audit-prod", "audit-checkpoint", {"retentionPolicy": {"retentionPeriod": 7776000, "isLocked": true}})

gs_docs_bucket := gs_bucket("example-project-lanekeeper-docs-prod", "docs", {"versioning": {"enabled": true}})

gs_serviceaccount(name) := {
	"apiVersion": "iam.cnrm.cloud.google.com/v1beta1",
	"kind": "IAMServiceAccount",
	"metadata": {"name": name, "namespace": gs_namespace, "labels": gs_labels("identity"), "annotations": {"cnrm.cloud.google.com/project-id": gs_project}},
	"spec": {"displayName": name},
}

gs_gsa(name) := concat("", ["serviceAccount:", name, "@example-project.iam.gserviceaccount.com"])

gs_member(name, role, ref, who) := {
	"apiVersion": "iam.cnrm.cloud.google.com/v1beta1",
	"kind": "IAMPolicyMember",
	"metadata": {"name": name, "namespace": gs_namespace, "labels": gs_labels("iam"), "annotations": {"cnrm.cloud.google.com/project-id": gs_project}},
	"spec": {"member": who, "role": role, "resourceRef": ref},
}

gs_bucket_ref(name) := {"apiVersion": "storage.cnrm.cloud.google.com/v1beta1", "kind": "StorageBucket", "name": name}

# The hub's keys and the Secret Manager secrets it reads (09/fix, I1): a key ring, four keys, ten containers. The names are the ones of
# deploy/values/gcp/prod.yaml; the hub chart's values name the same ones (deploy/tests/render.sh contract checks it).
gs_ring_name := "lanekeeper"

gs_keep_annotations := {
	"cnrm.cloud.google.com/project-id": gs_project,
	"cnrm.cloud.google.com/deletion-policy": "abandon",
	"argocd.argoproj.io/sync-options": "Prune=false",
}

gs_keyring := {
	"apiVersion": "kms.cnrm.cloud.google.com/v1beta1",
	"kind": "KMSKeyRing",
	"metadata": {"name": gs_ring_name, "namespace": gs_namespace, "labels": gs_labels("key-ring"), "annotations": gs_keep_annotations},
	"spec": {"location": "example-location"},
}

gs_signing_key(name, purpose) := {
	"apiVersion": "kms.cnrm.cloud.google.com/v1beta1",
	"kind": "KMSCryptoKey",
	"metadata": {"name": name, "namespace": gs_namespace, "labels": gs_labels(purpose), "annotations": gs_keep_annotations},
	"spec": {
		"keyRingRef": {"name": gs_ring_name},
		"purpose": "ASYMMETRIC_SIGN",
		"versionTemplate": {"algorithm": "EC_SIGN_P256_SHA256", "protectionLevel": "HSM"},
		"destroyScheduledDuration": "2592000s",
	},
}

gs_vault_key := {
	"apiVersion": "kms.cnrm.cloud.google.com/v1beta1",
	"kind": "KMSCryptoKey",
	"metadata": {"name": "lanekeeper-vault-prod", "namespace": gs_namespace, "labels": gs_labels("vault"), "annotations": gs_keep_annotations},
	"spec": {
		"keyRingRef": {"name": gs_ring_name},
		"purpose": "ENCRYPT_DECRYPT",
		"versionTemplate": {"algorithm": "GOOGLE_SYMMETRIC_ENCRYPTION", "protectionLevel": "HSM"},
		"rotationPeriod": "7776000s",
		"destroyScheduledDuration": "2592000s",
	},
}

gs_keys := [
	gs_signing_key("lanekeeper-agent-ca-prod", "agent-ca"),
	gs_signing_key("lanekeeper-replica-ca-prod", "replica-ca"),
	gs_signing_key("lanekeeper-audit-checkpoint-prod", "audit-checkpoint-signing"),
	gs_vault_key,
]

gs_kms_ref(name) := {"apiVersion": "kms.cnrm.cloud.google.com/v1beta1", "kind": "KMSCryptoKey", "name": name}

gs_secret_names := [
	"lanekeeper-web-cert", "lanekeeper-web-key", "lanekeeper-agent-ca-cert", "lanekeeper-replica-ca-cert", "lanekeeper-entra-client",
	"lanekeeper-login-cookie", "write-preview-key", "github-read-token", "github-webhook-secret", "mcp-token-hmac-key",
]

gs_secrets_key := gs_key("lanekeeper-secrets")

gs_secret(name) := {
	"apiVersion": "secretmanager.cnrm.cloud.google.com/v1beta1",
	"kind": "SecretManagerSecret",
	"metadata": {"name": name, "namespace": gs_namespace, "labels": gs_labels("secret"), "annotations": gs_keep_annotations},
	"spec": {"replication": {"userManaged": {"replicas": [{
		"location": "example-location",
		"customerManagedEncryption": {"kmsKeyRef": {"external": gs_secrets_key}},
	}]}}},
}

gs_secrets := [gs_secret(n) | some n in gs_secret_names]

gs_secret_ref(name) := {"apiVersion": "secretmanager.cnrm.cloud.google.com/v1beta1", "kind": "SecretManagerSecret", "name": name}

gs_members := [
	gs_member("lanekeeper-hub-workload-identity", "roles/iam.workloadIdentityUser", {"apiVersion": "iam.cnrm.cloud.google.com/v1beta1", "kind": "IAMServiceAccount", "name": "lanekeeper-hub"}, "serviceAccount:example-project.svc.id.goog[lanekeeper/lanekeeper-hub]"),
	gs_member("lanekeeper-db-workload-identity", "roles/iam.workloadIdentityUser", {"apiVersion": "iam.cnrm.cloud.google.com/v1beta1", "kind": "IAMServiceAccount", "name": "lanekeeper-db"}, "serviceAccount:example-project.svc.id.goog[lanekeeper/lanekeeper-db]"),
	gs_member("lanekeeper-db-backup", "roles/storage.objectUser", gs_bucket_ref("example-project-lanekeeper-backup-prod"), gs_gsa("lanekeeper-db")),
	gs_member("lanekeeper-hub-audit-create", "roles/storage.objectCreator", gs_bucket_ref("example-project-lanekeeper-audit-prod"), gs_gsa("lanekeeper-hub")),
	gs_member("lanekeeper-hub-audit-read", "roles/storage.objectViewer", gs_bucket_ref("example-project-lanekeeper-audit-prod"), gs_gsa("lanekeeper-hub")),
	gs_member("lanekeeper-hub-docs", "roles/storage.objectUser", gs_bucket_ref("example-project-lanekeeper-docs-prod"), gs_gsa("lanekeeper-hub")),
	gs_member("lanekeeper-hub-trace", "roles/cloudtrace.agent", {"kind": "Project", "external": "projects/example-project"}, gs_gsa("lanekeeper-hub")),
	gs_member("lanekeeper-gcs-agent-key1", "roles/cloudkms.cryptoKeyEncrypterDecrypter", {"kind": "KMSCryptoKey", "external": gs_key("lanekeeper-backup")}, gs_storage_agent),
	gs_member("lanekeeper-gcs-agent-key2", "roles/cloudkms.cryptoKeyEncrypterDecrypter", {"kind": "KMSCryptoKey", "external": gs_key("lanekeeper-audit-checkpoint")}, gs_storage_agent),
	gs_member("lanekeeper-gcs-agent-key3", "roles/cloudkms.cryptoKeyEncrypterDecrypter", {"kind": "KMSCryptoKey", "external": gs_key("lanekeeper-docs")}, gs_storage_agent),
	gs_member("lanekeeper-compute-agent-key1", "roles/cloudkms.cryptoKeyEncrypterDecrypter", {"kind": "KMSCryptoKey", "external": db_disk_key}, gs_compute_agent),
	gs_member("lanekeeper-secretmanager-agent-key1", "roles/cloudkms.cryptoKeyEncrypterDecrypter", {"kind": "KMSCryptoKey", "external": gs_secrets_key}, gs_secretmanager_agent),
	gs_member("lanekeeper-hub-sign-agent-ca", "roles/cloudkms.signerVerifier", gs_kms_ref("lanekeeper-agent-ca-prod"), gs_gsa("lanekeeper-hub")),
	gs_member("lanekeeper-hub-sign-replica-ca", "roles/cloudkms.signerVerifier", gs_kms_ref("lanekeeper-replica-ca-prod"), gs_gsa("lanekeeper-hub")),
	gs_member("lanekeeper-hub-sign-audit-checkpoint", "roles/cloudkms.signerVerifier", gs_kms_ref("lanekeeper-audit-checkpoint-prod"), gs_gsa("lanekeeper-hub")),
	gs_member("lanekeeper-hub-vault", "roles/cloudkms.cryptoKeyEncrypterDecrypter", gs_kms_ref("lanekeeper-vault-prod"), gs_gsa("lanekeeper-hub")),
]

# One binding for each secret the hub reads, on that secret and on nothing wider.
gs_secret_members := [gs_member(concat("-", ["lanekeeper-hub-read", n]), "roles/secretmanager.secretAccessor", gs_secret_ref(n), gs_gsa("lanekeeper-hub")) | some n in gs_secret_names]

# The Google-managed service agents that use a customer-managed key on the project's behalf (the project number is an example).
gs_storage_agent := "serviceAccount:service-123456789012@gs-project-accounts.iam.gserviceaccount.com"

gs_compute_agent := "serviceAccount:service-123456789012@compute-system.iam.gserviceaccount.com"

gs_secretmanager_agent := "serviceAccount:service-123456789012@gcp-sa-secretmanager.iam.gserviceaccount.com"

gs_docs := array.concat(
	array.concat(
		[gs_backup_bucket, gs_audit_bucket, gs_docs_bucket, gs_serviceaccount("lanekeeper-hub"), gs_serviceaccount("lanekeeper-db"), gs_keyring],
		array.concat(gs_keys, gs_secrets),
	),
	array.concat(gs_members, gs_secret_members),
)

# The input of a conftest run over the gcp chart's output without Cloud Armor.
gs := [{"path": "rendered/gcp-storage.yaml", "contents": gs_docs}]

gs_patch(kind, doc_name, ops) := [{"path": "rendered/gcp-storage.yaml", "contents": changed(gs_docs, kind, doc_name, ops)}]

gs_without(kind) := [{"path": "rendered/gcp-storage.yaml", "contents": [d | some d in gs_docs; d.kind != kind]}]

gs_with_doc(doc) := [{"path": "rendered/gcp-storage.yaml", "contents": array.concat(gs_docs, [doc])}]

gs_with(docs) := [{"path": "rendered/gcp-storage.yaml", "contents": docs}]

gs_replace(old_doc, new_doc) := [{"path": "rendered/gcp-storage.yaml", "contents": [d | some doc in gs_docs; d := replaced(doc, old_doc, new_doc)]}]

replaced(doc, old_doc, new_doc) := new_doc if doc == old_doc

replaced(doc, old_doc, _) := doc if doc != old_doc

gs_bucket_set(name, path, value) := gs_patch("StorageBucket", name, [{"op": "replace", "path": path, "value": value}])

gs_bucket_add(name, path, value) := gs_patch("StorageBucket", name, [{"op": "add", "path": path, "value": value}])

gs_bucket_remove(name, path) := gs_patch("StorageBucket", name, [{"op": "remove", "path": path}])

gs_member_set(name, path, value) := gs_patch("IAMPolicyMember", name, [{"op": "replace", "path": path, "value": value}])

gs_key_set(name, path, value) := gs_patch("KMSCryptoKey", name, [{"op": "replace", "path": path, "value": value}])

gs_key_add(name, path, value) := gs_patch("KMSCryptoKey", name, [{"op": "add", "path": path, "value": value}])

gs_key_remove(name, path) := gs_patch("KMSCryptoKey", name, [{"op": "remove", "path": path}])

gs_secret_set(name, path, value) := gs_patch("SecretManagerSecret", name, [{"op": "replace", "path": path, "value": value}])

gs_secret_add(name, path, value) := gs_patch("SecretManagerSecret", name, [{"op": "add", "path": path, "value": value}])

gs_secret_remove(name, path) := gs_patch("SecretManagerSecret", name, [{"op": "remove", "path": path}])
