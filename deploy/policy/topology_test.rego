# Tests for topology.rego: the hub is three replicas spread over zones, with a PodDisruptionBudget that keeps two, and a headless
# Service for the replicas to find one another (D62).
package topology

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

anti := "/spec/template/spec/affinity/podAntiAffinity"

test_good_passes if {
	count(deny) == 0 with input as fixtures.hub
	count(warn) == 0 with input as fixtures.hub
}

# --- three replicas

test_replicas_3 if {
	refused(fixtures.sts_set("/spec/replicas", 2), "3 replicas")
}

test_replicas_five if {
	refused(fixtures.sts_set("/spec/replicas", 5), "3 replicas")
}

test_replicas_missing if {
	refused(fixtures.sts_remove("/spec/replicas"), "3 replicas")
}

# --- the budget

test_pdb_min_available_2 if {
	planted := fixtures.patch("PodDisruptionBudget", "lanekeeper-hub", [{"op": "replace", "path": "/spec/minAvailable", "value": 1}])
	refused(planted, "minAvailable: 2")
}

test_pdb_percentage_is_not_enough if {
	planted := fixtures.patch("PodDisruptionBudget", "lanekeeper-hub", [{"op": "replace", "path": "/spec/minAvailable", "value": "50%"}])
	refused(planted, "minAvailable: 2")
}

test_pdb_max_unavailable_instead if {
	planted := fixtures.patch("PodDisruptionBudget", "lanekeeper-hub", [
		{"op": "remove", "path": "/spec/minAvailable"},
		{"op": "add", "path": "/spec/maxUnavailable", "value": 1},
	])
	refused(planted, "minAvailable: 2")
}

test_pdb_missing if {
	refused(fixtures.without("PodDisruptionBudget"), "PodDisruptionBudget")
}

test_pdb_selects_other_pods if {
	planted := fixtures.patch("PodDisruptionBudget", "lanekeeper-hub", [{"op": "replace", "path": "/spec/selector/matchLabels", "value": {"app.kubernetes.io/name": "other"}}])
	refused(planted, "selects")
}

test_pdb_with_an_empty_selector if {
	planted := fixtures.patch("PodDisruptionBudget", "lanekeeper-hub", [{"op": "replace", "path": "/spec/selector/matchLabels", "value": {}}])
	refused(planted, "empty selector")
}

# --- spread over zones

test_zone_anti_affinity if {
	refused(fixtures.sts_remove("/spec/template/spec/affinity"), "zone")
}

test_zone_anti_affinity_on_hostname if {
	planted := fixtures.sts_set(concat("", [anti, "/requiredDuringSchedulingIgnoredDuringExecution/0/topologyKey"]), "kubernetes.io/hostname")
	refused(planted, "zone")
}

test_zone_anti_affinity_only_preferred if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "remove", "path": concat("", [anti, "/requiredDuringSchedulingIgnoredDuringExecution"])},
		{"op": "add", "path": concat("", [anti, "/preferredDuringSchedulingIgnoredDuringExecution"]), "value": [{
			"weight": 100,
			"podAffinityTerm": {"labelSelector": {"matchLabels": fixtures.selector}, "topologyKey": "topology.kubernetes.io/zone"},
		}]},
	])
	refused(planted, "zone")
}

# A selector that matches nothing spreads nothing.
test_zone_anti_affinity_selecting_other_pods if {
	planted := fixtures.sts_set(concat("", [anti, "/requiredDuringSchedulingIgnoredDuringExecution/0/labelSelector/matchLabels"]), {"app.kubernetes.io/name": "other"})
	refused(planted, "its own pods")
}

# kind has one zone: it spreads over its three workers by hostname, says so, and is warned about (plan 09 A13).
test_kind_spreads_by_hostname_with_a_warning if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "add", "path": "/metadata/annotations", "value": {"lanekeeper.io/topology": "kind"}},
		{"op": "remove", "path": concat("", [anti, "/requiredDuringSchedulingIgnoredDuringExecution"])},
		{"op": "add", "path": concat("", [anti, "/preferredDuringSchedulingIgnoredDuringExecution"]), "value": [{
			"weight": 100,
			"podAffinityTerm": {"labelSelector": {"matchLabels": fixtures.selector}, "topologyKey": "kubernetes.io/hostname"},
		}]},
	])
	count(deny) == 0 with input as planted
	warned(planted, "kind")
}

test_kind_annotation_does_not_excuse_missing_spread if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "add", "path": "/metadata/annotations", "value": {"lanekeeper.io/topology": "kind"}},
		{"op": "remove", "path": "/spec/template/spec/affinity"},
	])
	refused(planted, "hostname")
}

# --- the headless Service

test_headless_service if {
	refused(fixtures.without("Service") , "lanekeeper-hub-replicas")
}

test_headless_service_with_a_cluster_ip if {
	planted := fixtures.patch("Service", "lanekeeper-hub-replicas", [{"op": "replace", "path": "/spec/clusterIP", "value": "10.0.0.7"}])
	refused(planted, "headless")
}

test_headless_service_without_the_replica_port if {
	planted := fixtures.patch("Service", "lanekeeper-hub-replicas", [{"op": "replace", "path": "/spec/ports/0/port", "value": 8444}])
	refused(planted, "8445")
}

test_stateful_set_names_another_service if {
	refused(fixtures.sts_set("/spec/serviceName", "other"), "serviceName")
}

test_headless_service_selects_other_pods if {
	planted := fixtures.patch("Service", "lanekeeper-hub-replicas", [{"op": "replace", "path": "/spec/selector", "value": {"app.kubernetes.io/name": "other"}}])
	refused(planted, "selects")
}
