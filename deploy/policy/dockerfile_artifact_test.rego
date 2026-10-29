# Tests for dockerfile_artifact.rego (09/T8; S19, S20): the good input passes, and each planted violation is refused by its own rule.
# Run with `conftest verify --policy deploy/policy`.
package dockerfile_artifact

import rego.v1

rust := "docker.io/library/rust:1.88.0-trixie@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db"

ubuntu := "docker.io/library/ubuntu:noble-20260930.1@sha256:68ee22004a9109b44973313b02ff7b1de250f1de0f5ecb628d73f579aaff33ac"

# The shape conftest's parser produces (see the header of dockerfile.rego).
ins(stage, cmd, value, flags) := {"Stage": stage, "Cmd": cmd, "SubCmd": "", "Value": value, "Flags": flags, "JSON": false}

# The stages: `binary` builds the sentinel; `rpm-tools` is the tools that sign and package (rpm and gpg from the signed archive) and
# depends on no other stage, so that building it builds nothing of ours; `rpm` extends it with the binary and runs rpmbuild; `out` exports.
good := [
	ins(0, "from", [rust, "AS", "binary"], []),
	ins(0, "run", ["curl -fsSL -o /tmp/std.tar.xz https://static.rust-lang.org/dist/x.tar.xz && echo \"$sha  /tmp/std.tar.xz\" | sha256sum -c -"], []),
	ins(0, "copy", [".", "."], []),
	ins(0, "run", ["cargo build --release --locked --target x86_64-unknown-linux-musl -p sentinel"], []),
	ins(1, "from", [ubuntu, "AS", "rpm-tools"], []),
	ins(1, "run", ["apt-get update && apt-get install --yes --no-install-recommends rpm=4.18.2+dfsg-2.1build2 'gpg=2.4.4-2ubuntu17*' 'gpg-agent=2.4.4-2ubuntu17*'"], []),
	ins(2, "from", ["rpm-tools", "AS", "rpm"], []),
	ins(2, "copy", ["/src/sentinel", "/build/SOURCES/"], ["--from=binary"]),
	ins(2, "run", ["rpmbuild -bb /build/SPECS/lanekeeper-sentinel.spec"], []),
	ins(3, "from", ["scratch", "AS", "out"], []),
	ins(3, "copy", ["/src/sentinel", "/lanekeeper-sentinel"], ["--from=binary"]),
	ins(3, "copy", ["/build/RPMS/x86_64/", "/"], ["--from=rpm"]),
]

# Where the instructions are in `good`.
at_tools_from := 4

at_apt := 5

at_rpm_from := 6

at_out_from := 9

# Replace the instruction at index n.
with_at(n, replacement) := array.concat(array.concat(array.slice(good, 0, n), [replacement]), array.slice(good, n + 1, count(good)))

appended(extra) := array.concat(good, [extra])

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

test_good_passes if {
	count(deny) == 0 with input as good
}

# --- the bases

test_deny_tag_only_base if {
	refused(with_at(0, ins(0, "from", ["docker.io/library/rust:1.88.0-trixie", "AS", "binary"], [])), "not pinned by digest")
}

test_deny_floating_base if {
	refused(with_at(at_tools_from, ins(1, "from", ["docker.io/library/ubuntu:latest", "AS", "rpm-tools"], [])), "not pinned by digest")
}

test_deny_floating_base_under_the_rpm_stage if {
	refused(with_at(at_rpm_from, ins(2, "from", ["docker.io/library/ubuntu:latest", "AS", "rpm"], [])), "not pinned by digest")
}

# A stage may start from an EARLIER stage of the same file (its root is pinned, which the rule above checks). A name that is no earlier
# stage is an unpinned image: a later stage, a stage that is not there, or a tag.
test_a_stage_may_extend_an_earlier_stage if {
	count(deny) == 0 with input as with_at(at_rpm_from, ins(2, "from", ["binary", "AS", "rpm"], []))
}

test_deny_a_base_that_is_a_later_stage if {
	refused(with_at(at_rpm_from, ins(2, "from", ["out", "AS", "rpm"], [])), "not pinned by digest")
}

test_deny_a_base_that_names_no_stage if {
	refused(with_at(at_rpm_from, ins(2, "from", ["nothing", "AS", "rpm"], [])), "not pinned by digest")
}

# --- the export stage: files made by earlier stages, and nothing else

test_deny_a_final_stage_that_is_an_image if {
	refused(with_at(at_out_from, ins(3, "from", [ubuntu, "AS", "out"], [])), "the final stage must be FROM scratch")
}

test_deny_an_unpinned_image_as_the_final_stage if {
	refused(with_at(at_out_from, ins(3, "from", ["docker.io/library/ubuntu:latest", "AS", "out"], [])), "not pinned by digest")
}

test_deny_a_final_stage_that_runs_something if {
	refused(appended(ins(3, "run", ["echo hello"], [])), "the final stage may only copy")
}

test_deny_a_final_stage_that_copies_the_context if {
	refused(appended(ins(3, "copy", ["secrets.txt", "/"], [])), "the final stage copies from the build context")
}

test_deny_a_final_stage_with_an_entrypoint if {
	refused(appended(ins(3, "entrypoint", ["/lanekeeper-sentinel"], [])), "the final stage may only copy")
}

test_deny_a_final_stage_that_adds if {
	refused(appended(ins(3, "add", ["https://example.com/x", "/x"], [])), "the final stage may only copy")
}

# --- downloads and installs

test_deny_add_anywhere if {
	refused(with_at(2, ins(0, "add", ["https://example.com/x.tar.xz", "/x.tar.xz"], [])), "ADD is not allowed")
}

test_deny_unlocked_cargo_build if {
	refused(with_at(3, ins(0, "run", ["cargo build --release --target x86_64-unknown-linux-musl -p sentinel"], [])), "without --locked")
}

test_deny_unlocked_cargo_install if {
	refused(appended(ins(0, "run", ["cargo install cargo-cyclonedx"], [])), "without --locked")
}

test_deny_unchecked_download if {
	refused(with_at(1, ins(0, "run", ["curl -fsSL -o /tmp/std.tar.xz https://static.rust-lang.org/dist/x.tar.xz"], [])), "not checked with sha256sum -c")
}

test_deny_unchecked_wget if {
	refused(appended(ins(0, "run", ["wget https://example.com/tool"], [])), "not checked with sha256sum -c")
}

test_deny_download_piped_into_a_shell if {
	refused(appended(ins(0, "run", ["curl -fsSL https://example.com/install.sh | sh"], [])), "piped into a shell")
}

test_deny_apt_without_no_install_recommends if {
	refused(with_at(at_apt, ins(1, "run", ["apt-get update && apt-get install --yes rpm=4.18.2+dfsg-2.1build2 'gpg=2.4.4-2ubuntu17*' 'gpg-agent=2.4.4-2ubuntu17*'"], [])), "--no-install-recommends")
}

test_deny_unpinned_rpm if {
	refused(with_at(at_apt, ins(1, "run", ["apt-get update && apt-get install --yes --no-install-recommends rpm 'gpg=2.4.4-2ubuntu17*' 'gpg-agent=2.4.4-2ubuntu17*'"], [])), "rpm must be pinned")
}

# gpg and its agent are the tools that sign: the upstream version is pinned (the glob leaves the archive's security rebuilds in, which a
# superseded exact version would not), and the exact versions are printed by the build.
test_deny_unpinned_gpg if {
	refused(with_at(at_apt, ins(1, "run", ["apt-get update && apt-get install --yes --no-install-recommends rpm=4.18.2+dfsg-2.1build2 gpg gpg-agent=2.4.4-2ubuntu17*"], [])), "gpg must be pinned")
}

test_deny_a_quoted_unpinned_gpg if {
	refused(with_at(at_apt, ins(1, "run", ["apt-get update && apt-get install --yes --no-install-recommends rpm=4.18.2+dfsg-2.1build2 'gpg' 'gpg-agent=2.4.4-2ubuntu17*'"], [])), "gpg must be pinned")
}

test_deny_unpinned_gpg_agent if {
	refused(with_at(at_apt, ins(1, "run", ["apt-get update && apt-get install --yes --no-install-recommends rpm=4.18.2+dfsg-2.1build2 gpg=2.4.4-2ubuntu17* gpg-agent"], [])), "gpg-agent must be pinned")
}

test_deny_apt_upgrade if {
	refused(appended(ins(1, "run", ["apt-get upgrade --yes"], [])), "apt-get upgrade")
}

test_deny_apt_dist_upgrade if {
	refused(appended(ins(1, "run", ["apt-get dist-upgrade --yes"], [])), "apt-get upgrade")
}

test_deny_unauthenticated_packages if {
	refused(appended(ins(1, "run", ["apt-get install --yes --no-install-recommends --allow-unauthenticated foo"], [])), "unauthenticated")
}

test_deny_force_yes if {
	refused(appended(ins(1, "run", ["apt-get install --yes --no-install-recommends --force-yes foo"], [])), "unauthenticated")
}

test_deny_a_secret_looking_variable if {
	refused(appended(ins(1, "env", ["SIGNING_KEY=abc"], [])), "a secret-looking variable")
}

test_deny_a_secret_looking_argument if {
	refused(appended(ins(0, "arg", ["RELEASE_TOKEN"], [])), "a secret-looking variable")
}

test_a_mirror_argument_is_not_a_secret if {
	count(deny) == 0 with input as appended(ins(1, "arg", ["LK_APT_MIRROR=http://archive.ubuntu.com/ubuntu"], []))
}

# --- the tools stage: the signing jobs build only this (S19, S20, S7)
#
# `sign-release` holds the Workload Identity credential and the release key, and builds `rpm-tools`. If that stage depended on `binary`, BuildKit
# would run cargo (build scripts and proc-macros of every crate in the sentinel's tree) in that job. So the stage must exist, start from a
# pinned image, copy nothing from another stage and run no build tool.

test_deny_a_missing_tools_stage if {
	refused(with_at(at_tools_from, ins(1, "from", [ubuntu, "AS", "signing-tools"], [])), "no stage named rpm-tools")
}

test_deny_a_tools_stage_that_extends_another_stage if {
	refused(with_at(at_tools_from, ins(1, "from", ["binary", "AS", "rpm-tools"], [])), "must start from a pinned image")
}

test_deny_a_tools_stage_that_copies_from_a_stage if {
	refused(appended(ins(1, "copy", ["/src/sentinel", "/build/"], ["--from=binary"])), "rpm-tools stage uses another stage")
}

test_deny_a_tools_stage_that_mounts_a_stage if {
	refused(appended(ins(1, "run", ["true"], ["--mount=type=bind,from=binary,source=/src,target=/src"])), "rpm-tools stage uses another stage")
}

test_deny_a_tools_stage_whose_mount_starts_with_from if {
	refused(appended(ins(1, "run", ["true"], ["--mount=from=binary,source=/src,target=/src"])), "rpm-tools stage uses another stage")
}

test_a_secret_mount_in_the_tools_stage_is_not_another_stage if {
	count(deny) == 0 with input as appended(ins(1, "run", ["true"], ["--mount=type=secret,id=build_ca,target=/etc/ssl/certs/ca-certificates.crt,required=false"]))
}

test_deny_a_tools_stage_that_runs_cargo if {
	refused(appended(ins(1, "run", ["cargo build --release --locked -p sentinel"], [])), "rpm-tools stage runs a build tool")
}

test_deny_a_tools_stage_that_runs_a_build_tool_after_a_pipe if {
	refused(appended(ins(1, "run", ["echo x | make"], [])), "rpm-tools stage runs a build tool")
}

test_deny_a_tools_stage_that_installs_rust_through_a_script if {
	refused(appended(ins(1, "run", ["rustup toolchain install stable"], [])), "rpm-tools stage runs a build tool")
}

test_the_rpm_stage_may_copy_the_binary if {
	count(deny) == 0 with input as good
}

test_a_build_tool_in_the_binary_stage_is_not_a_tools_stage_problem if {
	count(deny) == 0 with input as appended(ins(0, "run", ["cargo build --release --locked -p other"], []))
}

# The word in a text that only mentions it does not run it.
test_a_tools_stage_may_name_a_build_tool_in_a_message if {
	count(deny) == 0 with input as appended(ins(1, "run", ["echo this stage never runs cargo"], []))
}
