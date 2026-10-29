# Policy for a Dockerfile that makes files and not an image to run (09/T8; S19, S20): deploy/sentinel/Dockerfile.build builds the sentinel
# binary and its RPM, and exports them with its last stage, which is `FROM scratch` and copies. Run with
# `conftest test --parser dockerfile --namespace dockerfile_artifact`.
#
# dockerfile.rego is the policy of the images that run in a cluster (distroless, a numeric user, an entrypoint); none of those make sense
# for an export, and loosening them there would let a runtime image skip them by ending in `FROM scratch`. So this is a second policy, for
# the Dockerfiles named in the Justfile (`sentinel-check`), with the rules that hold for every build stage: bases by digest, locked
# builds, checked downloads, no ADD, and packages from a signed archive. The shape of the input is in dockerfile.rego's header.
#
# The tools stage. deploy/sentinel/Dockerfile.build has a stage named `rpm-tools` (rpm and gpg, and nothing of ours), which the job that
# signs a release builds. That job holds the Workload Identity credential and the release key, so building the stage must not compile
# the sentinel: BuildKit builds a stage's whole dependency graph, so one `COPY --from=binary` (or a base that is another stage, or a
# `--mount=from=`) would bring in `cargo build` and the build scripts of every crate. The rules below keep `rpm-tools` a root of the graph
# that runs no build tool. The same fact is checked on BuildKit's own graph by deploy/sentinel/tests/tools-graph.sh.
package dockerfile_artifact

import rego.v1

flags(i) := f if {
	f := i.Flags
	is_array(f)
} else := []

shown(i) := concat(" ", array.concat(array.concat([upper(i.Cmd)], flags(i)), i.Value))

run_text(i) := concat(" ", i.Value)

final_stage := max({i.Stage | some i in input})

final contains i if {
	some i in input
	i.Stage == final_stage
}

digest_pinned(ref) if regex.match(`^[a-z0-9.\-/:_]+@sha256:[0-9a-f]{64}$`, ref)

# --- the bases (S19) ------------------------------------------------------------------------------------------------------------------

# `FROM <name> AS <alias>`: the stage's alias, in lower case (Docker compares names without regard to case).
stage_name(i) := lower(i.Value[2]) if {
	i.Cmd == "from"
	count(i.Value) >= 3
	lower(i.Value[1]) == "as"
}

# The stage that `i` (a FROM) names as its base, when it is a stage defined EARLIER in the file. Its own base is then checked by this
# same rule, so every chain ends in an image named by digest.
extends_earlier_stage(i) if {
	some j in input
	j.Cmd == "from"
	j.Stage < i.Stage
	stage_name(j) == lower(i.Value[0])
}

# Every stage names its pinned base (an image by digest, or an earlier stage of this file), except the final one, which is the empty image.
deny contains msg if {
	some i in input
	i.Cmd == "from"
	i.Stage != final_stage
	not digest_pinned(i.Value[0])
	not extends_earlier_stage(i)
	msg := sprintf("base image not pinned by digest: %s", [shown(i)])
}

deny contains msg if {
	some i in final
	i.Cmd == "from"
	i.Value[0] != "scratch"
	not digest_pinned(i.Value[0])
	msg := sprintf("base image not pinned by digest: %s", [shown(i)])
}

# --- the export stage: files that earlier stages made --------------------------------------------------------------------------------

deny contains msg if {
	some i in final
	i.Cmd == "from"
	i.Value[0] != "scratch"
	msg := sprintf("the final stage must be FROM scratch, an export of files and not an image: %s", [shown(i)])
}

deny contains msg if {
	some i in final
	not i.Cmd in {"from", "copy"}
	msg := sprintf("the final stage may only copy files that earlier stages made: %s", [shown(i)])
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

# --- builds, downloads and packages (S19) ----------------------------------------------------------------------------------------------

# ADD can fetch URLs and unpack archives without a checksum.
deny contains msg if {
	some i in input
	i.Cmd == "add"
	msg := sprintf("ADD is not allowed, use COPY or a checked download: %s", [shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	some cmd in ["cargo build", "cargo install"]
	contains(text, cmd)
	not contains(text, "--locked")
	msg := sprintf("`%s` without --locked: %s", [cmd, shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd == "run"
	regex.match(`(curl|wget)[^|;&]*\|\s*(ba|da|z)?sh`, run_text(i))
	msg := sprintf("a download piped into a shell: %s", [shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	regex.match(`(^|[^a-z])(curl|wget) `, text)
	not contains(text, "sha256sum -c")
	msg := sprintf("a download that is not checked with sha256sum -c: %s", [shown(i)])
}

# Packages come from the distribution's signed archive (apt checks its signatures), only what is named, and nothing is upgraded behind
# the pinned base image's back.
deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	contains(text, "apt-get install")
	not contains(text, "--no-install-recommends")
	msg := sprintf("apt-get install without --no-install-recommends, which would add packages nobody named: %s", [shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	regex.match(`apt-get (dist-)?upgrade`, text)
	msg := sprintf("apt-get upgrade or dist-upgrade changes the pinned base image's packages: %s", [shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	some flag in ["--allow-unauthenticated", "--force-yes", "--allow-insecure-repositories", "apt-key add"]
	contains(text, flag)
	msg := sprintf("%s lets unauthenticated packages in: %s", [flag, shown(i)])
}

# rpm is the tool that builds the package, so its version is part of what the package is: pinned to the version of the archive's
# release pocket, which does not change. gpg and its agent are the tools that sign: their upstream version is pinned, with a glob over
# the archive's revision (`gpg=2.4.4-2ubuntu17*`), because the archive drops a superseded revision from its index and an exact pin would
# break the build at each security update, while taking the update is what a signing tool should do. The build prints the exact versions.
pinned_tools := {"rpm", "gpg", "gpg-agent"}

deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	contains(text, "apt-get install")
	some tool in pinned_tools
	regex.match(sprintf(`(^|\s)['"]?%s['"]?(\s|$)`, [tool]), text)
	msg := sprintf("%s must be pinned to a version (%s=<version>): %s", [tool, tool, shown(i)])
}

# --- the tools stage: what the signing jobs build (S7, S19, S20) -----------------------------------------------------------------------

tools_stage := "rpm-tools"

tools_stage_from contains i if {
	some i in input
	i.Cmd == "from"
	stage_name(i) == tools_stage
}

in_tools_stage(i) if {
	some f in tools_stage_from
	i.Stage == f.Stage
}

deny contains msg if {
	count(tools_stage_from) == 0
	msg := "no stage named rpm-tools: the jobs that sign build this stage, which holds rpm and gpg and no build of ours"
}

# A root of the graph: an image by digest, not another stage.
deny contains msg if {
	some i in tools_stage_from
	not digest_pinned(i.Value[0])
	msg := sprintf("the rpm-tools stage must start from a pinned image, not from another stage, so that building it builds nothing else: %s", [shown(i)])
}

# No instruction of the stage reads another stage: COPY --from, or a RUN --mount=...from=<stage>.
uses_other_stage(i) if {
	some f in flags(i)
	regex.match(`^--(from=|mount=(.*,)?from=)`, f)
}

deny contains msg if {
	some i in input
	in_tools_stage(i)
	uses_other_stage(i)
	msg := sprintf("the rpm-tools stage uses another stage, which BuildKit would build first: %s", [shown(i)])
}

# A build tool where a command can start: at the start of the text or after `&&`, `||`, `;`, `|`, `(`, a backtick (\x60) or `$(`, past sudo and
# variable assignments. A word in a message that echo prints does not start a command.
build_tool_command := `(^|&&|\|\||[;|(\x60]|\$\()\s*((sudo|env|exec|time|nice)\s+|[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*(cargo|rustc|rustdoc|rustup|npm|npx|pnpm|yarn|pip3?|pipx|poetry|go|make|cmake|ninja|gcc|cc|g\+\+)(\s|$)`

deny contains msg if {
	some i in input
	in_tools_stage(i)
	i.Cmd == "run"
	regex.match(build_tool_command, run_text(i))
	msg := sprintf("the rpm-tools stage runs a build tool: %s", [shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd in {"env", "arg"}
	some marker in ["SECRET", "TOKEN", "PASSWORD", "KEY", "CREDENTIAL"]
	contains(upper(concat(" ", i.Value)), marker)
	msg := sprintf("a secret-looking variable in a build stage: %s", [shown(i)])
}
