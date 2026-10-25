# Dockerfile policy (09/T1; S16, S19, S20). Run with `conftest test --parser dockerfile --namespace dockerfile`.
#
# conftest's Dockerfile parser gives an array of instructions: {Cmd (lower case), SubCmd, Flags, Value, JSON, Stage}, with
# continuation lines already joined and comments as `comment` entries. There is no original text, so messages are rebuilt
# from the parts (`shown`). The final stage is the one with the highest Stage number. `deploy/policy/testdata/dockerfile`
# holds planted Dockerfiles that `just images-policy-test` feeds through the real parser, so a rule written against the
# wrong shape fails there instead of passing silently.
#
# The same rules are linted per crate in crates/{hub,agent}/tests/hardening.rs; this policy is the one CI applies to every
# Dockerfile under the repository, so a new image cannot skip them.
package dockerfile

import rego.v1

flags(i) := f if {
	f := i.Flags
	is_array(f)
} else := []

shown(i) := concat(" ", array.concat(array.concat([upper(i.Cmd)], flags(i)), i.Value))

final_stage := max({i.Stage | some i in input})

final contains i if {
	some i in input
	i.Stage == final_stage
}

digest_pinned(ref) if regex.match(`^[a-z0-9.\-/:_]+@sha256:[0-9a-f]{64}$`, ref)

# Every base image is pinned by digest, including the build stages (S19). A stage built FROM another stage name would
# hide its base, so it is refused too: every stage names its pinned image.
deny contains msg if {
	some i in input
	i.Cmd == "from"
	not digest_pinned(i.Value[0])
	msg := sprintf("base image not pinned by digest: %s", [shown(i)])
}

# The runtime image is distroless, never a debug or floating variant (S16).
deny contains msg if {
	some i in final
	i.Cmd == "from"
	not startswith(i.Value[0], "gcr.io/distroless/")
	msg := sprintf("the final image is not distroless: %s", [i.Value[0]])
}

deny contains msg if {
	some i in final
	i.Cmd == "from"
	some bad in ["debug", ":latest"]
	contains(i.Value[0], bad)
	msg := sprintf("the final image is a debug or floating tag: %s", [i.Value[0]])
}

# A numeric, non-root USER in the final stage, so runAsNonRoot can be checked by the kubelet (S16).
final_users := [i | some i in final; i.Cmd == "user"]

deny contains "the final stage never sets USER" if count(final_users) == 0

numeric_nonroot(s) if {
	regex.match(`^[0-9]+$`, s)
	to_number(s) != 0
}

user_ok(v) if {
	parts := split(v, ":")
	count(parts) == 1
	numeric_nonroot(parts[0])
}

user_ok(v) if {
	parts := split(v, ":")
	count(parts) == 2
	numeric_nonroot(parts[0])
	numeric_nonroot(parts[1])
}

deny contains msg if {
	count(final_users) > 0
	last := final_users[count(final_users) - 1]
	not user_ok(last.Value[0])
	msg := sprintf("USER must be a numeric non-root uid and gid: %s", [shown(last)])
}

# Nothing runs, is fetched, or comes from the build context in the final stage: only files a build stage made (S16).
deny contains msg if {
	some i in final
	i.Cmd in {"run", "add"}
	msg := sprintf("the final stage runs or fetches something: %s", [shown(i)])
}

copies_from_stage(i) if {
	some f in flags(i)
	startswith(f, "--from=")
}

deny contains msg if {
	some i in final
	i.Cmd == "copy"
	not copies_from_stage(i)
	msg := sprintf("the final stage copies from the build context: %s", [shown(i)])
}

deny contains msg if {
	some i in final
	i.Cmd in {"env", "arg"}
	some marker in ["SECRET", "TOKEN", "PASSWORD", "KEY", "CREDENTIAL"]
	contains(upper(concat(" ", i.Value)), marker)
	msg := sprintf("a secret-looking variable in the image: %s", [shown(i)])
}

# The entrypoint is the binary itself, in exec form: no shell to interpret it (S16).
final_entrypoints := [i | some i in final; i.Cmd == "entrypoint"]

deny contains "no ENTRYPOINT in the final stage" if count(final_entrypoints) == 0

deny contains msg if {
	some i in final_entrypoints
	not i.JSON
	msg := sprintf("ENTRYPOINT must be the exec form: %s", [shown(i)])
}

# ADD can fetch URLs and unpack archives without a checksum; COPY and a checked fetch are used instead (S19).
deny contains msg if {
	some i in input
	i.Cmd == "add"
	msg := sprintf("ADD is not allowed, use COPY or a checked download: %s", [shown(i)])
}

run_text(i) := concat(" ", i.Value)

# Every Cargo build or install uses exactly the versions in a lockfile (S19).
deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	some cmd in ["cargo build", "cargo install", "cargo chef cook"]
	contains(text, cmd)
	not contains(text, "--locked")
	msg := sprintf("`%s` without --locked: %s", [cmd, shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	contains(text, "pnpm install")
	not contains(text, "--frozen-lockfile")
	msg := sprintf("`pnpm install` without --frozen-lockfile: %s", [shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd == "run"
	regex.match(`(curl|wget)[^|;&]*\|\s*(ba|da|z)?sh`, run_text(i))
	msg := sprintf("a download piped into a shell: %s", [shown(i)])
}

# A download in a build stage is checked against a pinned sha256 in the same instruction (S19, S20).
deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	regex.match(`(^|[^a-z])(curl|wget) `, text)
	not contains(text, "sha256sum -c")
	msg := sprintf("a download that is not checked with sha256sum -c: %s", [shown(i)])
}
