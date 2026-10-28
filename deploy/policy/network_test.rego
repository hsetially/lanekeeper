# Tests for network.rego (S18): the hub's pods deny by default and admit only what crates/hub/HARDENING.md lists, both load
# balancers have source ranges, the webhook path is the only door for GitHub's ranges, and Cloud Armor ends in a deny.
package network

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

refused_gcp(planted, fragment) if {
	msgs := deny with input as planted with data.cloudArmor as {"githubHookRanges": fixtures.github_hook_ranges}
	some msg in msgs
	contains(msg, fragment)
}

netpol(ops) := fixtures.patch("NetworkPolicy", "lanekeeper-hub", ops)

add_egress(rule) := netpol([{"op": "add", "path": "/spec/egress/-", "value": rule}])

add_ingress(rule) := netpol([{"op": "add", "path": "/spec/ingress/-", "value": rule}])

tcp(port) := {"protocol": "TCP", "port": port}

hooks_rule_text(expression) := {"action": "allow", "priority": 2000, "description": "GitHub webhooks", "match": {"expr": {"expression": expression}}}

weaker_rule := {"to": [{"ipBlock": {"cidr": "0.0.0.0/0", "except": ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16"]}}], "ports": [tcp(443)]}

# The hub's policy in kind: no FQDN policy, a 443 egress rule that skips private and link-local ranges, and the annotation.
weaker_input := [{"path": "rendered/hub.yaml", "contents": [d | some d in fixtures.hub_docs; d.kind != "FQDNNetworkPolicy"; not is_hubs_policy(d)]}, fixtures.budgets]

# The hub's own policy (the Jobs' stays: it is another policy, and the Jobs are in the input).
is_hubs_policy(d) if {
	d.kind == "NetworkPolicy"
	d.metadata.name == "lanekeeper-hub"
}

weaker_policy(rule) := json.patch(fixtures.networkpolicy, [
	{"op": "add", "path": "/metadata/annotations", "value": {"lanekeeper.io/weaker-egress": "true"}},
	{"op": "add", "path": "/spec/egress/-", "value": rule},
])

weaker_with(rule) := [{"path": "rendered/hub.yaml", "contents": array.concat(weaker_input[0].contents, [weaker_policy(rule)])}, fixtures.budgets]

test_good_passes if {
	count(deny) == 0 with input as fixtures.hub
	count(warn) == 0 with input as fixtures.hub
}

# --- default deny (S18)

test_deny_without_default_deny_egress if {
	refused(netpol([{"op": "replace", "path": "/spec/policyTypes", "value": ["Ingress"]}]), "default")
}

test_deny_without_default_deny_ingress if {
	refused(netpol([{"op": "replace", "path": "/spec/policyTypes", "value": ["Egress"]}]), "default")
}

test_deny_without_policy_types if {
	refused(netpol([{"op": "remove", "path": "/spec/policyTypes"}]), "default")
}

test_deny_without_default_deny if {
	refused(fixtures.without("NetworkPolicy"), "NetworkPolicy")
}

test_deny_policy_that_selects_every_pod if {
	refused(netpol([{"op": "replace", "path": "/spec/podSelector", "value": {}}]), "selects")
}

# --- egress: exactly the destinations of HARDENING.md (S18)

test_hub_egress_exact if {
	count(deny) == 0 with input as fixtures.hub
}

test_deny_egress_to_anywhere if {
	refused(add_egress({"to": [{"ipBlock": {"cidr": "0.0.0.0/0"}}], "ports": [tcp(443)]}), "unrecognised")
}

test_deny_egress_to_every_destination if {
	refused(add_egress({"ports": [tcp(443)]}), "every destination")
}

test_deny_egress_on_every_port if {
	refused(add_egress({"to": [{"podSelector": {"matchLabels": {"cnpg.io/cluster": "lanekeeper-db"}}}]}), "every port")
}

test_deny_egress_missing_dns if {
	refused(netpol([{"op": "remove", "path": "/spec/egress/0"}]), "exactly")
}

test_deny_egress_missing_postgres if {
	refused(netpol([{"op": "remove", "path": "/spec/egress/1"}]), "exactly")
}

test_deny_egress_extra_port_to_postgres if {
	refused(netpol([{"op": "add", "path": "/spec/egress/1/ports/-", "value": tcp(22)}]), "exactly")
}

test_deny_egress_to_postgres_in_another_namespace if {
	planted := add_egress({"to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "other"}}, "podSelector": {"matchLabels": {"cnpg.io/cluster": "lanekeeper-db"}}}], "ports": [tcp(5432)]})
	refused(planted, "unrecognised")
}

test_deny_egress_to_the_wider_link_local_range if {
	refused(add_egress({"to": [{"ipBlock": {"cidr": "169.254.0.0/16"}}], "ports": [tcp(80)]}), "unrecognised")
}

test_otel_egress_is_allowed_when_configured if {
	otel := {
		"to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "monitoring"}}, "podSelector": {"matchLabels": {"app.kubernetes.io/name": "otel-collector"}}}],
		"ports": [tcp(4317)],
	}
	count(deny) == 0 with input as add_egress(otel)
}

test_deny_otel_egress_on_another_port if {
	otel := {
		"to": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "monitoring"}}, "podSelector": {"matchLabels": {"app.kubernetes.io/name": "otel-collector"}}}],
		"ports": [tcp(9999)],
	}
	refused(add_egress(otel), "exactly")
}

# --- egress to the external hosts (S18): names, port 443

test_external_hosts_exact if {
	count(deny) == 0 with input as fixtures.hub
}

test_deny_external_wildcard_host if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [{"op": "add", "path": "/spec/egress/0/matches/-", "value": {"pattern": "*.googleapis.com"}}])
	refused(planted, "wildcard")
}

test_deny_external_extra_host if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [{"op": "add", "path": "/spec/egress/0/matches/-", "value": {"pattern": "example.org"}}])
	refused(planted, "example.org")
}

test_deny_external_missing_host if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [{"op": "remove", "path": "/spec/egress/0/matches/0"}])
	refused(planted, "login.microsoftonline.com")
}

# The audit-checkpoint writer (crates/hub-identity/src/audit/bucket.rs, GcsCheckpointBucket::HOST) puts each checkpoint in the locked
# bucket over storage.googleapis.com (S9). Without the name, default deny blocks the second copy of the audit chain.
test_deny_external_missing_audit_bucket_host if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [{"op": "remove", "path": "/spec/egress/0/matches/6"}])
	refused(planted, "storage.googleapis.com")
}

test_deny_external_port_other_than_443 if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [{"op": "add", "path": "/spec/egress/0/ports/-", "value": tcp(80)}])
	refused(planted, "443")
}

test_deny_external_empty_host if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [{"op": "add", "path": "/spec/egress/0/matches/-", "value": {"pattern": ""}}])
	refused(planted, "empty external host")
}

test_deny_external_rule_without_ports if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [{"op": "remove", "path": "/spec/egress/0/ports"}])
	refused(planted, "no ports")
}

test_deny_external_policy_selecting_every_pod if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [{"op": "replace", "path": "/spec/podSelector", "value": {}}])
	refused(planted, "selects")
}

# The Teams webhook host is not known yet (Q9). When it is, it is named in an annotation and in the pattern, and nowhere else.
test_teams_host_is_allowed_when_named if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [
		{"op": "add", "path": "/metadata/annotations", "value": {"lanekeeper.io/teams-webhook-host": "contoso.webhook.office.com"}},
		{"op": "add", "path": "/spec/egress/0/matches/-", "value": {"pattern": "contoso.webhook.office.com"}},
	])
	count(deny) == 0 with input as planted
}

test_deny_teams_host_that_is_not_the_annotated_one if {
	planted := fixtures.patch("FQDNNetworkPolicy", "lanekeeper-hub-external", [
		{"op": "add", "path": "/metadata/annotations", "value": {"lanekeeper.io/teams-webhook-host": "contoso.webhook.office.com"}},
		{"op": "add", "path": "/spec/egress/0/matches/-", "value": {"pattern": "evil.example.net"}},
	])
	refused(planted, "evil.example.net")
}

# Without the FQDN policy (a cluster that does not have it, plan 09 A11) the external egress is 443 to everything outside the
# private and link-local ranges, which is weaker: it needs the annotation, and it warns.
test_weaker_egress_is_allowed_with_a_warning if {
	planted := weaker_with(weaker_rule)
	count(deny) == 0 with input as planted
	warned(planted, "weaker")
}

test_deny_weaker_egress_without_the_annotation if {
	planted := fixtures.patch("NetworkPolicy", "lanekeeper-hub", [{"op": "add", "path": "/spec/egress/-", "value": weaker_rule}])
	refused(planted, "annotation")
}

test_deny_weaker_egress_that_reaches_private_ranges if {
	rule := {"to": [{"ipBlock": {"cidr": "0.0.0.0/0", "except": ["10.0.0.0/8"]}}], "ports": [tcp(443)]}
	refused(weaker_with(rule), "private")
}

test_deny_weaker_egress_on_port_80 if {
	rule := {"to": weaker_rule.to, "ports": [tcp(80)]}
	refused(weaker_with(rule), "exactly")
}

test_deny_no_way_to_reach_the_external_hosts if {
	refused(fixtures.without("FQDNNetworkPolicy"), "external")
}

# --- ingress: the four listeners from their own callers, nothing else (S18)

test_hub_ingress_exact if {
	count(deny) == 0 with input as fixtures.hub
}

test_deny_ingress_from_the_internet if {
	refused(netpol([{"op": "replace", "path": "/spec/ingress/1/from/0/ipBlock/cidr", "value": "0.0.0.0/0"}]), "too wide")
}

test_deny_ingress_from_a_wide_range if {
	refused(netpol([{"op": "replace", "path": "/spec/ingress/1/from/0/ipBlock/cidr", "value": "10.0.0.0/8"}]), "too wide")
}

test_deny_empty_source_ranges_in_the_agent_ingress if {
	refused(netpol([{"op": "replace", "path": "/spec/ingress/1/from", "value": []}]), "names no callers")
}

test_deny_ingress_without_from if {
	refused(netpol([{"op": "remove", "path": "/spec/ingress/1/from"}]), "names no callers")
}

test_deny_ingress_rule_with_two_ports if {
	refused(netpol([{"op": "add", "path": "/spec/ingress/1/ports/-", "value": tcp(22)}]), "one port")
}

test_deny_extra_ingress_port if {
	refused(add_ingress({"from": [{"ipBlock": {"cidr": "198.51.100.0/24"}}], "ports": [tcp(22)]}), "exactly")
}

test_deny_replica_port_from_other_pods if {
	refused(netpol([{"op": "replace", "path": "/spec/ingress/2/from/0/podSelector/matchLabels", "value": {"app": "other"}}]), "exactly")
}

test_deny_ops_port_from_another_namespace if {
	refused(netpol([{"op": "replace", "path": "/spec/ingress/3/from/0/namespaceSelector/matchLabels", "value": {"kubernetes.io/metadata.name": "default"}}]), "exactly")
}

# The load balancer's health checks (plan 09 A10) come from Google's two ranges and may ask the ops port: those ranges and no others.
test_deny_ops_port_from_other_ranges if {
	refused(netpol([{"op": "replace", "path": "/spec/ingress/4/from/0/ipBlock/cidr", "value": "198.51.100.0/24"}]), "health-check")
}

test_deny_missing_agent_ingress if {
	refused(netpol([{"op": "remove", "path": "/spec/ingress/1"}]), "exactly")
}

test_deny_agent_ingress_that_the_load_balancer_does_not_have if {
	refused(netpol([{"op": "add", "path": "/spec/ingress/1/from/-", "value": {"ipBlock": {"cidr": "192.0.2.0/24"}}}]), "load balancer")
}

# --- the agent load balancer (S18, Q1)

test_deny_empty_source_ranges if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/loadBalancerSourceRanges", "value": []}])
	refused(planted, "loadBalancerSourceRanges")
}

test_deny_missing_source_ranges if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "remove", "path": "/spec/loadBalancerSourceRanges"}])
	refused(planted, "loadBalancerSourceRanges")
}

test_deny_world_source_range if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/loadBalancerSourceRanges", "value": ["0.0.0.0/0"]}])
	refused(planted, "too wide")
}

test_deny_world_source_range_ipv6 if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/loadBalancerSourceRanges", "value": ["::/0"]}])
	refused(planted, "too wide")
}

test_deny_wide_source_range if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/loadBalancerSourceRanges", "value": ["198.51.100.0/24", "10.0.0.0/8"]}])
	refused(planted, "too wide")
}

test_deny_agent_load_balancer_that_hides_the_client_address if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/externalTrafficPolicy", "value": "Cluster"}])
	refused(planted, "externalTrafficPolicy")
}

test_deny_agent_load_balancer_with_another_port if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "add", "path": "/spec/ports/-", "value": {"name": "web", "port": 8443, "targetPort": "web"}}])
	refused(planted, "must expose port 443 and nothing else")
}

# B7: the agents dial the hub on 443 (crates/agent/HARDENING.md) and the hub's pods listen on 8444 (crates/hub/HARDENING.md). The
# Service is the join between them: it listens on 443 and forwards to the container port named agent. The pods' NetworkPolicy
# still admits 8444, because it sees the traffic after the Service has forwarded it.
test_deny_agent_load_balancer_on_the_container_port if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/ports/0/port", "value": 8444}])
	refused(planted, "must expose port 443 and nothing else")
}

test_deny_agent_load_balancer_on_another_port if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/ports/0/port", "value": 22}])
	refused(planted, "must expose port 443 and nothing else")
}

test_deny_agent_load_balancer_that_forwards_to_a_number if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/ports/0/targetPort", "value": 8444}])
	refused(planted, "must forward to the container port named agent")
}

test_deny_agent_load_balancer_that_forwards_to_another_named_port if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "replace", "path": "/spec/ports/0/targetPort", "value": "web"}])
	refused(planted, "must forward to the container port named agent")
}

# Without a targetPort Kubernetes forwards to the same number, 443, where nothing listens.
test_deny_agent_load_balancer_without_a_target_port if {
	planted := fixtures.patch("Service", "lanekeeper-hub-agent", [{"op": "remove", "path": "/spec/ports/0/targetPort"}])
	refused(planted, "must forward to the container port named agent")
}

# The pods' ingress is on the container port, and the load balancer's change did not move it.
test_deny_agent_ingress_on_the_load_balancer_port if {
	refused(netpol([{"op": "replace", "path": "/spec/ingress/1/ports/0/port", "value": 443}]), "exactly")
}

# The web port is behind the Gateway and Cloud Armor; a second public door would go around both.
test_deny_web_service_as_a_load_balancer if {
	planted := fixtures.patch("Service", "lanekeeper-hub-web", [{"op": "replace", "path": "/spec/type", "value": "LoadBalancer"}])
	refused(planted, "Service/lanekeeper-hub-web")
}

test_deny_node_port_service if {
	planted := fixtures.patch("Service", "lanekeeper-hub-web", [{"op": "replace", "path": "/spec/type", "value": "NodePort"}])
	refused(planted, "NodePort")
}

# --- the user entry point: a Gateway with TLS, Cloud Armor and a health check (S18, Q2)

test_deny_gateway_http_listener if {
	planted := fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "replace", "path": "/spec/listeners/0/protocol", "value": "HTTP"}])
	refused(planted, "only HTTPS listeners")
}

test_deny_gateway_listener_on_another_port if {
	refused(fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "replace", "path": "/spec/listeners/0/port", "value": 8443}]), "443")
}

test_deny_gateway_passthrough if {
	refused(fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "replace", "path": "/spec/listeners/0/tls/mode", "value": "Passthrough"}]), "Terminate")
}

test_deny_gateway_without_tls if {
	refused(fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "remove", "path": "/spec/listeners/0/tls"}]), "Terminate")
}

test_deny_gateway_without_a_certificate if {
	refused(fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "remove", "path": "/spec/listeners/0/tls/options"}]), "certificate")
}

test_deny_gateway_certificate_in_a_secret if {
	planted := fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "add", "path": "/spec/listeners/0/tls/certificateRefs", "value": [{"name": "tls"}]}])
	refused(planted, "Secret")
}

test_deny_gateway_without_hostname if {
	refused(fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "remove", "path": "/spec/listeners/0/hostname"}]), "hostname")
}

test_deny_gateway_wildcard_hostname if {
	refused(fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "replace", "path": "/spec/listeners/0/hostname", "value": "*.example.com"}]), "hostname")
}

test_deny_gateway_that_is_internal if {
	refused(fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "replace", "path": "/spec/gatewayClassName", "value": "gke-l7-rilb"}]), "gke-l7-global-external-managed")
}

test_deny_gateway_that_takes_routes_from_all_namespaces if {
	refused(fixtures.patch("Gateway", "lanekeeper-hub", [{"op": "replace", "path": "/spec/listeners/0/allowedRoutes/namespaces/from", "value": "All"}]), "namespaces")
}

test_deny_backend_policy_without_cloud_armor if {
	planted := fixtures.patch("GCPBackendPolicy", "lanekeeper-hub-web", [{"op": "remove", "path": "/spec/default/securityPolicy"}])
	refused(planted, "Cloud Armor")
}

test_deny_missing_backend_policy if {
	refused(fixtures.without("GCPBackendPolicy"), "GCPBackendPolicy")
}

test_deny_backend_policy_for_another_service if {
	planted := fixtures.patch("GCPBackendPolicy", "lanekeeper-hub-web", [{"op": "replace", "path": "/spec/targetRef/name", "value": "other"}])
	refused(planted, "lanekeeper-hub-web")
}

test_deny_health_check_on_the_wrong_path if {
	planted := fixtures.patch("HealthCheckPolicy", "lanekeeper-hub-web", [{"op": "replace", "path": "/spec/default/config/httpHealthCheck/requestPath", "value": "/healthz"}])
	refused(planted, "/readyz")
}

test_deny_health_check_on_the_wrong_port if {
	planted := fixtures.patch("HealthCheckPolicy", "lanekeeper-hub-web", [{"op": "replace", "path": "/spec/default/config/httpHealthCheck/port", "value": 8443}])
	refused(planted, "9090")
}

test_deny_missing_health_check if {
	refused(fixtures.without("HealthCheckPolicy"), "HealthCheckPolicy")
}

test_warn_without_a_gateway if {
	planted := fixtures.without("Gateway")
	warned(planted, "no Gateway")
}

# --- GitHub's webhooks: the /hooks/github route exists (P2, Q28)

test_hooks_route_exists if {
	count(deny) == 0 with input as fixtures.hub
}

test_deny_route_missing if {
	refused(fixtures.without("HTTPRoute"), "HTTPRoute")
}

test_deny_route_without_the_hooks_path if {
	planted := fixtures.patch("HTTPRoute", "lanekeeper-hub", [{"op": "remove", "path": "/spec/rules/0"}])
	refused(planted, "/hooks/github")
}

test_deny_hooks_path_as_a_prefix if {
	planted := fixtures.patch("HTTPRoute", "lanekeeper-hub", [{"op": "replace", "path": "/spec/rules/0/matches/0/path/type", "value": "PathPrefix"}])
	refused(planted, "/hooks/github")
}

test_deny_route_to_another_backend if {
	planted := fixtures.patch("HTTPRoute", "lanekeeper-hub", [{"op": "replace", "path": "/spec/rules/0/backendRefs/0/name", "value": "other"}])
	refused(planted, "backend")
}

# --- Cloud Armor (S18): users' ranges, GitHub's ranges on the hooks path only, and a default deny

test_cloud_armor_good_passes if {
	count(deny) == 0 with input as fixtures.gcp with data.cloudArmor as {"githubHookRanges": fixtures.github_hook_ranges}
}

# GitHub's ranges are in the rule for the exact path and in no other: moving one into the users' rule is refused.
test_hooks_ranges_only_on_hooks_path if {
	count(deny) == 0 with input as fixtures.gcp with data.cloudArmor as {"githubHookRanges": fixtures.github_hook_ranges}
	rule := json.patch(fixtures.armor_rule_users, [{"op": "add", "path": "/match/config/srcIpRanges/-", "value": "2001:db8::/32"}])
	refused_gcp(fixtures.gcp_with_rules([rule, fixtures.armor_rule_hooks, fixtures.armor_rule_default]), "outside the hooks path")
}

test_deny_hook_range_in_a_users_rule if {
	rule := json.patch(fixtures.armor_rule_users, [{"op": "add", "path": "/match/config/srcIpRanges/-", "value": "203.0.113.0/24"}])
	refused_gcp(fixtures.gcp_with_rules([rule, fixtures.armor_rule_hooks, fixtures.armor_rule_default]), "hooks path")
}

test_deny_hooks_rule_without_the_path if {
	rule := hooks_rule_text("inIpRange(origin.ip, '203.0.113.0/24') || inIpRange(origin.ip, '2001:db8::/32')")
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, rule, fixtures.armor_rule_default]), "hooks path")
}

test_deny_hooks_rule_with_the_path_as_an_alternative if {
	rule := hooks_rule_text("request.path == '/hooks/github' || inIpRange(origin.ip, '203.0.113.0/24')")
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, rule, fixtures.armor_rule_default]), "hooks path")
}

test_deny_hooks_rule_with_a_path_prefix if {
	rule := hooks_rule_text("request.path.startsWith('/hooks') && (inIpRange(origin.ip, '203.0.113.0/24') || inIpRange(origin.ip, '2001:db8::/32'))")
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, rule, fixtures.armor_rule_default]), "hooks path")
}

test_deny_hooks_rule_with_another_condition if {
	rule := hooks_rule_text("request.path == '/hooks/github' && (inIpRange(origin.ip, '203.0.113.0/24') || true)")
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, rule, fixtures.armor_rule_default]), "hooks path")
}

test_deny_stale_hook_ranges if {
	rule := hooks_rule_text("request.path == '/hooks/github' && (inIpRange(origin.ip, '203.0.113.0/24'))")
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, rule, fixtures.armor_rule_default]), "2001:db8::/32")
}

test_deny_a_range_that_is_not_githubs if {
	rule := hooks_rule_text("request.path == '/hooks/github' && (inIpRange(origin.ip, '203.0.113.0/24') || inIpRange(origin.ip, '2001:db8::/32') || inIpRange(origin.ip, '192.0.2.0/24'))")
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, rule, fixtures.armor_rule_default]), "192.0.2.0/24")
}

test_deny_hooks_data_missing if {
	msgs := deny with input as fixtures.gcp
	some msg in msgs
	contains(msg, "hook ranges")
}

# The unresolved file: an empty list of GitHub's ranges (plan 09 blockers, B5).
test_deny_hook_ranges_empty if {
	msgs := deny with input as fixtures.gcp with data.cloudArmor as {"githubHookRanges": []}
	some msg in msgs
	contains(msg, "hook ranges are empty")
}

test_deny_default_rule_missing if {
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, fixtures.armor_rule_hooks]), "default")
}

test_deny_default_rule_that_allows if {
	rule := json.patch(fixtures.armor_rule_default, [{"op": "replace", "path": "/action", "value": "allow"}])
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, fixtures.armor_rule_hooks, rule]), "default")
}

test_deny_an_allow_rule_for_everyone if {
	rule := {"action": "allow", "priority": 3000, "match": {"versionedExpr": "SRC_IPS_V1", "config": {"srcIpRanges": ["*"]}}}
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, fixtures.armor_rule_hooks, rule, fixtures.armor_rule_default]), "too wide")
}

test_deny_users_rule_for_a_wide_range if {
	rule := json.patch(fixtures.armor_rule_users, [{"op": "replace", "path": "/match/config/srcIpRanges", "value": ["10.0.0.0/8"]}])
	refused_gcp(fixtures.gcp_with_rules([rule, fixtures.armor_rule_hooks, fixtures.armor_rule_default]), "too wide")
}

# Helm renders an emptied list as null.
test_deny_users_rule_with_null_ranges if {
	rule := json.patch(fixtures.armor_rule_users, [{"op": "replace", "path": "/match/config/srcIpRanges", "value": null}])
	refused_gcp(fixtures.gcp_with_rules([rule, fixtures.armor_rule_hooks, fixtures.armor_rule_default]), "no source ranges")
}

test_deny_users_rule_with_an_empty_list if {
	rule := json.patch(fixtures.armor_rule_users, [{"op": "replace", "path": "/match/config/srcIpRanges", "value": []}])
	refused_gcp(fixtures.gcp_with_rules([rule, fixtures.armor_rule_hooks, fixtures.armor_rule_default]), "no source ranges")
}

test_deny_no_users_rule if {
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_hooks, fixtures.armor_rule_default]), "users")
}

test_deny_rule_in_preview if {
	rule := json.patch(fixtures.armor_rule_default, [{"op": "add", "path": "/preview", "value": true}])
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, fixtures.armor_rule_hooks, rule]), "preview")
}

test_deny_two_rules_with_one_priority if {
	rule := json.patch(fixtures.armor_rule_hooks, [{"op": "replace", "path": "/priority", "value": 1000}])
	refused_gcp(fixtures.gcp_with_rules([fixtures.armor_rule_users, rule, fixtures.armor_rule_default]), "priority")
}

# --- the agent (S17, S18): default deny, and exactly the five destinations of crates/agent/HARDENING.md

agent_netpol(ops) := fixtures.agent_patch("NetworkPolicy", "lanekeeper-agent", ops)

agent_add_egress(rule) := agent_netpol([{"op": "add", "path": "/spec/egress/-", "value": rule}])

agent_add_ingress(rule) := agent_netpol([{"op": "add", "path": "/spec/ingress/-", "value": rule}])

test_agent_egress_exact if {
	count(deny) == 0 with input as fixtures.agent
	count(warn) == 0 with input as fixtures.agent
}

test_deny_agent_without_a_network_policy if {
	refused(fixtures.agent_without("NetworkPolicy"), "NetworkPolicy/lanekeeper-agent is missing")
}

test_deny_agent_policy_without_default_deny_egress if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/policyTypes", "value": ["Ingress"]}]), "not default deny")
}

test_deny_agent_policy_without_default_deny_ingress if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/policyTypes", "value": ["Egress"]}]), "not default deny")
}

test_deny_agent_policy_that_selects_every_pod if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/podSelector", "value": {}}]), "selects")
}

test_deny_agent_policy_that_selects_other_pods if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/podSelector/matchLabels", "value": {"app.kubernetes.io/name": "lanekeeper-hub"}}]), "selects")
}

test_deny_agent_egress_to_anywhere if {
	refused(agent_add_egress({"to": [{"ipBlock": {"cidr": "0.0.0.0/0"}}], "ports": [tcp(443)]}), "unrecognised")
}

test_deny_agent_egress_to_every_destination if {
	refused(agent_add_egress({"ports": [tcp(443)]}), "every destination")
}

test_deny_agent_egress_on_every_port if {
	refused(agent_add_egress({"to": [{"ipBlock": {"cidr": "203.0.113.99/32"}}]}), "every port")
}

# The hub's and the API server's addresses are single hosts or small blocks: a /24 at most.
test_deny_agent_egress_to_a_wide_hub_block if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/egress/0/to/0/ipBlock/cidr", "value": "203.0.0.0/16"}]), "unrecognised")
}

test_deny_agent_egress_to_a_private_range if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/egress/1/to/0/ipBlock/cidr", "value": "10.0.0.0/8"}]), "unrecognised")
}

test_agent_egress_to_a_slash_24_is_allowed if {
	count(deny) == 0 with input as agent_netpol([{"op": "replace", "path": "/spec/egress/1/to/0/ipBlock/cidr", "value": "198.51.100.0/24"}])
}

# The hub may have several addresses: the rule is still one rule, for the hub.
test_agent_egress_to_several_hub_addresses_is_allowed if {
	planted := agent_netpol([{"op": "add", "path": "/spec/egress/0/to/-", "value": {"ipBlock": {"cidr": "203.0.113.11/32"}}}, {"op": "add", "path": "/spec/egress/1/to/-", "value": {"ipBlock": {"cidr": "198.51.100.3/32"}}}])
	count(deny) == 0 with input as planted
}

test_deny_agent_egress_block_with_exceptions if {
	refused(agent_netpol([{"op": "add", "path": "/spec/egress/0/to/0/ipBlock/except", "value": ["203.0.113.1/32"]}]), "unrecognised")
}

# Two rules for the two endpoints, the hub and the API server (HARDENING.md): a third one is a destination the agent does not have.
test_deny_agent_egress_to_a_third_endpoint if {
	refused(agent_add_egress({"to": [{"ipBlock": {"cidr": "192.0.2.77/32"}}], "ports": [tcp(443)]}), "exactly")
}

test_deny_agent_egress_with_a_missing_endpoint if {
	refused(agent_netpol([{"op": "remove", "path": "/spec/egress/1"}]), "exactly")
}

test_deny_agent_egress_to_the_hub_on_another_port if {
	refused(agent_netpol([{"op": "add", "path": "/spec/egress/0/ports/-", "value": tcp(8444)}]), "exactly")
}

test_deny_agent_egress_to_the_hub_on_ssh if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/egress/0/ports/0/port", "value": 22}]), "exactly")
}

test_deny_agent_egress_missing_config_server if {
	refused(agent_netpol([{"op": "remove", "path": "/spec/egress/2"}]), "exactly")
}

test_deny_agent_egress_to_the_config_server_on_another_port if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/egress/2/ports/0/port", "value": 8080}]), "exactly")
}

test_deny_agent_egress_to_the_config_server_in_another_namespace if {
	planted := agent_netpol([{"op": "add", "path": "/spec/egress/2/to/0/namespaceSelector", "value": {"matchLabels": {"kubernetes.io/metadata.name": "other"}}}])
	refused(planted, "unrecognised")
}

test_deny_agent_egress_to_other_pods_on_the_config_server_port if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/egress/2/to/0/podSelector/matchLabels", "value": {"app": "something-else"}}]), "unrecognised")
}

test_deny_agent_egress_missing_metadata_server if {
	refused(agent_netpol([{"op": "remove", "path": "/spec/egress/3"}]), "exactly")
}

test_deny_agent_egress_to_the_metadata_range if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/egress/3/to/0/ipBlock/cidr", "value": "169.254.0.0/16"}]), "unrecognised")
}

test_deny_agent_egress_missing_dns if {
	refused(agent_netpol([{"op": "remove", "path": "/spec/egress/4"}]), "exactly")
}

test_deny_agent_egress_to_dns_in_every_namespace if {
	refused(agent_netpol([{"op": "remove", "path": "/spec/egress/4/to/0/namespaceSelector"}]), "unrecognised")
}

# An FQDN policy beside the agent's own would let it reach hosts by name that the NetworkPolicy does not list.
test_deny_agent_with_an_fqdn_policy if {
	refused(fixtures.agent_with_doc(fixtures.fqdnpolicy), "FQDNNetworkPolicy")
}

# --- the agent: who may reach the health port

test_deny_agent_ingress_from_everywhere if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/ingress", "value": [{"ports": [tcp(9090)]}]}]), "names no callers")
}

test_deny_agent_ingress_from_another_namespace if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/ingress/0/from/0/namespaceSelector/matchLabels", "value": {"kubernetes.io/metadata.name": "other"}}]), "monitoring")
}

test_deny_agent_ingress_on_another_port if {
	refused(agent_netpol([{"op": "replace", "path": "/spec/ingress/0/ports/0/port", "value": 8080}]), "9090")
}

test_deny_agent_ingress_from_a_block if {
	refused(agent_add_ingress({"from": [{"ipBlock": {"cidr": "198.51.100.0/24"}}], "ports": [tcp(9090)]}), "monitoring")
}

test_deny_agent_ingress_from_the_cluster if {
	refused(agent_add_ingress({"from": [{"podSelector": {}}], "ports": [tcp(9090)]}), "monitoring")
}

# The agent has no door of its own: its only Service-shaped surface would be a load balancer, and the hub's is the one that exists.
test_deny_agent_load_balancer if {
	lb := {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "lanekeeper-agent", "namespace": "lanekeeper"}, "spec": {"type": "LoadBalancer", "ports": [{"port": 9090}]}}
	refused(fixtures.agent_with_doc(lb), "LoadBalancer")
}

# --- the hub image's Jobs: their own pods, their own policy (crates/hub/HARDENING.md; 09/fix, J3) ---------------------------------------------
#
# The Jobs' pods do not carry the hub's label, so the hub's policy does not select them (it would give them the other replicas, the metadata
# server and the external hosts). Their own policy admits nothing in, and lets out the names (kube-dns) and the database, and nothing else.

jobs_policy(ops) := fixtures.patch("NetworkPolicy", fixtures.jobs_name, ops)

test_the_jobs_policy_passes if {
	count(deny) == 0 with input as fixtures.hub
}

test_deny_jobs_without_their_policy if {
	refused(fixtures.without_named("NetworkPolicy", fixtures.jobs_name), "NetworkPolicy/lanekeeper-hub-jobs is missing")
}

test_deny_jobs_policy_without_egress_deny if {
	refused(jobs_policy([{"op": "replace", "path": "/spec/policyTypes", "value": ["Ingress"]}]), "lanekeeper-hub-jobs: not default deny")
}

test_deny_jobs_policy_without_ingress_deny if {
	refused(jobs_policy([{"op": "replace", "path": "/spec/policyTypes", "value": ["Egress"]}]), "lanekeeper-hub-jobs: not default deny")
}

test_deny_jobs_policy_that_selects_every_pod if {
	refused(jobs_policy([{"op": "replace", "path": "/spec/podSelector", "value": {}}]), "lanekeeper-hub-jobs: it selects pods other than the Jobs'")
}

test_deny_jobs_policy_that_selects_the_hub if {
	refused(jobs_policy([{"op": "replace", "path": "/spec/podSelector/matchLabels", "value": fixtures.selector}]), "lanekeeper-hub-jobs: it selects pods other than the Jobs'")
}

test_deny_jobs_policy_with_ingress if {
	refused(jobs_policy([{"op": "add", "path": "/spec/ingress", "value": [{"from": [{"podSelector": {}}], "ports": [tcp(8443)]}]}]), "admits nothing in")
}

test_deny_jobs_policy_with_an_empty_ingress_rule if {
	refused(jobs_policy([{"op": "add", "path": "/spec/ingress", "value": [{}]}]), "admits nothing in")
}

test_deny_jobs_policy_to_the_other_replicas if {
	refused(jobs_policy([{"op": "add", "path": "/spec/egress/-", "value": {"to": [{"podSelector": {"matchLabels": fixtures.selector}}], "ports": [tcp(8445)]}}]), "lanekeeper-hub-jobs: egress must be exactly")
}

test_deny_jobs_policy_to_the_metadata_server if {
	refused(jobs_policy([{"op": "add", "path": "/spec/egress/-", "value": {"to": [{"ipBlock": {"cidr": "169.254.169.254/32"}}], "ports": [tcp(80)]}}]), "lanekeeper-hub-jobs: egress must be exactly")
}

test_deny_jobs_policy_to_the_internet if {
	refused(jobs_policy([{"op": "add", "path": "/spec/egress/-", "value": weaker_rule}]), "lanekeeper-hub-jobs")
}

test_deny_jobs_policy_to_every_destination if {
	refused(jobs_policy([{"op": "add", "path": "/spec/egress/-", "value": {"ports": [tcp(5432)]}}]), "allows every destination")
}

test_deny_jobs_policy_on_every_port if {
	refused(jobs_policy([{"op": "add", "path": "/spec/egress/-", "value": {"to": [{"podSelector": {"matchLabels": {"cnpg.io/cluster": "lanekeeper-db"}}}]}}]), "allows every port")
}

test_deny_jobs_policy_to_the_database_on_another_port if {
	refused(jobs_policy([{"op": "replace", "path": "/spec/egress/1/ports/0/port", "value": 8000}]), "lanekeeper-hub-jobs: egress must be exactly")
}

test_deny_jobs_policy_to_the_database_of_every_namespace if {
	refused(jobs_policy([{"op": "add", "path": "/spec/egress/1/to/0/namespaceSelector", "value": {}}]), "unrecognised egress destination")
}

test_deny_jobs_policy_without_dns if {
	refused(jobs_policy([{"op": "remove", "path": "/spec/egress/0"}]), "lanekeeper-hub-jobs: egress must be exactly")
}

test_deny_jobs_policy_without_the_database if {
	refused(jobs_policy([{"op": "remove", "path": "/spec/egress/1"}]), "lanekeeper-hub-jobs: egress must be exactly")
}

# The hub's own policy is not widened for the Jobs: it keeps its ingress and its egress, and still selects only the hub.
test_the_hubs_policy_does_not_admit_the_jobs_pods if {
	planted := netpol([{"op": "replace", "path": "/spec/ingress/2/from/0/podSelector/matchLabels", "value": fixtures.jobs_selector}])
	refused(planted, "ingress must be exactly")
}
