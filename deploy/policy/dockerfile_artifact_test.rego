# Tests for dockerfile_artifact.rego (09/T8; S19, S20): the good input passes, and each planted violation is refused by its own rule.
# Run with `conftest verify --policy deploy/policy`.
package dockerfile_artifact

import rego.v1

rust := "docker.io/library/rust:1.88.0-trixie@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db"

ubuntu := "docker.io/library/ubuntu:noble-20260930.1@sha256:68ee22004a9109b44973313b02ff7b1de250f1de0f5ecb628d73f579aaff33ac"

# The shape conftest's parser produces (see the header of dockerfile.rego).
ins(stage, cmd, value, flags) := {"Stage": stage, "Cmd": cmd, "SubCmd": "", "Value": value, "Flags": flags, "JSON": false}

good := [
	ins(0, "from", [rust, "AS", "binary"], []),
	ins(0, "run", ["curl -fsSL -o /tmp/std.tar.xz https://static.rust-lang.org/dist/x.tar.xz && echo \"$sha  /tmp/std.tar.xz\" | sha256sum -c -"], []),
	ins(0, "copy", [".", "."], []),
	ins(0, "run", ["cargo build --release --locked --target x86_64-unknown-linux-musl -p sentinel"], []),
	ins(1, "from", [ubuntu, "AS", "rpm"], []),
	ins(1, "run", ["apt-get update && apt-get install --yes --no-install-recommends rpm=4.18.2+dfsg-2.1build2 gpg"], []),
	ins(1, "copy", ["/src/sentinel", "/build/SOURCES/"], ["--from=binary"]),
	ins(1, "run", ["rpmbuild -bb /build/SPECS/lanekeeper-sentinel.spec"], []),
	ins(2, "from", ["scratch", "AS", "out"], []),
	ins(2, "copy", ["/src/sentinel", "/lanekeeper-sentinel"], ["--from=binary"]),
	ins(2, "copy", ["/build/RPMS/x86_64/", "/"], ["--from=rpm"]),
]

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
	refused(with_at(4, ins(1, "from", ["docker.io/library/ubuntu:latest", "AS", "rpm"], [])), "not pinned by digest")
}

test_deny_stage_name_as_base if {
	refused(with_at(4, ins(1, "from", ["binary", "AS", "rpm"], [])), "not pinned by digest")
}

# --- the export stage: files made by earlier stages, and nothing else

test_deny_a_final_stage_that_is_an_image if {
	refused(with_at(8, ins(2, "from", [ubuntu, "AS", "out"], [])), "the final stage must be FROM scratch")
}

test_deny_an_unpinned_image_as_the_final_stage if {
	refused(with_at(8, ins(2, "from", ["docker.io/library/ubuntu:latest", "AS", "out"], [])), "not pinned by digest")
}

test_deny_a_final_stage_that_runs_something if {
	refused(appended(ins(2, "run", ["echo hello"], [])), "the final stage may only copy")
}

test_deny_a_final_stage_that_copies_the_context if {
	refused(appended(ins(2, "copy", ["secrets.txt", "/"], [])), "the final stage copies from the build context")
}

test_deny_a_final_stage_with_an_entrypoint if {
	refused(appended(ins(2, "entrypoint", ["/lanekeeper-sentinel"], [])), "the final stage may only copy")
}

test_deny_a_final_stage_that_adds if {
	refused(appended(ins(2, "add", ["https://example.com/x", "/x"], [])), "the final stage may only copy")
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
	refused(with_at(5, ins(1, "run", ["apt-get update && apt-get install --yes rpm=4.18.2+dfsg-2.1build2"], [])), "--no-install-recommends")
}

test_deny_unpinned_rpm if {
	refused(with_at(5, ins(1, "run", ["apt-get update && apt-get install --yes --no-install-recommends rpm gpg"], [])), "rpm must be pinned")
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
