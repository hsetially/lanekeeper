# Network policy (09/T3; S18, P2). Run with `conftest test --combine --namespace network` over a chart's rendered output (the
# gcp chart's also needs `--data deploy/values/hub/github-hook-ranges.yaml`). The hub's pods deny by default and admit exactly
# what crates/hub/HARDENING.md lists, both load balancers have source-IP allowlists, GitHub's hook ranges are allowed on the
# webhook path and nowhere else, and Cloud Armor ends in a deny.
package network

import rego.v1

import data.lk

hub_selector := {"app.kubernetes.io/name": "lanekeeper-hub"}

# The ranges that Google's load balancers and health checks come from (plan 09 A10).
google_ranges := {"35.191.0.0/16", "130.211.0.0/22"}

agent_service := "lanekeeper-hub-agent"

web_service := "lanekeeper-hub-web"

hub_policies := lk.named("NetworkPolicy", lk.hub_name)

# --- default deny -------------------------------------------------------------------------------------------------------

deny contains msg if {
	count(lk.hub_statefulsets) > 0
	count(hub_policies) == 0
	msg := "NetworkPolicy/lanekeeper-hub is missing: nothing denies by default for the hub's pods (S18)"
}

deny contains msg if {
	some p in hub_policies
	missing := {"Ingress", "Egress"} - {t | some t in object.get(p.spec, "policyTypes", [])}
	count(missing) > 0
	msg := sprintf("%s: not default deny: policyTypes must list Ingress and Egress, %v is missing (S18)", [lk.who(p), sort(missing)])
}

deny contains msg if {
	some p in hub_policies
	object.get(p.spec.podSelector, "matchLabels", {}) != hub_selector
	msg := sprintf("%s: it selects pods other than the hub's; an empty selector is every pod (S18)", [lk.who(p)])
}

# --- egress: exactly the destinations of HARDENING.md --------------------------------------------------------------------

# Without the FQDN policy, external hosts are reached by 443 to everything but these ranges (plan 09 A11). Weaker, so it is
# marked and warned about.
private_ranges := {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16"}

destination(d) := "metadata" if {
	d.ipBlock.cidr == "169.254.169.254/32"
	not d.ipBlock.except
}

destination(d) := "dns" if {
	d.namespaceSelector.matchLabels == {"kubernetes.io/metadata.name": "kube-system"}
	d.podSelector.matchLabels == {"k8s-app": "kube-dns"}
}

destination(d) := "postgres" if {
	d.podSelector.matchLabels == {"cnpg.io/cluster": "lanekeeper-db"}
	not d.namespaceSelector
}

destination(d) := "replicas" if {
	d.podSelector.matchLabels == hub_selector
	not d.namespaceSelector
}

destination(d) := "otel" if {
	d.namespaceSelector.matchLabels == {"kubernetes.io/metadata.name": "monitoring"}
	d.podSelector.matchLabels == {"app.kubernetes.io/name": "otel-collector"}
}

destination(d) := "external" if {
	d.ipBlock.cidr == "0.0.0.0/0"
	{e | some e in d.ipBlock.except} == private_ranges
}

egress_rules(p) := object.get(p.spec, "egress", [])

deny contains msg if {
	some p in hub_policies
	some rule in egress_rules(p)
	count(object.get(rule, "to", [])) == 0
	msg := sprintf("%s: an egress rule allows every destination (it has no `to`) (S18)", [lk.who(p)])
}

deny contains msg if {
	some p in hub_policies
	some rule in egress_rules(p)
	count(object.get(rule, "ports", [])) == 0
	msg := sprintf("%s: an egress rule allows every port (it has no `ports`) (S18)", [lk.who(p)])
}

deny contains msg if {
	some p in hub_policies
	some rule in egress_rules(p)
	some d in object.get(rule, "to", [])
	not destination(d)
	msg := sprintf("%s: an unrecognised egress destination: %v (S18)", [lk.who(p), d])
}

deny contains msg if {
	some p in hub_policies
	some rule in egress_rules(p)
	some d in object.get(rule, "to", [])
	d.ipBlock.cidr == "0.0.0.0/0"
	{e | some e in object.get(d.ipBlock, "except", [])} != private_ranges
	msg := sprintf("%s: egress to 0.0.0.0/0 must exclude the private and link-local ranges %v, and reaches private addresses without them (S18)", [lk.who(p), sort(private_ranges)])
}

egress_pairs(p) := {sprintf("%s %s/%d", [destination(d), object.get(port, "protocol", "TCP"), port.port]) |
	some rule in egress_rules(p)
	some d in object.get(rule, "to", [])
	some port in object.get(rule, "ports", [])
}

base_egress := {"dns UDP/53", "dns TCP/53", "postgres TCP/5432", "replicas TCP/8445", "metadata TCP/80"}

optional_egress := {"otel TCP/4317", "external TCP/443"}

deny contains msg if {
	some p in hub_policies
	egress_pairs(p) - optional_egress != base_egress
	msg := sprintf("%s: egress must be exactly %v, plus optionally %v; found %v (S18)", [lk.who(p), sort(base_egress), sort(optional_egress), sort(egress_pairs(p))])
}

weaker(p) if "external TCP/443" in egress_pairs(p)

deny contains msg if {
	some p in hub_policies
	weaker(p)
	object.get(p.metadata, ["annotations", "lanekeeper.io/weaker-egress"], "") != "true"
	msg := sprintf("%s: egress to external hosts by address range needs the annotation lanekeeper.io/weaker-egress: \"true\" (plan 09 A11)", [lk.who(p)])
}

warn contains msg if {
	some p in hub_policies
	weaker(p)
	msg := sprintf("%s: external egress is weaker: 443 to every address outside the private ranges, because the cluster has no FQDN network policy (plan 09 A11)", [lk.who(p)])
}

# The hub must reach Entra, GitHub and Google: by name, or by the weaker rule. Neither is a hub that cannot start.
deny contains msg if {
	some p in hub_policies
	count(lk.of_kind("FQDNNetworkPolicy")) == 0
	not weaker(p)
	msg := sprintf("%s: there is no way to the external hosts: no FQDNNetworkPolicy and no external egress rule (S18)", [lk.who(p)])
}

# --- egress to the external hosts: names and 443 ---------------------------------------------------------------------------

required_hosts := {
	"login.microsoftonline.com", "github.com", "api.github.com",
	"secretmanager.googleapis.com", "cloudkms.googleapis.com", "www.googleapis.com",
}

# The Teams webhook host is not known yet (Q9). When it is, it is named in this annotation and in a pattern.
allowed_hosts(p) := required_hosts | {h} if {
	h := p.metadata.annotations["lanekeeper.io/teams-webhook-host"]
} else := required_hosts

hosts(p) := {h |
	some rule in object.get(p.spec, "egress", [])
	some m in object.get(rule, "matches", [])
	h := object.get(m, "pattern", object.get(m, "name", ""))
}

deny contains msg if {
	some p in lk.of_kind("FQDNNetworkPolicy")
	some h in hosts(p)
	h == ""
	msg := sprintf("%s: an empty external host (S18)", [lk.who(p)])
}

deny contains msg if {
	some p in lk.of_kind("FQDNNetworkPolicy")
	some h in hosts(p)
	contains(h, "*")
	msg := sprintf("%s: %q is a wildcard host (S18)", [lk.who(p), h])
}

deny contains msg if {
	some p in lk.of_kind("FQDNNetworkPolicy")
	some h in hosts(p)
	not h in allowed_hosts(p)
	msg := sprintf("%s: %q is not an allowed external host (S18)", [lk.who(p), h])
}

deny contains msg if {
	some p in lk.of_kind("FQDNNetworkPolicy")
	some h in required_hosts
	not h in hosts(p)
	msg := sprintf("%s: the host %q is missing; the hub needs it (S18)", [lk.who(p), h])
}

deny contains msg if {
	some p in lk.of_kind("FQDNNetworkPolicy")
	some rule in object.get(p.spec, "egress", [])
	ports := object.get(rule, "ports", [])
	count(ports) == 0
	msg := sprintf("%s: an external egress rule has no ports; it must be 443 (S18)", [lk.who(p)])
}

deny contains msg if {
	some p in lk.of_kind("FQDNNetworkPolicy")
	some rule in object.get(p.spec, "egress", [])
	some port in object.get(rule, "ports", [])
	port.port != 443
	msg := sprintf("%s: external egress on port %v; only 443 is allowed (S18)", [lk.who(p), port.port])
}

deny contains msg if {
	some p in lk.of_kind("FQDNNetworkPolicy")
	object.get(p.spec.podSelector, "matchLabels", {}) != hub_selector
	msg := sprintf("%s: it selects pods other than the hub's; an empty selector is every pod (S18)", [lk.who(p)])
}

# --- ingress: the four listeners from their own callers -----------------------------------------------------------------

ingress_rules(p) := object.get(p.spec, "ingress", [])

caller(from) := "block" if {
	every f in from {
		f.ipBlock.cidr
		not f.ipBlock.except
	}
}

caller(from) := "hub" if {
	every f in from {
		f.podSelector.matchLabels == hub_selector
		not f.namespaceSelector
	}
}

caller(from) := "monitoring" if {
	every f in from {
		f.namespaceSelector.matchLabels == {"kubernetes.io/metadata.name": "monitoring"}
		not f.podSelector
	}
}

deny contains msg if {
	some p in hub_policies
	some rule in ingress_rules(p)
	count(object.get(rule, "from", [])) == 0
	msg := sprintf("%s: an ingress rule names no callers (empty or missing `from` admits everyone) (S18)", [lk.who(p)])
}

deny contains msg if {
	some p in hub_policies
	some rule in ingress_rules(p)
	count(object.get(rule, "ports", [])) != 1
	msg := sprintf("%s: an ingress rule must name exactly one port, so that each listener has its own callers (S18)", [lk.who(p)])
}

deny contains msg if {
	some p in hub_policies
	some rule in ingress_rules(p)
	some f in object.get(rule, "from", [])
	cidr := f.ipBlock.cidr
	lk.too_wide(cidr)
	msg := sprintf("%s: ingress from %s is too wide; it is the internet or close to it (S18)", [lk.who(p), cidr])
}

ingress_pairs(p) := {sprintf("%d %s", [port.port, caller(rule.from)]) |
	some rule in ingress_rules(p)
	count(rule.from) > 0
	count(object.get(rule, "ports", [])) == 1
	some port in rule.ports
}

expected_ingress := {"8443 block", "8444 block", "8445 hub", "9090 monitoring", "9090 block"}

deny contains msg if {
	some p in hub_policies
	ingress_pairs(p) != expected_ingress
	msg := sprintf("%s: ingress must be exactly %v; found %v (S18)", [lk.who(p), sort(expected_ingress), sort(ingress_pairs(p))])
}

# The operations port is for Prometheus and for the load balancer's health checks, which come from Google's two ranges (A10).
deny contains msg if {
	some p in hub_policies
	some rule in ingress_rules(p)
	some port in object.get(rule, "ports", [])
	port.port == 9090
	some f in object.get(rule, "from", [])
	cidr := f.ipBlock.cidr
	not cidr in google_ranges
	msg := sprintf("%s: port 9090 admits %s; only the load balancer's health-check ranges %v may reach it from outside (plan 09 A10)", [lk.who(p), cidr, sort(google_ranges)])
}

agent_ingress_ranges(p) := {f.ipBlock.cidr |
	some rule in ingress_rules(p)
	some port in object.get(rule, "ports", [])
	port.port == 8444
	some f in object.get(rule, "from", [])
}

# The agent port admits the ranges that the agent load balancer admits, and no others.
deny contains msg if {
	some p in hub_policies
	some svc in lk.named("Service", agent_service)
	agent_ingress_ranges(p) != {r | some r in object.get(svc.spec, "loadBalancerSourceRanges", [])}
	msg := sprintf("%s: the agent port admits %v and the load balancer %v; they must be the same ranges (S18, Q1)", [lk.who(p), sort(agent_ingress_ranges(p)), object.get(svc.spec, "loadBalancerSourceRanges", [])])
}

# --- the agent load balancer: TCP pass-through with a source allowlist ----------------------------------------------------

public_services := [s | some s in lk.of_kind("Service"); s.spec.type in {"LoadBalancer", "NodePort"}]

deny contains msg if {
	some s in public_services
	s.metadata.name != agent_service
	msg := sprintf("%s is a %s Service; the only public door besides the Gateway is %s (S18)", [lk.who(s), s.spec.type, agent_service])
}

agent_lbs := [s | some s in public_services; s.metadata.name == agent_service; s.spec.type == "LoadBalancer"]

deny contains msg if {
	some s in agent_lbs
	count(object.get(s.spec, "loadBalancerSourceRanges", [])) == 0
	msg := sprintf("%s: loadBalancerSourceRanges is empty or missing, so the agent port is open to the internet (S18, Q1)", [lk.who(s)])
}

deny contains msg if {
	some s in agent_lbs
	some r in object.get(s.spec, "loadBalancerSourceRanges", [])
	lk.too_wide(r)
	msg := sprintf("%s: loadBalancerSourceRanges has %s, which is too wide (S18)", [lk.who(s), r])
}

deny contains msg if {
	some s in agent_lbs
	object.get(s.spec, "externalTrafficPolicy", "Cluster") != "Local"
	msg := sprintf("%s: externalTrafficPolicy must be Local, so that the allowlist sees the clients' addresses (S18)", [lk.who(s)])
}

deny contains msg if {
	some s in agent_lbs
	{p.port | some p in object.get(s.spec, "ports", [])} != {8444}
	msg := sprintf("%s: it must expose port 8444 and nothing else (HARDENING.md)", [lk.who(s)])
}

# --- the users' entry point: a Gateway with TLS, Cloud Armor and a health check -------------------------------------------

gateways := lk.of_kind("Gateway")

warn contains msg if {
	count(lk.hub_statefulsets) > 0
	count(gateways) == 0
	msg := "there is no Gateway: users and GitHub's webhooks cannot reach the hub, which falls back to polling (P2, Q28); expected only where there is no GKE Gateway, as in kind"
}

deny contains msg if {
	some g in gateways
	g.spec.gatewayClassName != "gke-l7-global-external-managed"
	msg := sprintf("%s: the class must be gke-l7-global-external-managed, the external Application Load Balancer that Cloud Armor attaches to (S18)", [lk.who(g)])
}

deny contains msg if {
	some g in gateways
	some l in object.get(g.spec, "listeners", [])
	l.protocol != "HTTPS"
	msg := sprintf("%s: listener %s is %s; only HTTPS listeners are allowed (S18)", [lk.who(g), l.name, l.protocol])
}

deny contains msg if {
	some g in gateways
	some l in object.get(g.spec, "listeners", [])
	l.port != 443
	msg := sprintf("%s: listener %s is on port %v; HTTPS is on 443 (S18)", [lk.who(g), l.name, l.port])
}

deny contains msg if {
	some g in gateways
	some l in object.get(g.spec, "listeners", [])
	object.get(l, ["tls", "mode"], "") != "Terminate"
	msg := sprintf("%s: listener %s must terminate TLS at the load balancer (tls.mode: Terminate) (S18)", [lk.who(g), l.name])
}

deny contains msg if {
	some g in gateways
	some l in object.get(g.spec, "listeners", [])
	"certificateRefs" in object.keys(object.get(l, "tls", {}))
	msg := sprintf("%s: listener %s takes its certificate from a Kubernetes Secret; use a Google-managed or Certificate Manager certificate (S7)", [lk.who(g), l.name])
}

certificate_options := {"networking.gke.io/pre-shared-certs", "networking.gke.io/cert-manager-certs"}

has_certificate(l) if {
	some option in certificate_options
	object.get(l, ["tls", "options", option], "") != ""
}

deny contains msg if {
	some g in gateways
	some l in object.get(g.spec, "listeners", [])
	not has_certificate(l)
	msg := sprintf("%s: listener %s names no certificate (networking.gke.io/pre-shared-certs or /cert-manager-certs) (S18)", [lk.who(g), l.name])
}

deny contains msg if {
	some g in gateways
	some l in object.get(g.spec, "listeners", [])
	h := object.get(l, "hostname", "")
	h == ""
	msg := sprintf("%s: listener %s has no hostname (S18)", [lk.who(g), l.name])
}

deny contains msg if {
	some g in gateways
	some l in object.get(g.spec, "listeners", [])
	contains(object.get(l, "hostname", ""), "*")
	msg := sprintf("%s: listener %s has a wildcard hostname (S18)", [lk.who(g), l.name])
}

deny contains msg if {
	some g in gateways
	some l in object.get(g.spec, "listeners", [])
	object.get(l, ["allowedRoutes", "namespaces", "from"], "") != "Same"
	msg := sprintf("%s: listener %s takes routes from other namespaces; it must be Same (S18)", [lk.who(g), l.name])
}

# Cloud Armor and the health check are attached to the Service behind the Gateway.
backend_policies := [p | some p in lk.of_kind("GCPBackendPolicy"); p.spec.targetRef.name == web_service]

deny contains msg if {
	count(gateways) > 0
	count(backend_policies) == 0
	msg := sprintf("there is a Gateway but no GCPBackendPolicy for Service %s, so no Cloud Armor policy is attached (S18)", [web_service])
}

deny contains msg if {
	some p in backend_policies
	object.get(p.spec, ["default", "securityPolicy"], "") == ""
	msg := sprintf("%s: it names no Cloud Armor security policy (default.securityPolicy) (S18)", [lk.who(p)])
}

health_policies := [p | some p in lk.of_kind("HealthCheckPolicy"); p.spec.targetRef.name == web_service]

deny contains msg if {
	count(gateways) > 0
	count(health_policies) == 0
	msg := sprintf("there is a Gateway but no HealthCheckPolicy for Service %s: the load balancer would check the serving port, not /readyz (HARDENING.md)", [web_service])
}

deny contains msg if {
	some p in health_policies
	object.get(p.spec, ["default", "config", "httpHealthCheck", "requestPath"], "") != "/readyz"
	msg := sprintf("%s: the health check must ask /readyz (HARDENING.md)", [lk.who(p)])
}

deny contains msg if {
	some p in health_policies
	object.get(p.spec, ["default", "config", "httpHealthCheck", "port"], 0) != 9090
	msg := sprintf("%s: the health check must use the operations port, 9090 (HARDENING.md)", [lk.who(p)])
}

# --- GitHub's webhook path (P2, Q28) ---------------------------------------------------------------------------------------

routes := lk.of_kind("HTTPRoute")

deny contains msg if {
	count(gateways) > 0
	count(routes) == 0
	msg := "there is a Gateway but no HTTPRoute, so nothing is served, and GitHub's webhooks have no path (P2, Q28)"
}

hooks_rule(rule) if {
	some m in rule.matches
	m.path.type == "Exact"
	m.path.value == "/hooks/github"
}

deny contains msg if {
	some r in routes
	not any_hooks_rule(r)
	msg := sprintf("%s: no rule matches the path /hooks/github exactly; GitHub's webhooks have no route (P2, Q28)", [lk.who(r)])
}

any_hooks_rule(r) if {
	some rule in r.spec.rules
	hooks_rule(rule)
}

deny contains msg if {
	some r in routes
	some rule in r.spec.rules
	some b in object.get(rule, "backendRefs", [])
	not b.name == web_service
	msg := sprintf("%s: a rule sends traffic to the backend %q; every rule goes to %s (S18)", [lk.who(r), b.name, web_service])
}

# --- Cloud Armor: users' ranges, GitHub's ranges on the hooks path only, a default deny ----------------------------------

armor_policies := lk.of_kind("ComputeSecurityPolicy")

# GitHub's published hook ranges, written by deploy/tools/github-hook-ranges.mjs into
# deploy/values/hub/github-hook-ranges.yaml and given to conftest with --data.
hook_ranges := data.cloudArmor.githubHookRanges

default_priority := 2147483647

deny contains msg if {
	count(armor_policies) > 0
	not hook_ranges
	msg := "the GitHub hook ranges were not given to the policy (--data deploy/values/hub/github-hook-ranges.yaml), so the hooks rule cannot be checked (Q28)"
}

deny contains msg if {
	count(armor_policies) > 0
	count(hook_ranges) == 0
	msg := "the GitHub hook ranges are empty: run `node deploy/tools/github-hook-ranges.mjs` (Q28)"
}

armor_rules(p) := object.get(p.spec, "rule", [])

expr_text(rule) := rule.match.expr.expression

# The only expression a rule may have: the hooks path, and only GitHub's ranges.
hooks_expression := `^request\.path == '/hooks/github' && \(inIpRange\(origin\.ip, '[0-9a-fA-F:.]+/[0-9]+'\)( \|\| inIpRange\(origin\.ip, '[0-9a-fA-F:.]+/[0-9]+'\))*\)$`

deny contains msg if {
	some p in armor_policies
	some rule in armor_rules(p)
	not regex.match(hooks_expression, expr_text(rule))
	msg := sprintf("%s: rule %v has an expression that is not exactly `request.path == '/hooks/github' && (inIpRange(...) || ...)`; GitHub's ranges are allowed on the hooks path only (S18, Q28)", [lk.who(p), rule.priority])
}

hooks_rules(p) := [rule | some rule in armor_rules(p); rule.match.expr]

ranges_in_hooks_rules(p) := {m[1] |
	some rule in hooks_rules(p)
	some m in regex.find_all_string_submatch_n(`inIpRange\(origin\.ip, '([^']+)'\)`, expr_text(rule), -1)
}

deny contains msg if {
	some p in armor_policies
	some r in ranges_in_hooks_rules(p)
	not r in hook_ranges
	msg := sprintf("%s: the hooks rules allow %s, which is not one of GitHub's hook ranges (Q28)", [lk.who(p), r])
}

deny contains msg if {
	some p in armor_policies
	some r in hook_ranges
	not r in ranges_in_hooks_rules(p)
	msg := sprintf("%s: GitHub's hook range %s is missing from the hooks rules (stale; regenerate with github-hook-ranges.mjs) (Q28)", [lk.who(p), r])
}

# Helm renders an emptied list as null, so the ranges of a rule are read as a list, and null is no ranges.
src_ranges(rule) := rule.match.config.srcIpRanges if is_array(rule.match.config.srcIpRanges) else := []

ip_rules(p) := [rule | some rule in armor_rules(p); is_object(rule.match.config)]

# A range-based rule (the users') may not carry a GitHub range: that would open every path to GitHub.
deny contains msg if {
	some p in armor_policies
	some rule in ip_rules(p)
	some r in src_ranges(rule)
	r in hook_ranges
	msg := sprintf("%s: rule %v allows GitHub's hook range %s outside the hooks path (S18, Q28)", [lk.who(p), rule.priority, r])
}

allow_ip_rules(p) := [rule | some rule in ip_rules(p); startswith(rule.action, "allow")]

deny contains msg if {
	some p in armor_policies
	some rule in allow_ip_rules(p)
	some r in src_ranges(rule)
	lk.too_wide(r)
	msg := sprintf("%s: rule %v allows %s, which is too wide; it is everyone or close to it (S18)", [lk.who(p), rule.priority, r])
}

deny contains msg if {
	some p in armor_policies
	some rule in allow_ip_rules(p)
	count(src_ranges(rule)) == 0
	msg := sprintf("%s: rule %v has no source ranges (S18)", [lk.who(p), rule.priority])
}

deny contains msg if {
	count(armor_policies) > 0
	some p in armor_policies
	count(allow_ip_rules(p)) == 0
	msg := sprintf("%s: no rule allows the users' ranges (Q2); nobody could sign in (S18)", [lk.who(p)])
}

default_rules(p) := [rule | some rule in armor_rules(p); rule.priority == default_priority]

default_is_deny(rule) if {
	startswith(rule.action, "deny")
	src_ranges(rule) == ["*"]
}

deny contains msg if {
	some p in armor_policies
	count([rule | some rule in default_rules(p); default_is_deny(rule)]) == 0
	msg := sprintf("%s: the default rule (priority %d) must deny everyone (srcIpRanges [\"*\"]) (S18)", [lk.who(p), default_priority])
}

deny contains msg if {
	some p in armor_policies
	some rule in armor_rules(p)
	object.get(rule, "preview", false) == true
	msg := sprintf("%s: rule %v is in preview and enforces nothing (S18)", [lk.who(p), rule.priority])
}

deny contains msg if {
	some p in armor_policies
	priorities := [rule.priority | some rule in armor_rules(p)]
	count(priorities) != count({x | some x in priorities})
	msg := sprintf("%s: two rules have one priority; the order of evaluation would be undefined (S18)", [lk.who(p)])
}
