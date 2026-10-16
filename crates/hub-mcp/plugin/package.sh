#!/usr/bin/env bash
# Builds the Cursor plugin package of the Lanekeeper MCP server, for ONE authentication mode (plan 07, T4; D50, D52, A9).
#
#   package.sh <oauth|personal_token> <out-dir>
#
# The package is three things: .cursor-plugin/plugin.json, one mcp.json, and a copy of skills/lanekeeper/ (prompt 11's skill).
# The mode is chosen here and never combined: the mcp.json of a package has `auth` or `headers`, not both (D52).
#
# Settings come from the environment, never from the repository (no real host, tenant or client id is committed, AGENTS.md rule 12):
#
#   LK_PLUGIN_HUB_ORIGIN     both modes   the hub's public origin: https://host[:port] (http only for localhost or 127.0.0.1)
#   LK_PLUGIN_CLIENT_ID      oauth only   the Cursor public client's Application (client) ID, a GUID
#   LK_PLUGIN_API_AUDIENCE   oauth only   the hub API's Application ID URI, api://...; the scope is this plus /mcp.access
#   LK_PLUGIN_SKILL_DIR      tests only   a different skill folder (default: skills/lanekeeper/ of this repository)
#
# OAuth settings in a personal-token build are an error, not a hint. Every value is checked before anything is written, a refusal
# names the variable and never prints the value, and a build that fails leaves nothing behind. The out-dir must not exist or must
# be empty. Nothing here was checked against Cursor 3.22.12 (UNVERIFIED, see README.md).
set -euo pipefail
# bash 5.2 makes `&` special in a replacement; the checked values cannot hold one, and this keeps it that way.
shopt -u patsub_replacement 2>/dev/null || true

fail() {
    printf 'package.sh: %s\n' "$1" >&2
    exit 1
}

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"

[ "$#" -eq 2 ] || fail "usage: package.sh <oauth|personal_token> <out-dir>"
mode="$1"
out="$2"

case "$mode" in
    oauth) template="mcp.oauth.json" ;;
    personal_token) template="mcp.pat.json" ;;
    *) fail "the mode must be oauth or personal_token" ;;
esac
[ -n "$out" ] || fail "the out-dir is empty"

# ---- Check every setting before anything is written. ----

origin="${LK_PLUGIN_HUB_ORIGIN-}"
[ -n "$origin" ] || fail "LK_PLUGIN_HUB_ORIGIN is not set"
re_https='^https://[a-z0-9]([a-z0-9.-]{0,251}[a-z0-9])?(:[0-9]{1,5})?$'
re_loopback='^http://(localhost|127\.0\.0\.1)(:[0-9]{1,5})?$'
if ! [[ $origin =~ $re_https ]] && ! [[ $origin =~ $re_loopback ]]; then
    fail "LK_PLUGIN_HUB_ORIGIN is not valid: https://host or https://host:port in lower case, with no path, user or query"
fi

client_id=""
audience=""
if [ "$mode" = oauth ]; then
    client_id="${LK_PLUGIN_CLIENT_ID-}"
    audience="${LK_PLUGIN_API_AUDIENCE-}"
    [ -n "$client_id" ] || fail "LK_PLUGIN_CLIENT_ID is not set"
    [ -n "$audience" ] || fail "LK_PLUGIN_API_AUDIENCE is not set"
    re_guid='^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
    [[ $client_id =~ $re_guid ]] || fail "LK_PLUGIN_CLIENT_ID is not valid: it must be a GUID"
    re_audience='^api://[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*$'
    if ! [[ $audience =~ $re_audience ]] || [ "${#audience}" -gt 134 ]; then
        fail "LK_PLUGIN_API_AUDIENCE is not valid: api:// followed by the hub API's Application ID"
    fi
else
    [ -z "${LK_PLUGIN_CLIENT_ID-}" ] || fail "LK_PLUGIN_CLIENT_ID is for the oauth mode only: a personal-token package carries no sign-in settings (D52)"
    [ -z "${LK_PLUGIN_API_AUDIENCE-}" ] || fail "LK_PLUGIN_API_AUDIENCE is for the oauth mode only: a personal-token package carries no sign-in settings (D52)"
fi

skill="${LK_PLUGIN_SKILL_DIR:-$repo/skills/lanekeeper}"
[ -d "$skill" ] && [ ! -L "$skill" ] || fail "the skill folder is missing or is a symlink; there is no SKILL.md to package (prompt 11 owns skills/lanekeeper/)"
[ -f "$skill/SKILL.md" ] || fail "SKILL.md is missing from the skill folder (prompt 11 owns skills/lanekeeper/)"
# A link would carry a file from outside the repository into a package that is handed to the whole organisation.
if [ -n "$(find "$skill" -type l -print -quit)" ]; then
    fail "the skill folder holds a symlink; a package never carries one"
fi
if [ -n "$(find "$skill" ! -type f ! -type d -print -quit)" ]; then
    fail "the skill folder holds something that is neither a file nor a directory"
fi

[ -f "$here/$template" ] || fail "$template is missing from the plugin folder"
[ -f "$here/.cursor-plugin/plugin.json" ] || fail "the manifest .cursor-plugin/plugin.json is missing from the plugin folder"

if [ -e "$out" ] || [ -L "$out" ]; then
    { [ -d "$out" ] && [ ! -L "$out" ]; } || fail "the out-dir exists and is not a directory"
    [ -z "$(ls -A "$out")" ] || fail "the out-dir is not empty; the builder never writes into a directory that holds files"
fi

# ---- Build in a staging directory next to the target, then move it into place. ----

parent="$(dirname "$out")"
mkdir -p "$parent"
stage="$(mktemp -d "$parent/.package-stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

mkdir -p "$stage/.cursor-plugin" "$stage/skills"
cp "$here/.cursor-plugin/plugin.json" "$stage/.cursor-plugin/plugin.json"
cp -R "$skill" "$stage/skills/lanekeeper"

config="$(cat "$here/$template")"
config="${config//@HUB_ORIGIN@/$origin}"
config="${config//@CLIENT_ID@/$client_id}"
config="${config//@API_AUDIENCE@/$audience}"
re_left='@[A-Z_]+@'
if [[ $config =~ $re_left ]]; then
    fail "$template still holds an unfilled placeholder"
fi
# D52 again, on the result: one mode only, whatever the template says.
if [ "$mode" = oauth ]; then
    grep -qF '"auth"' <<<"$config" || fail "the oauth template has no auth block"
    ! grep -qF '"headers"' <<<"$config" || fail "the oauth template carries headers (D52)"
else
    grep -qF '"headers"' <<<"$config" || fail "the personal-token template has no headers"
    ! grep -qF '"auth"' <<<"$config" || fail "the personal-token template carries an auth block (D52)"
fi
printf '%s\n' "$config" >"$stage/mcp.json"

chmod -R u+rwX,go+rX,go-w "$stage"
chmod 755 "$stage"

if [ -d "$out" ]; then
    rmdir "$out"
fi
mv "$stage" "$out"
printf 'package.sh: built the %s package in %s\n' "$mode" "$out"
