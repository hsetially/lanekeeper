# Tests for resources.rego: the hub's memory request is the P11 budget (perf/budgets.toml), its limit is not below it, and the
# startup probe allows the 10 s that P11 budgets for start-up.
package resources

import rego.v1

import data.fixtures

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

memory_op(section, value) := {"op": "replace", "path": concat("", ["/spec/template/spec/containers/0/resources/", section, "/memory"]), "value": value}

with_budget(threshold) := [
	{"path": "rendered/hub.yaml", "contents": fixtures.hub_docs},
	{"path": "perf/budgets.toml", "contents": {"budget": [{"id": "P11", "threshold": 10000}, {"id": "P11.memory_mib", "threshold": threshold}]}},
]

test_good_passes if {
	count(deny) == 0 with input as fixtures.hub
}

test_hub_memory_request_matches_p11 if {
	count(deny) == 0 with input as with_budget(1536)
}

test_deny_request_below_the_budget if {
	refused(fixtures.patch("StatefulSet", "lanekeeper-hub", [memory_op("requests", "1Gi")]), "P11")
}

test_deny_request_above_the_budget if {
	refused(fixtures.patch("StatefulSet", "lanekeeper-hub", [memory_op("requests", "1792Mi")]), "P11")
}

# The budget moves, the request must follow: that is why the policy reads the file instead of repeating the number.
test_deny_when_the_budget_changes if {
	refused(with_budget(2048), "P11")
}

test_request_in_gibibytes_is_read if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [memory_op("requests", "1.5Gi")])
	count(deny) == 0 with input as planted
}

test_deny_limit_below_the_request if {
	refused(fixtures.patch("StatefulSet", "lanekeeper-hub", [memory_op("limits", "1Gi")]), "limit")
}

test_deny_unreadable_unit if {
	refused(fixtures.patch("StatefulSet", "lanekeeper-hub", [memory_op("requests", "1536M")]), "unit")
}

test_deny_budget_file_missing if {
	refused(fixtures.without_budgets, "budgets.toml")
}

test_deny_budget_entry_missing if {
	planted := [
		{"path": "rendered/hub.yaml", "contents": fixtures.hub_docs},
		{"path": "perf/budgets.toml", "contents": {"budget": [{"id": "P11", "threshold": 10000}]}},
	]
	refused(planted, "P11.memory_mib")
}

test_deny_startup_budget_missing if {
	planted := [
		{"path": "rendered/hub.yaml", "contents": fixtures.hub_docs},
		{"path": "perf/budgets.toml", "contents": {"budget": [{"id": "P11.memory_mib", "threshold": 1536}]}},
	]
	refused(planted, "P11 entry")
}

# --- start-up (P11: p95 under 10 s)

test_deny_startup_probe_missing if {
	refused(fixtures.sts_remove("/spec/template/spec/containers/0/startupProbe"), "startupProbe")
}

test_deny_startup_probe_too_short if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "replace", "path": "/spec/template/spec/containers/0/startupProbe/failureThreshold", "value": 4},
	])
	refused(planted, "10 s")
}

test_startup_probe_of_exactly_ten_seconds_is_enough if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "replace", "path": "/spec/template/spec/containers/0/startupProbe/failureThreshold", "value": 5},
	])
	count(deny) == 0 with input as planted
}

test_deny_startup_probe_on_another_path if {
	planted := fixtures.patch("StatefulSet", "lanekeeper-hub", [
		{"op": "replace", "path": "/spec/template/spec/containers/0/startupProbe/httpGet/path", "value": "/readyz"},
	])
	refused(planted, "/healthz")
}
