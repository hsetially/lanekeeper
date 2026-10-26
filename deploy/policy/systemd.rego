# The sentinel's systemd unit (09/T8; S25). Run with
#
#   node deploy/sentinel/tests/unit2json.mjs deploy/sentinel/lanekeeper-sentinel.service > unit.json
#   conftest test --policy deploy/policy --namespace systemd unit.json
#
# The unit is read as systemd reads it (unit2json.mjs): every assignment of a key, in order, and an empty assignment resets a list,
# so only what follows the last reset counts. S25 asks for a dedicated user, NoNewPrivileges, ProtectSystem=strict, ProtectHome,
# PrivateTmp, an empty capability bounding set, MemoryDenyWriteExecute, sockets limited to AF_UNIX and AF_INET/AF_INET6, and a unit
# that can write only its spool directory. The rest of the hardening is the set of directives that systemd 239 (Rocky Linux 8) knows
# (plan 09 A15), kept here so that none of them is dropped without a test noticing. What this file cannot say is whether systemd 239
# understands every directive, or what the exposure score is: deploy/sentinel/tests/exposure.sh runs the real systemd-analyze for that.
package systemd

import rego.v1

service := object.get(input, "Service", {})

assignments(key) := object.get(service, key, [])

# An empty assignment resets a list (systemd.syntax(7)); the index of the last one, or -1.
last_reset(key) := max(array.concat([-1], [i | some i, v in assignments(key); v == ""]))

effective(key) := [v | some i, v in assignments(key); i > last_reset(key)]

words(key) := [w | some v in effective(key); some w in regex.split(`\s+`, trim_space(v)); w != ""]

last(key) := a[count(a) - 1] if {
	a := assignments(key)
	count(a) > 0
}

last_is(key, want) if last(key) == want

yes_values := {"yes", "true", "1", "on"}

is_yes(key) if lower(last(key)) in yes_values

spool_dir := "/var/lib/lanekeeper-sentinel"

program := "/usr/bin/lanekeeper-sentinel"

deny contains "the unit has no [Service] section (S25)" if not input.Service

# --- a dedicated user -----------------------------------------------------------------------------------------------------------

deny contains msg if {
	some key in {"User", "Group"}
	not last_is(key, "lanekeeper-sentinel")
	msg := sprintf("%s=lanekeeper-sentinel is required: the sentinel runs as its own user and group, never root and never another account's (S25)", [key])
}

deny contains msg if {
	count(effective("SupplementaryGroups")) > 0
	msg := sprintf("SupplementaryGroups= is set (%v): the sentinel reaches the audisp socket through its own group, and has no other (S25)", [words("SupplementaryGroups")])
}

# --- the switches S25 names, and the rest of the hardening ---------------------------------------------------------------------------

required_yes := {"NoNewPrivileges", "PrivateTmp", "MemoryDenyWriteExecute", "ProtectHome"}

hardening_yes := {
	"PrivateDevices", "PrivateMounts", "RestrictNamespaces", "RestrictRealtime", "RestrictSUIDSGID", "LockPersonality",
	"ProtectKernelTunables", "ProtectKernelModules", "ProtectControlGroups", "RemoveIPC",
}

deny contains msg if {
	some key in required_yes | hardening_yes
	not is_yes(key)
	msg := sprintf("%s=yes is required (S25)", [key])
}

deny contains msg if {
	not last_is("ProtectSystem", "strict")
	msg := "ProtectSystem=strict is required: the whole file system but the spool is read-only (S25); yes and full leave /etc writable"
}

deny contains msg if {
	not last_is("SystemCallArchitectures", "native")
	msg := "SystemCallArchitectures=native is required: no 32-bit system call table (A15)"
}

deny contains msg if {
	not last_is("UMask", "0077")
	msg := "UMask=0077 is required: what the sentinel creates is readable by itself alone (A15)"
}

deny contains msg if {
	not last_is("KeyringMode", "private")
	msg := "KeyringMode=private is required: the sentinel shares no key material with other services (A15)"
}

deny contains msg if {
	not last_is("DevicePolicy", "closed")
	msg := "DevicePolicy=closed is required: no device but the pseudo devices (A15)"
}

# The system calls: the usual set of a service, minus the privileged ones and the resource ones.
deny contains msg if {
	allowed := {w | some w in words("SystemCallFilter")}
	not "@system-service" in allowed
	msg := "SystemCallFilter= must allow @system-service and refuse @privileged (A15): the allow list is missing"
}

deny contains msg if {
	allowed := {w | some w in words("SystemCallFilter")}
	not "~@privileged" in allowed
	msg := "SystemCallFilter= must allow @system-service and refuse @privileged (A15): the refusal is missing"
}

# --- no capability at all -----------------------------------------------------------------------------------------------------

deny contains msg if {
	not capability_set_empty
	msg := "CapabilityBoundingSet= must be set and empty: the sentinel needs no capability, and the bounding set is the ceiling for everything below it (S25)"
}

capability_set_empty if {
	count(assignments("CapabilityBoundingSet")) > 0
	count(words("CapabilityBoundingSet")) == 0
}

deny contains msg if {
	count(words("AmbientCapabilities")) > 0
	msg := sprintf("AmbientCapabilities= must be empty, and it is %v (S25)", [words("AmbientCapabilities")])
}

# --- sockets: a local one for the audisp plugin and the hub's, nothing else --------------------------------------------------------------

address_families := {"AF_UNIX", "AF_INET", "AF_INET6"}

deny contains msg if {
	{w | some w in words("RestrictAddressFamilies")} != address_families
	msg := sprintf("RestrictAddressFamilies= must be exactly AF_UNIX AF_INET AF_INET6, and is %v: the audisp socket and the hub's mutual TLS, no netlink and no packet sockets (S25)", [words("RestrictAddressFamilies")])
}

# Outbound only: a private network namespace would cut the sentinel off from the hub.
deny contains msg if {
	is_yes("PrivateNetwork")
	msg := "PrivateNetwork=yes would cut the sentinel off from the hub, which it reaches outbound over mutual TLS (S25)"
}

# --- the one place it may write ------------------------------------------------------------------------------------------------------

deny contains msg if {
	count(words("ReadWritePaths")) == 0
	msg := sprintf("ReadWritePaths= must name the spool directory %s, and nothing else (S25)", [spool_dir])
}

inside_spool(path) if path == spool_dir

inside_spool(path) if {
	startswith(path, concat("", [spool_dir, "/"]))
	not regex.match(`(^|/)\.\.(/|$)`, path)
}

deny contains msg if {
	some path in words("ReadWritePaths")
	not inside_spool(path)
	msg := sprintf("ReadWritePaths= names %s, which is not the spool directory %s (S25)", [path, spool_dir])
}

deny contains msg if {
	some key in {"BindPaths", "BindReadOnlyPaths"}
	count(effective(key)) > 0
	msg := sprintf("%s= is set (%v): it would show a host path to the sentinel, writable or not, in a unit whose only path is its spool (S25)", [key, words(key)])
}

# The directories that systemd makes and makes writable: the state directory is the spool, and there is no other.
deny contains msg if {
	some w in words("StateDirectory")
	w != "lanekeeper-sentinel"
	msg := sprintf("StateDirectory= names %s: the only writable directory is lanekeeper-sentinel, the spool (S25)", [w])
}

deny contains msg if {
	some key in {"RuntimeDirectory", "CacheDirectory", "LogsDirectory", "ConfigurationDirectory"}
	count(words(key)) > 0
	msg := sprintf("%s= makes a writable directory (%v): the only one is the spool (S25); logs go to the journal", [key, words(key)])
}

# --- nothing runs with the privileges the unit takes away -----------------------------------------------------------------------------

exec_keys := {"ExecStart", "ExecStartPre", "ExecStartPost", "ExecCondition", "ExecReload", "ExecStop", "ExecStopPost"}

# The prefixes of systemd.service(5): `@`, `-` and `:` change how a line runs; `+` runs it with full privileges, `!` and `!!` with those
# of the unit's user but the ambient capabilities of root.
deny contains msg if {
	some key in exec_keys
	some v in assignments(key)
	regex.match(`^[@:-]*(\+|!)`, v)
	msg := sprintf("%s=%s runs with full privileges or with the capabilities of root, outside the restrictions of the unit (S25)", [key, v])
}

deny contains msg if {
	not exec_start_ok
	msg := sprintf("ExecStart= must run %s, once (S25)", [program])
}

exec_start_ok if {
	a := assignments("ExecStart")
	count(a) == 1
	regex.split(`\s+`, trim_space(a[0]))[0] == program
}

# --- how it is enabled -------------------------------------------------------------------------------------------------------------------
#
# Config management enables the unit, so it has an [Install] section; the section pulls it into multi-user.target and into nothing else, so
# that enabling it cannot make another unit (auditd, sshd, the network) start, depend on or be renamed to it.
deny contains msg if {
	install := object.get(input, "Install", {})
	count([k | some k, v in install; not install_key_ok(k, v)]) > 0
	msg := "[Install] may only have WantedBy=multi-user.target: the unit is pulled into that target and into no other unit (S25)"
}

install_key_ok(k, v) if {
	k == "WantedBy"
	v == ["multi-user.target"]
}
