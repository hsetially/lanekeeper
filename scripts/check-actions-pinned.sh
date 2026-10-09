#!/usr/bin/env bash
# S19: every GitHub Action must be pinned to a full commit SHA.
set -euo pipefail
bad=$(grep -RhoE 'uses:\s*[^ #]+' .github/workflows | sed -E 's/uses:\s*//' | grep -vE '^\./' | grep -vE '@[0-9a-f]{40}$' || true)
if [ -n "$bad" ]; then
  echo "Actions not pinned by commit SHA:"; echo "$bad"; exit 1
fi
echo "All actions pinned by SHA."
