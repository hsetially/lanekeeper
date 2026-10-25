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
weaker_input := [{"path": "rendered/hub.yaml", "contents": [d | some d in fixtures.hub_docs; d.kind != "FQDNNetworkPolicy"; d.kind != "NetworkPolicy"]}, fixtures.budgets]

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
	refused(planted, "8444")
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
