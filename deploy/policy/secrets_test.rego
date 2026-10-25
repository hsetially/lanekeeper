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

# The agent's certificate Secret as the chart renders it: empty, Opaque, and kept when the release goes (09/T4).
agent_cert_secret(extra) := object.union(fixtures.agent_cert_secret, extra)

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
	count(deny) == 0 with input as fixtures.with_doc(agent_cert_secret({}))
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

# --- the agent's certificate Secret (09/T4, plan 09 A14)

test_agent_chart_output_passes if {
	count(deny) == 0 with input as fixtures.agent
}

# The agent renews its certificate into this Secret; a chart upgrade or an uninstall must not empty it, or the agent has to join again.
test_deny_agent_cert_secret_without_keep if {
	refused(fixtures.agent_patch("Secret", "lanekeeper-agent-cert", [{"op": "remove", "path": "/metadata/annotations"}]), "helm.sh/resource-policy")
}

test_deny_agent_cert_secret_with_another_policy if {
	refused(fixtures.agent_patch("Secret", "lanekeeper-agent-cert", [{"op": "replace", "path": "/metadata/annotations/helm.sh~1resource-policy", "value": "delete"}]), "helm.sh/resource-policy")
}

# A kubernetes.io/tls Secret is refused by the API server without tls.crt and tls.key, and the policy requires it empty.
test_deny_agent_cert_secret_of_type_tls if {
	refused(fixtures.agent_patch("Secret", "lanekeeper-agent-cert", [{"op": "replace", "path": "/type", "value": "kubernetes.io/tls"}]), "Opaque")
}

# --- the names of the agent's Secrets are variables, and are not secrets (09/T4)

test_agent_secret_name_variables_pass if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/containers/0/env/-", "value": {"name": "LK_JOIN_TOKEN_SECRET", "value": "lanekeeper-agent-join-token"}}])
	count(deny) == 0 with input as planted
}

test_deny_agent_cert_secret_variable_naming_another_secret if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "replace", "path": "/spec/template/spec/containers/0/env/9/value", "value": "lanekeeper-db-app"}])
	refused(planted, "must be lanekeeper-agent-cert")
}

test_deny_agent_join_token_variable_naming_another_secret if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/containers/0/env/-", "value": {"name": "LK_JOIN_TOKEN_SECRET", "value": "lanekeeper-db-app"}}])
	refused(planted, "must be lanekeeper-agent-join-token")
}

# Only those two names are exempt: any other variable that looks like a secret is refused as before.
test_deny_agent_secret_looking_variable_other_than_the_two_names if {
	planted := fixtures.agent_patch("Deployment", "lanekeeper-agent", [{"op": "add", "path": "/spec/template/spec/containers/0/env/-", "value": {"name": "LK_HUB_TOKEN", "value": "x"}}])
	refused(planted, "looks like a secret")
}

# --- the hub's CA is public; a private key in a ConfigMap is not (S7)

test_deny_private_key_in_a_config_map if {
	planted := fixtures.agent_patch("ConfigMap", "lanekeeper-hub-ca", [{"op": "add", "path": "/data/ca.key", "value": "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"}])
	refused(planted, "private key")
}

test_deny_ec_private_key_in_a_config_map if {
	planted := fixtures.agent_patch("ConfigMap", "lanekeeper-hub-ca", [{"op": "replace", "path": "/data/ca.pem", "value": "-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----\n"}])
	refused(planted, "private key")
}

test_certificate_in_a_config_map_is_fine if {
	count(deny) == 0 with input as fixtures.agent
}
