# Tests for systemd.rego (09/T8; S25): the sentinel's unit file, as deploy/sentinel/tests/unit2json.mjs reads it. Run with
# `conftest verify --policy deploy/policy`. The fixture is the unit as it is meant to be; every test changes one thing.
package systemd

import rego.v1

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

good := {
	"Install": {"WantedBy": ["multi-user.target"]},
	"Unit": {"Description": ["Lanekeeper NFS VM sentinel"], "After": ["network-online.target auditd.service"]},
	"Service": {
		"Type": ["simple"],
		"ExecStart": ["/usr/bin/lanekeeper-sentinel"],
		"User": ["lanekeeper-sentinel"],
		"Group": ["lanekeeper-sentinel"],
		"Restart": ["on-failure"],
		"StateDirectory": ["lanekeeper-sentinel"],
		"ReadWritePaths": ["/var/lib/lanekeeper-sentinel"],
		"NoNewPrivileges": ["yes"],
		"ProtectSystem": ["strict"],
		"ProtectHome": ["yes"],
		"PrivateTmp": ["yes"],
		"PrivateDevices": ["yes"],
		"PrivateMounts": ["yes"],
		"CapabilityBoundingSet": [""],
		"AmbientCapabilities": [""],
		"MemoryDenyWriteExecute": ["yes"],
		"RestrictAddressFamilies": ["AF_UNIX AF_INET AF_INET6"],
		"RestrictNamespaces": ["yes"],
		"RestrictRealtime": ["yes"],
		"RestrictSUIDSGID": ["yes"],
		"LockPersonality": ["yes"],
		"ProtectKernelTunables": ["yes"],
		"ProtectKernelModules": ["yes"],
		"ProtectControlGroups": ["yes"],
		"SystemCallArchitectures": ["native"],
		"SystemCallFilter": ["@system-service", "~@privileged @resources"],
		"RemoveIPC": ["yes"],
		"UMask": ["0077"],
		"KeyringMode": ["private"],
		"DevicePolicy": ["closed"],
	},
}

# The unit with a key set to these assignments, or (with no assignments) without the key.
set_key(key, values) := {"Install": good.Install, "Unit": good.Unit, "Service": object.union(good.Service, {key: values})}

# (object.union would merge the Service sections and keep the key.)
drop_key(key) := {"Install": good.Install, "Unit": good.Unit, "Service": {k: v | some k, v in good.Service; k != key}}

test_good_unit_passes if {
	count(deny) == 0 with input as good
}

# --- the dedicated user

test_deny_no_user if {
	refused(drop_key("User"), "User=lanekeeper-sentinel")
}

test_deny_root_user if {
	refused(set_key("User", ["root"]), "User=lanekeeper-sentinel")
}

test_deny_another_user if {
	refused(set_key("User", ["nobody"]), "User=lanekeeper-sentinel")
}

test_deny_no_group if {
	refused(drop_key("Group"), "Group=lanekeeper-sentinel")
}

test_deny_root_group if {
	refused(set_key("Group", ["root"]), "Group=lanekeeper-sentinel")
}

test_deny_supplementary_groups if {
	refused(set_key("SupplementaryGroups", ["audit"]), "SupplementaryGroups")
}

# The last assignment wins, as in systemd: a later User=root undoes an earlier User=lanekeeper-sentinel.
test_deny_a_later_assignment_that_undoes_the_user if {
	refused(set_key("User", ["lanekeeper-sentinel", "root"]), "User=lanekeeper-sentinel")
}

# --- the switches S25 names

test_deny_no_new_privileges_missing if {
	refused(drop_key("NoNewPrivileges"), "NoNewPrivileges=yes")
}

test_deny_no_new_privileges_off if {
	refused(set_key("NoNewPrivileges", ["no"]), "NoNewPrivileges=yes")
}

test_deny_private_tmp_missing if {
	refused(drop_key("PrivateTmp"), "PrivateTmp=yes")
}

test_deny_memory_deny_write_execute_missing if {
	refused(drop_key("MemoryDenyWriteExecute"), "MemoryDenyWriteExecute=yes")
}

test_deny_memory_deny_write_execute_off if {
	refused(set_key("MemoryDenyWriteExecute", ["false"]), "MemoryDenyWriteExecute=yes")
}

test_deny_protect_home_missing if {
	refused(drop_key("ProtectHome"), "ProtectHome=yes")
}

# read-only and tmpfs leave the home directories in view.
test_deny_protect_home_read_only if {
	refused(set_key("ProtectHome", ["read-only"]), "ProtectHome=yes")
}

test_true_is_yes if {
	count(deny) == 0 with input as set_key("PrivateTmp", ["true"])
}

test_deny_protect_system_missing if {
	refused(drop_key("ProtectSystem"), "ProtectSystem=strict")
}

test_deny_protect_system_full if {
	refused(set_key("ProtectSystem", ["full"]), "ProtectSystem=strict")
}

test_deny_protect_system_yes if {
	refused(set_key("ProtectSystem", ["yes"]), "ProtectSystem=strict")
}

# --- the capabilities: none at all

test_deny_capability_bounding_set_missing if {
	refused(drop_key("CapabilityBoundingSet"), "CapabilityBoundingSet= must be set and empty")
}

test_deny_capability_bounding_set_with_a_capability if {
	refused(set_key("CapabilityBoundingSet", ["CAP_DAC_READ_SEARCH"]), "CapabilityBoundingSet= must be set and empty")
}

# A deny list ("everything but") is not an empty set.
test_deny_capability_bounding_set_as_a_deny_list if {
	refused(set_key("CapabilityBoundingSet", ["~CAP_SYS_ADMIN"]), "CapabilityBoundingSet= must be set and empty")
}

test_deny_capability_added_after_the_reset if {
	refused(set_key("CapabilityBoundingSet", ["", "CAP_NET_BIND_SERVICE"]), "CapabilityBoundingSet= must be set and empty")
}

test_a_reset_after_a_capability_is_empty if {
	count(deny) == 0 with input as set_key("CapabilityBoundingSet", ["CAP_NET_BIND_SERVICE", ""])
}

test_deny_ambient_capabilities if {
	refused(set_key("AmbientCapabilities", ["CAP_NET_BIND_SERVICE"]), "AmbientCapabilities= must be empty")
}

test_ambient_capabilities_unset_is_empty if {
	count(deny) == 0 with input as drop_key("AmbientCapabilities")
}

# --- sockets: local ones and the hub's, nothing else

test_deny_address_families_missing if {
	refused(drop_key("RestrictAddressFamilies"), "RestrictAddressFamilies= must be exactly AF_UNIX AF_INET AF_INET6")
}

test_deny_netlink if {
	refused(set_key("RestrictAddressFamilies", ["AF_UNIX AF_INET AF_INET6 AF_NETLINK"]), "RestrictAddressFamilies= must be exactly AF_UNIX AF_INET AF_INET6")
}

test_deny_packet_sockets if {
	refused(set_key("RestrictAddressFamilies", ["AF_UNIX AF_INET AF_INET6 AF_PACKET"]), "RestrictAddressFamilies= must be exactly AF_UNIX AF_INET AF_INET6")
}

test_deny_no_internet_sockets if {
	refused(set_key("RestrictAddressFamilies", ["AF_UNIX"]), "RestrictAddressFamilies= must be exactly AF_UNIX AF_INET AF_INET6")
}

# A deny list names what is refused, so it is not the list S25 asks for.
test_deny_address_families_as_a_deny_list if {
	refused(set_key("RestrictAddressFamilies", ["~AF_PACKET"]), "RestrictAddressFamilies= must be exactly AF_UNIX AF_INET AF_INET6")
}

test_address_families_in_another_order_pass if {
	count(deny) == 0 with input as set_key("RestrictAddressFamilies", ["AF_INET6 AF_UNIX AF_INET"])
}

# --- the one place it may write

test_deny_read_write_paths_missing if {
	refused(drop_key("ReadWritePaths"), "ReadWritePaths= must name the spool directory")
}

test_deny_read_write_paths_elsewhere if {
	refused(set_key("ReadWritePaths", ["/var/lib/lanekeeper-sentinel /etc"]), "ReadWritePaths= names /etc")
}

test_deny_read_write_paths_beside_the_spool if {
	refused(set_key("ReadWritePaths", ["/var/lib/lanekeeper-sentinel-other"]), "ReadWritePaths= names /var/lib/lanekeeper-sentinel-other")
}

test_deny_read_write_paths_that_climb_out if {
	refused(set_key("ReadWritePaths", ["/var/lib/lanekeeper-sentinel/../../etc"]), "ReadWritePaths= names /var/lib/lanekeeper-sentinel/../../etc")
}

test_deny_the_root_as_a_writable_path if {
	refused(set_key("ReadWritePaths", ["/"]), "ReadWritePaths= names /")
}

test_a_directory_inside_the_spool_passes if {
	count(deny) == 0 with input as set_key("ReadWritePaths", ["/var/lib/lanekeeper-sentinel/spool"])
}

# A path that was written and then reset is not writable any more.
test_a_reset_removes_a_writable_path if {
	refused(set_key("ReadWritePaths", ["/etc", ""]), "ReadWritePaths= must name the spool directory")
}

test_deny_bind_paths if {
	refused(set_key("BindPaths", ["/srv:/srv"]), "BindPaths")
}

test_deny_read_write_bind_paths_to_the_export if {
	refused(set_key("BindPaths", ["/export/csp-configuration"]), "BindPaths")
}

# --- nothing runs with the privileges the unit takes away

test_deny_exec_start_with_a_plus if {
	refused(set_key("ExecStart", ["+/usr/bin/lanekeeper-sentinel"]), "full privileges")
}

test_deny_exec_start_pre_with_a_plus if {
	refused(set_key("ExecStartPre", ["+/bin/chmod 666 /run/lanekeeper-audisp"]), "full privileges")
}

test_deny_exec_start_post_with_a_bang if {
	refused(set_key("ExecStartPost", ["!/bin/true"]), "full privileges")
}

test_deny_exec_stop_with_a_double_bang if {
	refused(set_key("ExecStop", ["!!/bin/true"]), "full privileges")
}

test_deny_exec_start_of_another_program if {
	refused(set_key("ExecStart", ["/bin/sh -c /usr/bin/lanekeeper-sentinel"]), "ExecStart= must run /usr/bin/lanekeeper-sentinel")
}

test_deny_exec_start_missing if {
	refused(drop_key("ExecStart"), "ExecStart= must run /usr/bin/lanekeeper-sentinel")
}

test_deny_two_exec_starts if {
	refused(set_key("ExecStart", ["/usr/bin/lanekeeper-sentinel", "/usr/bin/lanekeeper-sentinel"]), "ExecStart= must run /usr/bin/lanekeeper-sentinel")
}

test_exec_start_with_arguments_passes if {
	count(deny) == 0 with input as set_key("ExecStart", ["/usr/bin/lanekeeper-sentinel --config /etc/lanekeeper-sentinel/sentinel.toml"])
}

test_deny_exec_start_of_a_program_that_only_starts_alike if {
	refused(set_key("ExecStart", ["/usr/bin/lanekeeper-sentinel-evil"]), "ExecStart= must run /usr/bin/lanekeeper-sentinel")
}

# --- the rest of the hardening (A15: directives systemd 239 knows), so that none is dropped without a test noticing

test_deny_a_hardening_switch_missing if {
	every key in {"PrivateDevices", "PrivateMounts", "RestrictNamespaces", "RestrictRealtime", "RestrictSUIDSGID", "LockPersonality", "ProtectKernelTunables", "ProtectKernelModules", "ProtectControlGroups", "RemoveIPC"} {
		refused(drop_key(key), sprintf("%s=yes", [key]))
	}
}

test_deny_a_hardening_switch_off if {
	every key in {"PrivateDevices", "PrivateMounts", "RestrictNamespaces", "RestrictRealtime", "RestrictSUIDSGID", "LockPersonality", "ProtectKernelTunables", "ProtectKernelModules", "ProtectControlGroups", "RemoveIPC"} {
		refused(set_key(key, ["no"]), sprintf("%s=yes", [key]))
	}
}

test_deny_a_writable_directory_that_systemd_makes if {
	every key in {"RuntimeDirectory", "CacheDirectory", "LogsDirectory", "ConfigurationDirectory"} {
		refused(set_key(key, ["something"]), sprintf("%s= makes a writable directory", [key]))
	}
}

test_deny_another_state_directory if {
	refused(set_key("StateDirectory", ["lanekeeper-sentinel other"]), "StateDirectory= names other")
}

test_deny_bind_read_only_paths if {
	refused(set_key("BindReadOnlyPaths", ["/etc"]), "BindReadOnlyPaths= is set")
}

test_deny_a_later_assignment_that_adds_an_ambient_capability_after_a_reset if {
	refused(set_key("AmbientCapabilities", ["", "CAP_NET_RAW"]), "AmbientCapabilities= must be empty")
}

test_deny_system_call_architectures_missing if {
	refused(drop_key("SystemCallArchitectures"), "SystemCallArchitectures=native")
}

test_deny_system_call_architectures_all if {
	refused(set_key("SystemCallArchitectures", [""]), "SystemCallArchitectures=native")
}

test_deny_system_call_filter_missing if {
	refused(drop_key("SystemCallFilter"), "SystemCallFilter= must allow @system-service and refuse @privileged")
}

test_deny_system_call_filter_without_the_refusal if {
	refused(set_key("SystemCallFilter", ["@system-service"]), "SystemCallFilter= must allow @system-service and refuse @privileged")
}

test_deny_system_call_filter_without_the_allow_list if {
	refused(set_key("SystemCallFilter", ["~@privileged @resources"]), "SystemCallFilter= must allow @system-service and refuse @privileged")
}

test_deny_umask_open if {
	refused(set_key("UMask", ["0022"]), "UMask=0077")
}

test_deny_umask_missing if {
	refused(drop_key("UMask"), "UMask=0077")
}

test_deny_keyring_mode_missing if {
	refused(drop_key("KeyringMode"), "KeyringMode=private")
}

test_deny_device_policy_missing if {
	refused(drop_key("DevicePolicy"), "DevicePolicy=closed")
}

test_deny_device_policy_auto if {
	refused(set_key("DevicePolicy", ["auto"]), "DevicePolicy=closed")
}

# --- what it must not be

test_deny_a_unit_with_no_service_section if {
	refused({"Unit": {"Description": ["x"]}}, "[Service]")
}

test_deny_the_network_namespace_switch_that_would_cut_the_hub_off if {
	refused(set_key("PrivateNetwork", ["yes"]), "PrivateNetwork=yes")
}

# --- how it is enabled: multi-user.target and nothing that pulls it into another unit

test_deny_a_unit_that_other_units_require if {
	planted := object.union(good, {"Install": {"RequiredBy": ["sshd.service"]}})
	refused(planted, "[Install] may only have WantedBy=multi-user.target")
}

test_deny_wanted_by_another_target if {
	planted := {"Install": {"WantedBy": ["multi-user.target", "network-online.target"]}, "Unit": good.Unit, "Service": good.Service}
	refused(planted, "[Install] may only have WantedBy=multi-user.target")
}

test_deny_an_alias if {
	planted := {"Install": {"WantedBy": ["multi-user.target"], "Alias": ["auditd.service"]}, "Unit": good.Unit, "Service": good.Service}
	refused(planted, "[Install] may only have WantedBy=multi-user.target")
}

test_a_unit_with_no_install_section_passes if {
	count(deny) == 0 with input as {"Unit": good.Unit, "Service": good.Service}
}
