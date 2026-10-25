# Topology policy (09/T3; D62). Run with `conftest test --combine --namespace topology` over the hub chart's output: three
# replicas that are spread over zones, a PodDisruptionBudget that keeps two of them through a disruption, and a headless Service
# through which the replicas find one another (`agent_connections.replica_address`).
package topology

import rego.v1

import data.lk

headless_name := "lanekeeper-hub-replicas"

subset(wanted, labels) if {
	every k, v in wanted {
		labels[k] == v
	}
}

pod_template_labels(sts) := object.get(sts.spec.template.metadata, "labels", {})

# --- three replicas ----------------------------------------------------------------------------------------------------

deny contains msg if {
	some sts in lk.hub_statefulsets
	object.get(sts.spec, "replicas", 0) != 3
	msg := sprintf("%s: the hub runs 3 replicas (D62), not %v", [lk.who(sts), object.get(sts.spec, "replicas", "unset")])
}

# --- the PodDisruptionBudget ---------------------------------------------------------------------------------------------

hub_pdbs := lk.named("PodDisruptionBudget", lk.hub_name)

deny contains msg if {
	count(lk.hub_statefulsets) > 0
	count(hub_pdbs) == 0
	msg := "PodDisruptionBudget/lanekeeper-hub is missing: a node drain could take all replicas at once (D62)"
}

deny contains msg if {
	some pdb in hub_pdbs
	object.get(pdb.spec, "minAvailable", null) != 2
	msg := sprintf("%s: the budget must be minAvailable: 2, so that two replicas stay through a disruption (D62)", [lk.who(pdb)])
}

deny contains msg if {
	some pdb in hub_pdbs
	some sts in lk.hub_statefulsets
	not subset(object.get(pdb.spec.selector, "matchLabels", {}), pod_template_labels(sts))
	msg := sprintf("%s: its selector selects pods that the StatefulSet does not make (D62)", [lk.who(pdb)])
}

deny contains msg if {
	some pdb in hub_pdbs
	some sts in lk.hub_statefulsets
	count(object.get(pdb.spec.selector, "matchLabels", {})) == 0
	msg := sprintf("%s: an empty selector selects nothing it should (D62)", [lk.who(pdb)])
}

# --- spread over zones ---------------------------------------------------------------------------------------------------

# kind has one zone. It spreads the replicas over its workers by hostname, as a preference, says so with an annotation on the
# StatefulSet, and the policy warns every time (plan 09 A13). Everywhere else the spread is required, over zones.
kind_cluster(sts) if object.get(sts.metadata, ["annotations", "lanekeeper.io/topology"], "") == "kind"

anti_affinity(sts) := object.get(sts.spec.template.spec, ["affinity", "podAntiAffinity"], {})

required_terms(sts) := object.get(anti_affinity(sts), "requiredDuringSchedulingIgnoredDuringExecution", [])

preferred_terms(sts) := [w.podAffinityTerm | some w in object.get(anti_affinity(sts), "preferredDuringSchedulingIgnoredDuringExecution", [])]

deny contains msg if {
	some sts in lk.hub_statefulsets
	not kind_cluster(sts)
	zone_terms := [t | some t in required_terms(sts); t.topologyKey == "topology.kubernetes.io/zone"]
	count(zone_terms) == 0
	msg := sprintf("%s: the replicas must repel one another across zones: a required podAntiAffinity on topology.kubernetes.io/zone (D62)", [lk.who(sts)])
}

deny contains msg if {
	some sts in lk.hub_statefulsets
	kind_cluster(sts)
	host_terms := [t | some t in preferred_terms(sts); t.topologyKey == "kubernetes.io/hostname"]
	count(host_terms) == 0
	msg := sprintf("%s: on kind the replicas must at least prefer different hosts: a preferred podAntiAffinity on kubernetes.io/hostname (plan 09 A13)", [lk.who(sts)])
}

warn contains msg if {
	some sts in lk.hub_statefulsets
	kind_cluster(sts)
	msg := sprintf("%s: spread over hosts, not zones, because the cluster is kind (plan 09 A13); never use this in a real cluster", [lk.who(sts)])
}

# A selector that matches no replica spreads nothing.
deny contains msg if {
	some sts in lk.hub_statefulsets
	some term in array.concat(required_terms(sts), preferred_terms(sts))
	not subset(object.get(term.labelSelector, "matchLabels", {}), pod_template_labels(sts))
	msg := sprintf("%s: the anti-affinity selects pods other than its own pods, so it spreads nothing (D62)", [lk.who(sts)])
}

# --- the headless Service ---------------------------------------------------------------------------------------------------

headless_services := lk.named("Service", headless_name)

deny contains msg if {
	count(lk.hub_statefulsets) > 0
	count(headless_services) == 0
	msg := "Service/lanekeeper-hub-replicas is missing: the replicas find one another through a headless Service (D62)"
}

deny contains msg if {
	some svc in headless_services
	object.get(svc.spec, "clusterIP", "") != "None"
	msg := sprintf("%s: must be headless (clusterIP: None), so that each replica has its own DNS name (D62)", [lk.who(svc)])
}

deny contains msg if {
	some svc in headless_services
	ports := {[p.name, p.port] | some p in object.get(svc.spec, "ports", [])}
	not ["replica", 8445] in ports
	msg := sprintf("%s: must expose the replica port, 8445 (HARDENING.md)", [lk.who(svc)])
}

deny contains msg if {
	some svc in headless_services
	some sts in lk.hub_statefulsets
	not subset(object.get(svc.spec, "selector", {}), pod_template_labels(sts))
	msg := sprintf("%s: its selector selects pods that the StatefulSet does not make (D62)", [lk.who(svc)])
}

deny contains msg if {
	some sts in lk.hub_statefulsets
	object.get(sts.spec, "serviceName", "") != headless_name
	msg := sprintf("%s: serviceName must be %s, the headless Service (D62)", [lk.who(sts), headless_name])
}
