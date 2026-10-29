#!/usr/bin/env bash
# The hub and agent charts set only variables that the hub and the agent read (09/T3, T4; S16, S7).
#
#   deploy/tests/env-names.sh              check each chart and its values against the config.rs of its crate
#   deploy/tests/env-names.sh --self-test  the check must refuse a misspelt variable and a secret-looking one
#
# crates/hub/src/config.rs and the files of crates/hub/src/config/ (prompt 18 moved the settings of each group there), and
# crates/agent/src/config.rs, are where the binaries read their settings: every LK_* they know is a string literal there, so the
# names are found with grep. A variable in a chart that the binary does not read is a typo that turns a setting off silently, or a
# secret smuggled into the environment under a name nobody reads. The same lists exist in crates/hub/tests/hardening.rs and
# crates/agent/tests/hardening.rs for HARDENING.md; this is the check for the charts.
#
# The agent has two variables whose names look like secrets and are not: LK_CERT_SECRET and LK_JOIN_TOKEN_SECRET hold the NAME of a
# Secret that the agent reads through the Kubernetes API (crates/agent/RBAC.md). They are the only exceptions, they are named
# here, and secrets.rego checks their values.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# sources SPEC: the Rust files of SPEC, which is a colon separated list of files and directories (a directory contributes its *.rs
# files, one level down: a settings group is a file of crates/hub/src/config/, never a subdirectory).
sources() {
    local entry
    local -a entries
    IFS=: read -r -a entries <<<"$1"
    for entry in "${entries[@]}"; do
        if [ -d "$entry" ]; then
            find "$entry" -maxdepth 1 -type f -name '*.rs' | sort
        else
            printf '%s\n' "$entry"
        fi
    done
}

known_names() {
    local -a files
    mapfile -t files < <(sources "$1")
    [ "${#files[@]}" -gt 0 ] || return 0
    grep -hoE '"LK_[A-Z0-9_]+"' "${files[@]}" | tr -d '"' | sort -u
}

# check CONFIG PATH...: names under PATH... that CONFIG does not know, one per line, and secret-looking names. CONFIG is a file, a
# directory of files, or a colon separated list of both (see `sources`). The names in $secret_names (space separated) are the
# exceptions: they name a Secret and are not one.
secret_names=""
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
    secretish="$(grep -E '(PASSWORD|TOKEN|SECRET|CREDENTIAL|_KEY$)' <<<"$used" | grep -vxF -f <(tr ' ' '\n' <<<"$secret_names") || true)"
    if [ -n "$secretish" ]; then
        echo "env-names: these look like secrets in the environment (S7):" >&2
        while IFS= read -r name; do echo "  $name" >&2; done <<<"$secretish"
        status=1
    fi
    return "$status"
}

hub_paths=(deploy/charts/hub/templates deploy/charts/hub/values.yaml deploy/charts/hub/values.schema.json deploy/values/hub)
agent_paths=(deploy/charts/agent/templates deploy/charts/agent/values.yaml deploy/charts/agent/values.schema.json deploy/values/agent)

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
    # The hub's settings are in crates/hub/src/config.rs and in the files of crates/hub/src/config/: a name that only a file of the
    # directory knows is read by the hub, and a name that neither knows is a typo.
    mkdir "$tmp/config"
    printf 'let a = "LK_ENTRA_TENANT_ID";\n' >"$tmp/config/identity.rs"
    printf 'let b = "LK_NOT_A_SETTING";\n' >"$tmp/config/notes.txt"
    printf 'env:\n  - name: LK_WEB_ADDR\n  - name: LK_ENTRA_TENANT_ID\n' >"$tmp/group.yaml"
    printf 'env:\n  - name: LK_WEB_ADDR\n  - name: LK_NOT_A_SETTING\n' >"$tmp/not-a-setting.yaml"
    if check "$tmp/config.rs" "$tmp/group.yaml" 2>/dev/null; then
        echo "env-names self-test: a name that only the config directory knows was accepted when only config.rs was read" >&2
        exit 1
    fi
    check "$tmp/config.rs:$tmp/config" "$tmp/group.yaml" || {
        echo "env-names self-test: a name that a file of the config directory reads was refused" >&2
        exit 1
    }
    if check "$tmp/config.rs:$tmp/config" "$tmp/not-a-setting.yaml" 2>/dev/null; then
        echo "env-names self-test: a name that no file of the config directory reads was accepted" >&2
        exit 1
    fi
    # The agent's two Secret-name variables: allowed when named, and only those.
    printf 'let a = "LK_CERT_SECRET"; let b = "LK_HUB_TOKEN"; let c = "LK_SWIMLANE";\n' >"$tmp/config-agent.rs"
    printf 'env:\n  - name: LK_CERT_SECRET\n  - name: LK_SWIMLANE\n' >"$tmp/agent-good.yaml"
    printf 'env:\n  - name: LK_CERT_SECRET\n  - name: LK_HUB_TOKEN\n' >"$tmp/agent-token.yaml"
    if check "$tmp/config-agent.rs" "$tmp/agent-good.yaml" 2>/dev/null; then
        echo "env-names self-test: a variable named SECRET was accepted without being allowed" >&2
        exit 1
    fi
    secret_names="LK_CERT_SECRET LK_JOIN_TOKEN_SECRET"
    check "$tmp/config-agent.rs" "$tmp/agent-good.yaml" || {
        echo "env-names self-test: the agent's Secret-name variable was refused although it is allowed" >&2
        exit 1
    }
    if check "$tmp/config-agent.rs" "$tmp/agent-token.yaml" 2>/dev/null; then
        echo "env-names self-test: another secret-looking variable was accepted next to the allowed one" >&2
        exit 1
    fi
    echo "env-names self-test: a misspelt and a secret-looking variable are refused, and only the agent's two Secret names are exempt"
    exit 0
fi

check crates/hub/src/config.rs:crates/hub/src/config "${hub_paths[@]}"
echo "env-names: every LK_* variable of the hub chart is read by crates/hub/src/config.rs or crates/hub/src/config/*.rs"
secret_names="LK_CERT_SECRET LK_JOIN_TOKEN_SECRET"
check crates/agent/src/config.rs "${agent_paths[@]}"
echo "env-names: every LK_* variable of the agent chart is read by crates/agent/src/config.rs"
