#!/usr/bin/env bash
# Writes deploy/images/hub-model.lock from one Hugging Face commit (09/T1, S19). Run it where huggingface.co is reachable,
# review the diff, and commit it; the image build then fetches exactly these bytes or fails.
#
#   deploy/images/update-model-lock.sh <revision> [path ...]
#
# <revision> may be a branch such as `main`: it is resolved to the full commit, and only the commit is written. The model
# directory and the repo are kept from the current lock. The default paths are the files fastembed's user-defined model
# loads: the ONNX graph and the four tokenizer files.
#
# Every file is downloaded and hashed here; for files stored in LFS the hash is also compared with the one Hugging Face
# reports, so a mismatch between the two stops the update. The checker (model-lock.mjs) runs last and refuses a revision
# younger than 14 days.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
lock="$here/hub-model.lock"
[ $# -ge 1 ] || { echo "usage: $0 <revision> [path ...]" >&2; exit 2; }
want="$1"
shift
paths=("$@")
[ ${#paths[@]} -gt 0 ] || paths=(onnx/model.onnx tokenizer.json config.json special_tokens_map.json tokenizer_config.json)

model="$(awk '$1 == "model" { print $2 }' "$lock")"
repo="$(awk '$1 == "repo" { print $2 }' "$lock")"
[[ "$repo" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || { echo "bad repo in $lock: $repo" >&2; exit 1; }
[[ "$want" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "bad revision: $want" >&2; exit 1; }

api="https://huggingface.co/api/models/$repo"
fetch() { curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --retry 3 --max-time 900 "$@"; }

info="$(fetch "$api/revision/$want")"
revision="$(jq -r '.sha' <<<"$info")"
date="$(jq -r '.lastModified' <<<"$info" | sed -E 's/\.[0-9]+Z$/Z/')"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || { echo "Hugging Face did not return a commit for $want" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
{
    # Keep the header comments, drop the UNRESOLVED note and every value line.
    awk '/^# UNRESOLVED:/ { skip = 1 } skip && /^#/ { next } { skip = 0 } /^#/ { print }' "$lock"
    echo "model $model"
    echo "repo $repo"
    echo "revision $revision"
    echo "date $date"
    for path in "${paths[@]}"; do
        [[ "$path" =~ ^([A-Za-z0-9_-][A-Za-z0-9._-]*/)*[A-Za-z0-9_-][A-Za-z0-9._-]*$ ]] || { echo "bad path: $path" >&2; exit 1; }
        fetch --output "$tmp/file" "https://huggingface.co/$repo/resolve/$revision/$path"
        sha="$(sha256sum "$tmp/file" | cut -d' ' -f1)"
        size="$(stat -c %s "$tmp/file")"
        dir="$(dirname "$path")"
        [ "$dir" = "." ] && dir=""
        lfs="$(fetch "$api/tree/$revision/$dir" | jq -r --arg p "$path" '.[] | select(.path == $p) | .lfs.oid // empty')"
        if [ -n "$lfs" ] && [ "$lfs" != "$sha" ]; then
            echo "$path: downloaded sha256 $sha, Hugging Face reports $lfs" >&2
            exit 1
        fi
        echo "file $sha $size $path"
    done
} > "$tmp/lock"
mv "$tmp/lock" "$lock"
node "$here/model-lock.mjs" check "$lock"
