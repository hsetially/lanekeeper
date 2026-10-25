#!/usr/bin/env bash
# The hub chart sets only variables that the hub reads (09/T3; S16, S7).
#
#   deploy/tests/env-names.sh              check the chart and its values against crates/hub/src/config.rs
#   deploy/tests/env-names.sh --self-test  the check must refuse a misspelt variable and a secret-looking one
#
# crates/hub/src/config.rs is where the hub reads its settings: every LK_* it knows is a string literal there, so the names are
# found with grep. A variable in the chart that the hub does not read is a typo that turns a setting off silently, or a secret
# smuggled into the environment under a name nobody reads. The same lists exist in crates/hub/tests/hardening.rs for
# HARDENING.md; this is the check for the chart.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

known_names() { grep -oE '"LK_[A-Z0-9_]+"' "$1" | tr -d '"' | sort -u; }

# check CONFIG_RS PATH...: names under PATH... that CONFIG_RS does not know, one per line, and secret-looking names.
check() {
    local config="$1"
    shift
    local known used unknown secretish status=0
    known="$(known_names "$config")"
    [ -n "$known" ] || {
        echo "env-names: no LK_* names found in $config" >&2
        return 1
    }
    used="$(grep -rhoE 'LK_[A-Z0-9_]+' "$@" | sort -u)"
    [ -n "$used" ] || {
        echo "env-names: no LK_* names found in the chart or its values" >&2
        return 1
    }
    unknown="$(comm -13 <(printf '%s\n' "$known") <(printf '%s\n' "$used"))"
    if [ -n "$unknown" ]; then
        echo "env-names: the chart names variables that $config does not read:" >&2
        while IFS= read -r name; do echo "  $name" >&2; done <<<"$unknown"
        status=1
    fi
    secretish="$(grep -E '(PASSWORD|TOKEN|SECRET|CREDENTIAL|_KEY$)' <<<"$used" || true)"
    if [ -n "$secretish" ]; then
        echo "env-names: these look like secrets in the environment (S7):" >&2
        while IFS= read -r name; do echo "  $name" >&2; done <<<"$secretish"
        status=1
    fi
    return "$status"
}

paths=(deploy/charts/hub/templates deploy/charts/hub/values.yaml deploy/charts/hub/values.schema.json deploy/values/hub)

if [ "${1:-}" = "--self-test" ]; then
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    printf 'let a = "LK_WEB_ADDR"; let b = "LK_OPS_ADDR";\n' >"$tmp/config.rs"
    printf 'env:\n  - name: LK_WEB_ADDR\n  - name: LK_OPS_ADDR\n' >"$tmp/good.yaml"
    printf 'env:\n  - name: LK_WEB_ADDR\n  - name: LK_WEB_ADRR\n' >"$tmp/typo.yaml"
    printf 'env:\n  - name: LK_WEB_ADDR\n' >"$tmp/secret.yaml"
    printf 'let a = "LK_WEB_ADDR"; let b = "LK_GIT_TOKEN";\n' >"$tmp/config-secret.rs"
    printf 'env:\n  - name: LK_GIT_TOKEN\n' >"$tmp/secret-env.yaml"
    check "$tmp/config.rs" "$tmp/good.yaml" || {
        echo "env-names self-test: the good chart was refused" >&2
        exit 1
    }
    if check "$tmp/config.rs" "$tmp/typo.yaml" 2>/dev/null; then
        echo "env-names self-test: a misspelt variable was accepted" >&2
        exit 1
    fi
    if check "$tmp/config-secret.rs" "$tmp/secret-env.yaml" 2>/dev/null; then
        echo "env-names self-test: a secret-looking variable was accepted" >&2
        exit 1
    fi
    echo "env-names self-test: a misspelt and a secret-looking variable are refused"
    exit 0
fi

check crates/hub/src/config.rs "${paths[@]}"
echo "env-names: every LK_* variable of the hub chart is read by crates/hub/src/config.rs"
