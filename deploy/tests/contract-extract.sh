#!/usr/bin/env bash
# The YAML of a contract file, for conftest (09/T4; S16, S17).
#
#   deploy/tests/contract-extract.sh FILE.md OUT.yaml   write the fenced ```yaml blocks of FILE.md to OUT.yaml, one document stream
#   deploy/tests/contract-extract.sh --self-test        the extraction must keep the blocks and refuse a file it cannot read
#
# crates/agent/RBAC.md and crates/agent/HARDENING.md hold the manifests that the agent's own tests lint (crates/agent/tests), and the
# chart is checked against the same text (deploy/policy/contract.rego). The blocks are the ones fenced with exactly ```yaml; every
# other fence (shell, text, rust) is prose. This is the same reading as `documents()` in crates/agent/tests/hardening.rs.
#
# It stops with a message, and writes nothing, when the file is missing, has no yaml block, has a block that never closes, or has an
# empty one: a contract that extracts to nothing would make the comparison vacuously true.
set -euo pipefail

extract() {
    local md="$1" out="$2"
    [ -f "$md" ] || {
        echo "contract-extract: $md does not exist" >&2
        return 1
    }
    local tmp
    tmp="$(mktemp)"
    if ! awk -v file="$md" '
        BEGIN { open = 0; blocks = 0; lines = 0 }
        {
            line = $0
            sub(/[ \t\r]+$/, "", line)
        }
        open == 0 && line == "```yaml" { open = 1; start = NR; lines = 0; if (blocks > 0) print "---"; next }
        open == 1 && line == "```" {
            if (lines == 0) { printf "contract-extract: %s:%d: an empty yaml block\n", file, start > "/dev/stderr"; exit 3 }
            open = 0; blocks++; next
        }
        open == 1 { print $0; if ($0 !~ /^[ \t]*(#.*)?$/) lines++; next }
        END {
            if (open == 1) { printf "contract-extract: %s:%d: a yaml block that never closes\n", file, start > "/dev/stderr"; exit 4 }
            if (blocks == 0) { printf "contract-extract: %s has no yaml block\n", file > "/dev/stderr"; exit 5 }
        }
    ' "$md" >"$tmp"; then
        rm -f "$tmp"
        return 1
    fi
    mv "$tmp" "$out"
}

if [ "${1:-}" = "--self-test" ]; then
    dir="$(mktemp -d)"
    trap 'rm -rf "$dir"' EXIT
    fence='```'
    {
        printf 'Some prose.\n\n%s\n$ not yaml\n%s\n\n%syaml\nkind: Role\nmetadata: {name: a}\n---\nkind: RoleBinding\n%s\n\n' "$fence" "$fence" "$fence" "$fence"
        printf 'More prose, and a rust block:\n\n%srust\nfn main() {}\n%s\n\n%syaml\nkind: NetworkPolicy\n# a comment\nmetadata: {name: b}\n%s\n' "$fence" "$fence" "$fence" "$fence"
    } >"$dir/good.md"
    extract "$dir/good.md" "$dir/good.yaml" || {
        echo "contract-extract self-test: a good file was refused" >&2
        exit 1
    }
    [ "$(grep -c '^kind:' "$dir/good.yaml")" = 3 ] || {
        echo "contract-extract self-test: expected the 3 documents of the 2 yaml blocks" >&2
        cat "$dir/good.yaml" >&2
        exit 1
    }
    if grep -q -e 'not yaml' -e 'fn main' "$dir/good.yaml"; then
        echo "contract-extract self-test: a block that is not yaml was kept" >&2
        exit 1
    fi
    [ "$(grep -c '^---$' "$dir/good.yaml")" = 2 ] || {
        echo "contract-extract self-test: the blocks must be separated by one document marker, and the first block's own marker kept" >&2
        cat "$dir/good.yaml" >&2
        exit 1
    }
    refuse() { # NAME FILE: extraction must fail and leave no output
        local name="$1" file="$2"
        rm -f "$dir/out.yaml"
        if extract "$file" "$dir/out.yaml" 2>/dev/null; then
            echo "contract-extract self-test: $name was accepted" >&2
            exit 1
        fi
        [ ! -e "$dir/out.yaml" ] || {
            echo "contract-extract self-test: $name left a file behind" >&2
            exit 1
        }
    }
    printf 'prose only\n' >"$dir/none.md"
    refuse "a file without a yaml block" "$dir/none.md"
    # After a good block, so that only the unclosed one can be the reason.
    printf '%syaml\nkind: Role\n%s\n\n%syaml\nkind: RoleBinding\n' "$fence" "$fence" "$fence" >"$dir/open.md"
    refuse "a block that never closes" "$dir/open.md"
    printf '%syaml\n%s\n' "$fence" "$fence" >"$dir/empty.md"
    refuse "an empty block" "$dir/empty.md"
    printf '%syaml\n# only a comment\n\n%s\n' "$fence" "$fence" >"$dir/comment.md"
    refuse "a block with only a comment" "$dir/comment.md"
    refuse "a file that does not exist" "$dir/missing.md"
    # The real contract files extract to the documents the policy needs.
    cd "$(dirname "${BASH_SOURCE[0]}")/../.."
    extract crates/agent/RBAC.md "$dir/rbac.yaml"
    [ "$(grep -c '^kind: Role$' "$dir/rbac.yaml")" = 2 ] || {
        echo "contract-extract self-test: crates/agent/RBAC.md must hold 2 Roles" >&2
        exit 1
    }
    extract crates/agent/HARDENING.md "$dir/hardening.yaml"
    [ "$(grep -c -e '^kind: Pod$' -e '^kind: NetworkPolicy$' "$dir/hardening.yaml")" = 2 ] || {
        echo "contract-extract self-test: crates/agent/HARDENING.md must hold a Pod and a NetworkPolicy" >&2
        exit 1
    }
    echo "contract-extract self-test: yaml blocks are kept, other fences are not, and an unreadable contract is refused"
    exit 0
fi

[ "$#" -eq 2 ] || {
    echo "usage: deploy/tests/contract-extract.sh FILE.md OUT.yaml | --self-test" >&2
    exit 2
}
extract "$1" "$2"
