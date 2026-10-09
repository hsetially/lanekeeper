# 13: v2 features

| | |
|---|---|
| **Wave** | v2 |
| **Depends on** | v1 |
| **Rule** | Before an agent starts on any feature here, expand it into a full prompt using the standard template: metadata table, tasks with Verify commands, the S# and P# it touches, and a gate. |

These are short specs. Expand each one into a full prompt, using the same structure as the v1 prompts, when it gets scheduled.

## Decision model

The decision model (`vllm-sr/Decision-2.0-Kai-0.6B`) runs on users' machines through Ollama, and the skill uses it optionally.

- Check first that Ollama can run its custom decision head.
- Feed it the exported drift labels (below).
- The hub never depends on it.

## Drift-from-template check

Applies if `sitN` branches are created from `templates/*` (Q22). Report changes on the template branch that haven't reached each tenant branch, like C1.

## Swimlane claims

Informational claims on a swimlane, with an alert when the swimlane's tenant suffix changes.

## Base tag creation

Admins create tags from the UI, using their own token. Audited.

## Drift label export

Export drift resolutions as JSONL.

## Release snapshots

A snapshot names a base tag together with tenant commits. Users can compare a swimlane against it.

## Wider scope

Cover `data/atm-config` and the `data/*.json` files.

## Pre-copy warning

The sync Job asks the agent which files are at risk before it overwrites them.

## Surgical setting edits

`set_setting(swimlane, path, setting_path, value)` uses tree-sitter locations to change one value in place, keeping comments, formatting and CRLF line endings intact.

This makes AI edits safer than replacing the whole file's content, especially in very large files such as `security-roles.yml`, which runs to over 6,000 lines.

## Multi-replica hub and tool-driven sync

Route agent requests across hub replicas. Then let the agent apply Git to NFS, which retires the Job's copy step and leaves one copier.
