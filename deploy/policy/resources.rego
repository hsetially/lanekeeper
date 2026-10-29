# Resources policy (09/T3; P11, S16). Run with `conftest test --combine --namespace resources` over the hub chart's output
# together with perf/budgets.toml. The hub's memory request is the P11 budget for memory per replica at design scale, read from
# the file rather than repeated here, so a budget that moves drags the request with it; and the startup probe allows the
# start-up time that P11 budgets.
package resources

import rego.v1

import data.lk

budgets := [b | some f in lk.files_ending("budgets.toml"); some b in f.contents.budget]

budget(id) := b.threshold if {
	some b in budgets
	b.id == id
}

# --- memory --------------------------------------------------------------------------------------------------------------

deny contains msg if {
	count(lk.hub_statefulsets) > 0
	count(lk.files_ending("budgets.toml")) == 0
	msg := "perf/budgets.toml is not among the files checked, so the hub's memory request cannot be compared with P11; pass it to conftest with --combine"
}

deny contains msg if {
	count(lk.hub_statefulsets) > 0
	count(lk.files_ending("budgets.toml")) > 0
	not budget("P11.memory_mib")
	msg := "perf/budgets.toml has no P11.memory_mib entry to compare the hub's memory request with"
}

deny contains msg if {
	count(lk.hub_statefulsets) > 0
	count(lk.files_ending("budgets.toml")) > 0
	not budget("P11")
	msg := "perf/budgets.toml has no P11 entry to compare the hub's startup probe with"
}

hub_containers := [{"sts": sts, "container": c} | some sts in lk.hub_statefulsets; some c in sts.spec.template.spec.containers]

memory(c, section) := object.get(c, ["resources", section, "memory"], null)

deny contains msg if {
	some x in hub_containers
	some section in ["requests", "limits"]
	q := memory(x.container, section)
	q != null
	not lk.mebibytes(q)
	msg := sprintf("%s: the memory %s %q has a unit this policy does not read; use Mi or Gi (P11)", [lk.who(x.sts), section, q])
}

deny contains msg if {
	some x in hub_containers
	want := budget("P11.memory_mib")
	got := lk.mebibytes(memory(x.container, "requests"))
	got != want
	msg := sprintf("%s: the memory request is %v MiB and the P11 budget (P11.memory_mib in perf/budgets.toml) is %v MiB; the request is the budget (P11)", [lk.who(x.sts), got, want])
}

deny contains msg if {
	some x in hub_containers
	request := lk.mebibytes(memory(x.container, "requests"))
	limit := lk.mebibytes(memory(x.container, "limits"))
	limit < request
	msg := sprintf("%s: the memory limit (%v MiB) is below the request (%v MiB) (P11)", [lk.who(x.sts), limit, request])
}

# --- start-up -----------------------------------------------------------------------------------------------------------

# A pod that is restarted while it is still starting never starts, so the startup probe must allow at least the budgeted time.
# It asks /healthz. The hub binds all four sockets first but serves the operations socket last (crates/hub/src/process.rs, Sockets::bind and serve_hub), so /healthz answers only
# when the whole start has finished, and a start that cannot reach the database, Secret Manager or KMS exits 1 and restarts on its own; the
# startup probe is what keeps the liveness probe from judging a start that has not finished.
deny contains msg if {
	some x in hub_containers
	not x.container.startupProbe
	msg := sprintf("%s: the container has no startupProbe, so the liveness probe would judge a slow start (P11)", [lk.who(x.sts)])
}

deny contains msg if {
	some x in hub_containers
	probe := x.container.startupProbe
	object.get(probe, ["httpGet", "path"], "") != "/healthz"
	msg := sprintf("%s: the startupProbe must ask /healthz on the ops port, not %q (P11)", [lk.who(x.sts), object.get(probe, ["httpGet", "path"], "")])
}

deny contains msg if {
	some x in hub_containers
	probe := x.container.startupProbe
	startup_ms := budget("P11")
	allowed_s := object.get(probe, "periodSeconds", 10) * object.get(probe, "failureThreshold", 3)
	allowed_s * 1000 < startup_ms
	msg := sprintf("%s: the startupProbe allows %v s and P11 budgets start-up at %v s (P11)", [lk.who(x.sts), allowed_s, startup_ms / 1000])
}
