# Tests for secrets.rego: no Kubernetes Secret in the charts' output except the agent's empty certificate Secret, no secret in a
# pod's environment, and no secret in a values file (S7).
package secrets

import rego.v1

import data.fixtures

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

env_op(env) := {"op": "add", "path": "/spec/template/spec/containers/0/env/-", "value": env}

secret(name, extra) := object.union({"apiVersion": "v1", "kind": "Secret", "metadata": {"name": name}}, extra)

values_input(contents) := [{"path": "deploy/values/hub/prod.yaml", "contents": contents}]

test_good_passes if {
	count(deny) == 0 with input as fixtures.hub
}

# --- Secret objects (S7): secrets come from Secret Manager; CloudNativePG creates its own, and the agent's certificate Secret
# is the one the chart may render, empty.

test_deny_secret_object if {
	refused(fixtures.with_doc(secret("db-password", {"stringData": {"password": "x"}})), "Secret")
}

test_deny_secret_object_without_data if {
	refused(fixtures.with_doc(secret("anything", {})), "Secret")
}

test_agent_cert_secret_is_allowed_empty if {
	count(deny) == 0 with input as fixtures.with_doc(secret("lanekeeper-agent-cert", {"type": "kubernetes.io/tls"}))
}

test_deny_agent_cert_secret_with_data if {
	refused(fixtures.with_doc(secret("lanekeeper-agent-cert", {"data": {"tls.key": "AAAA"}})), "empty")
}

test_deny_agent_cert_secret_with_string_data if {
	refused(fixtures.with_doc(secret("lanekeeper-agent-cert", {"stringData": {"tls.key": "x"}})), "empty")
}

# --- the environment (S7): files and Secret Manager, never a variable

test_deny_env_secretkeyref if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [env_op({"name": "LK_WEB_DIST", "valueFrom": {"secretKeyRef": {"name": "s", "key": "k"}}})])
	refused(planted, "secretKeyRef")
}

test_deny_env_from_secret if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [{"op": "add", "path": "/spec/template/spec/containers/0/envFrom", "value": [{"secretRef": {"name": "s"}}]}])
	refused(planted, "secretRef")
}

test_deny_secret_looking_env_name if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [env_op({"name": "LK_GIT_TOKEN_VALUE", "value": "x"})])
	refused(planted, "looks like a secret")
}

test_deny_password_env_name if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [env_op({"name": "DB_PASSWORD", "value": "x"})])
	refused(planted, "looks like a secret")
}

test_deny_key_suffix_env_name if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [env_op({"name": "SIGNING_KEY", "value": "x"})])
	refused(planted, "looks like a secret")
}

test_deny_database_url_in_environment if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [env_op({"name": "LK_DATABASE_URL", "value": "postgres://u@h/db"})])
	refused(planted, "LK_DATABASE_URL_FILE")
}

# --- values files (S7): no secret value in Git

test_values_without_secrets_pass if {
	good := values_input({"image": {"repository": "ghcr.io/x/y", "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}, "database": {"credentialsSecretName": "lanekeeper-db-app"}})
	count(deny) == 0 with input as good
}

test_deny_secret_in_values if {
	refused(values_input({"database": {"password": "hunter2hunter2"}}), "password")
}

test_deny_secret_in_values_token_key if {
	refused(values_input({"git": {"token": "abc"}}), "token")
}

test_deny_secret_in_values_nested_list if {
	refused(values_input({"extra": [{"clientSecret": "abc"}]}), "clientSecret")
}

test_empty_secret_key_in_values_is_not_a_secret if {
	count(deny) == 0 with input as values_input({"database": {"password": ""}})
}

test_deny_private_key_in_values if {
	refused(values_input({"tls": {"pem": "-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----"}}), "private key")
}

test_deny_github_token_in_values if {
	refused(values_input({"note": "ghp_0123456789abcdefghijklmnopqrstuvwxyzAB"}), "token")
}

test_deny_cloud_api_key_in_values if {
	refused(values_input({"note": "AIzaSyA-0123456789abcdefghijklmnopqrstuv"}), "API key")
}

test_deny_jwt_in_values if {
	refused(values_input({"note": "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk"}), "token")
}

test_rendered_documents_are_not_scanned_as_values if {
	count(deny) == 0 with input as fixtures.hub
}
