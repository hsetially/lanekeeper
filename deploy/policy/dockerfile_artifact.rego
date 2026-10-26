# Policy for a Dockerfile that makes files and not an image to run (09/T8; S19, S20): deploy/sentinel/Dockerfile.build builds the sentinel
# binary and its RPM, and exports them with its last stage, which is `FROM scratch` and copies. Run with
# `conftest test --parser dockerfile --namespace dockerfile_artifact`.
#
# dockerfile.rego is the policy of the images that run in a cluster (distroless, a numeric user, an entrypoint); none of those make sense
# for an export, and loosening them there would let a runtime image skip them by ending in `FROM scratch`. So this is a second policy, for
# the Dockerfiles named in the Justfile (`sentinel-check`), with the rules that hold for every build stage: bases by digest, locked
# builds, checked downloads, no ADD, and packages from a signed archive. The shape of the input is in dockerfile.rego's header.
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

# Every stage names its pinned base, except the final one, which is the empty image.
deny contains msg if {
	some i in input
	i.Cmd == "from"
	i.Stage != final_stage
	not digest_pinned(i.Value[0])
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
# release pocket, which does not change.
deny contains msg if {
	some i in input
	i.Cmd == "run"
	text := run_text(i)
	contains(text, "apt-get install")
	not regex.match(`(^|\s)rpm=[0-9][^\s]*`, text)
	regex.match(`(^|\s)rpm(\s|$)`, text)
	msg := sprintf("rpm must be pinned to a version (rpm=<version>): %s", [shown(i)])
}

deny contains msg if {
	some i in input
	i.Cmd in {"env", "arg"}
	some marker in ["SECRET", "TOKEN", "PASSWORD", "KEY", "CREDENTIAL"]
	contains(upper(concat(" ", i.Value)), marker)
	msg := sprintf("a secret-looking variable in a build stage: %s", [shown(i)])
}
