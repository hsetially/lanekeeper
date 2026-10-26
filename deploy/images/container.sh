# shellcheck shell=bash
# What the scripts that run a tool in a container have in common (09, CI first run; S19, S20). Source it, after `set -euo pipefail` and
# after tools.env:
#
#   . "$here/container.sh"
#
# Why it exists. The images of grype and syft are built from nothing: their /tmp is owned by root and not writable by anyone else, and
# they have no home directory. A script that starts them as the user who runs it (`--user "$(id -u):$(id -g)"`, so that the files in a
# mounted directory belong to that user) works in a root shell, and fails on a CI runner, whose user is `runner` (uid 1001):
#
#   grype:  unable to create listing temp file: open /tmp/grype-db-listing...: permission denied
#   syft:   oci-archive: mkdir /tmp/stereoscope-...: permission denied      (and `mkdir /.cache: permission denied`)
#
# And syft was run with --quiet, which hides that message too, so the step ended with exit code 1 and no line of output.
#
# lk_container_run LABEL DOCKER-RUN-ARGS... IMAGE [ARGS...]
#     `docker run --rm` as the calling user, with a /tmp and a HOME that this user owns: a directory made for the run (mktemp -d, so 0700
#     and the caller's) that is mounted at /tmp, removed when the container ends, and named by TMPDIR and HOME. It is on the host's disk, not
#     in memory, because an image archive is unpacked there. The container gets no capability and no mount that it did not have, and the
#     image's own files stay as they are. When the container exits with a code other than 0, one more line says which tool, with what
#     code and as whom; the lines above it are the tool's own messages, and docker's (the codes 125 to 127 are docker's, not the tool's).
#     A scan that finds a critical vulnerability exits 1 as well, and its table is above that line.
# lk_syft IN_DIR SOURCE OUT_DIR OUT_NAME [DOCKER-RUN-ARGS...]
#     syft (LK_SYFT_IMAGE) reads SOURCE, a syft source whose files are in IN_DIR (mounted read-only at /in; `oci-archive:/in/x.tar`,
#     `dir:/in`), and writes CycloneDX JSON to OUT_DIR/OUT_NAME (OUT_DIR is mounted at /out and must belong to the caller). It is never
#     quiet: its errors are what explains a failure.
# lk_explain_failures
#     A command that fails under `set -e` ends the script; this makes the script say which command, on which line, with what exit code,
#     on standard error, so that no failure is silent (the tools that print nothing, `cargo ... --quiet`, are the reason).

lk_container_run() {
    local label="$1" scratch uid gid status=0
    shift
    uid="$(id -u)"
    gid="$(id -g)"
    scratch="$(mktemp -d)"
    docker run --rm --user "$uid:$gid" -e HOME=/tmp -e TMPDIR=/tmp -v "$scratch:/tmp" "$@" || status=$?
    rm -rf "$scratch"
    if [ "$status" -ne 0 ]; then
        echo "${0##*/}: $label exited with code $status, run as uid $uid with a /tmp and a HOME that uid owns; the lines above it are the tool's own messages" >&2
    fi
    return "$status"
}

lk_syft() {
    local in_dir="$1" source="$2" out_dir="$3" out_name="$4"
    shift 4
    lk_container_run syft -e SYFT_CHECK_FOR_APP_UPDATE=false "$@" \
        -v "$in_dir:/in:ro" -v "$out_dir:/out" "${LK_SYFT_IMAGE:?tools.env is not sourced}" \
        "$source" --output "cyclonedx-json=/out/$out_name"
}

lk_explain_failures() {
    set -E
    # shellcheck disable=SC2154 # `status` is set inside the trap's own string
    trap 'status=$?; echo "${0##*/}: line $LINENO: \`$BASH_COMMAND\` failed with exit code $status" >&2' ERR
}
