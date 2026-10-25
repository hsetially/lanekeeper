# Tests for dockerfile.rego: the good input passes, and each planted violation is refused by its own rule.
# Run with `conftest verify --policy deploy/policy`.
package dockerfile

import rego.v1

rust := "docker.io/library/rust:1.88.0-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0"

distroless := "gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f"

# The shape conftest's parser produces (see the header of dockerfile.rego). The last argument is a note for the reader.
ins(stage, cmd, value, flags, _) := {
	"Stage": stage, "Cmd": cmd, "SubCmd": "", "Value": value, "Flags": flags,
	"JSON": cmd == "entrypoint",
}

good := [
	ins(0, "from", [rust, "AS", "build"], [], concat(" ", ["FROM", rust, "AS build"])),
	ins(0, "run", ["cargo install --locked cargo-chef --version 0.1.73"], [], "RUN cargo install --locked cargo-chef"),
	ins(0, "run", ["cargo chef cook --release --locked -p agent --recipe-path recipe.json"], ["--mount=type=bind,from=planner"], "RUN cargo chef cook"),
	ins(0, "copy", [".", "."], [], "COPY . ."),
	ins(0, "run", ["cargo build --release --locked -p agent"], [], "RUN cargo build --release --locked -p agent"),
	ins(1, "from", [rust, "AS", "model"], [], "FROM rust AS model"),
	ins(1, "run", ["curl -fsS -o /models/m.onnx https://huggingface.co/x && echo \"$sha  /models/m.onnx\" | sha256sum -c -"], [], "RUN curl ... sha256sum -c"),
	ins(2, "from", [distroless], [], concat(" ", ["FROM", distroless])),
	ins(2, "copy", ["/src/target/release/agent", "/usr/local/bin/agent"], ["--from=build"], "COPY --from=build agent"),
	ins(2, "user", ["65532:65532"], [], "USER 65532:65532"),
	ins(2, "entrypoint", ["/usr/local/bin/agent"], [], "ENTRYPOINT [\"/usr/local/bin/agent\"]"),
]

# Replace the instruction at index n.
with_at(n, replacement) := array.concat(array.concat(array.slice(good, 0, n), [replacement]), array.slice(good, n + 1, count(good)))

without_at(n) := array.concat(array.slice(good, 0, n), array.slice(good, n + 1, count(good)))

appended(extra) := array.concat(good, [extra])

refused(planted, fragment) if {
	msgs := deny with input as planted
	some msg in msgs
	contains(msg, fragment)
}

test_good_passes if {
	count(deny) == 0 with input as good
}

test_deny_tag_only_base if {
	refused(with_at(0, ins(0, "from", ["docker.io/library/rust:1.88.0-bookworm", "AS", "build"], [], "FROM rust:1.88.0")), "not pinned by digest")
}

test_deny_stage_name_base if {
	refused(with_at(5, ins(1, "from", ["build", "AS", "model"], [], "FROM build AS model")), "not pinned by digest")
}

test_deny_unpinned_final if {
	refused(with_at(7, ins(2, "from", ["gcr.io/distroless/cc-debian12:nonroot"], [], "FROM distroless")), "not pinned by digest")
}

test_deny_non_distroless_final if {
	refused(with_at(7, ins(2, "from", [rust], [], "FROM rust")), "not distroless")
}

test_deny_debug_final if {
	debug := replace(distroless, ":nonroot", ":debug-nonroot")
	refused(with_at(7, ins(2, "from", [debug], [], "FROM debug")), "debug or floating")
}

test_deny_missing_user if {
	refused(without_at(9), "never sets USER")
}

test_deny_root_user if {
	refused(with_at(9, ins(2, "user", ["root"], [], "USER root")), "numeric non-root")
}

test_deny_uid_zero if {
	refused(with_at(9, ins(2, "user", ["0"], [], "USER 0")), "numeric non-root")
}

test_deny_gid_zero if {
	refused(with_at(9, ins(2, "user", ["65532:0"], [], "USER 65532:0")), "numeric non-root")
}

test_deny_run_in_final if {
	refused(appended(ins(2, "run", ["apt-get install -y curl"], [], "RUN apt-get install -y curl")), "runs or fetches")
}

test_deny_copy_from_context_in_final if {
	refused(appended(ins(2, "copy", ["deploy/images/models", "/app/models"], [], "COPY deploy/images/models /app/models")), "copies from the build context")
}

test_deny_secret_env_in_final if {
	refused(appended(ins(2, "env", ["GITHUB_TOKEN", "abc"], [], "ENV GITHUB_TOKEN=abc")), "secret-looking")
}

test_deny_missing_entrypoint if {
	refused(without_at(10), "no ENTRYPOINT")
}

test_deny_shell_entrypoint if {
	shell := object.union(good[10], {"JSON": false, "Value": ["sh -c /usr/local/bin/agent"]})
	refused(with_at(10, shell), "exec form")
}

test_deny_add if {
	refused(appended(ins(0, "add", ["https://example.com/x.tar.gz", "/x"], [], "ADD https://example.com/x.tar.gz /x")), "ADD is not allowed")
}

test_deny_unlocked_build if {
	refused(with_at(4, ins(0, "run", ["cargo build --release -p agent"], [], "RUN cargo build --release -p agent")), "without --locked")
}

test_deny_unlocked_install if {
	refused(with_at(1, ins(0, "run", ["cargo install cargo-chef"], [], "RUN cargo install cargo-chef")), "without --locked")
}

test_deny_unlocked_cook if {
	refused(with_at(2, ins(0, "run", ["cargo chef cook --release"], [], "RUN cargo chef cook")), "without --locked")
}

test_deny_unfrozen_pnpm if {
	refused(appended(ins(0, "run", ["pnpm install"], [], "RUN pnpm install")), "--frozen-lockfile")
}

test_deny_curl_pipe_shell if {
	refused(appended(ins(0, "run", ["curl -fsSL https://example.com/i.sh | sh && sha256sum -c x"], [], "RUN curl | sh")), "piped into a shell")
}

test_deny_unchecked_download if {
	refused(with_at(6, ins(1, "run", ["curl -fsS -o /models/m.onnx https://huggingface.co/x"], [], "RUN curl")), "not checked")
}
