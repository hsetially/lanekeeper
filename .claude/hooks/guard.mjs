#!/usr/bin/env node
// PreToolUse guard. Reads the hook JSON on stdin; exit 2 blocks the tool call and shows stderr to the agent.
// It acts only inside agent worktrees (branch agent/NN-slug), so human sessions are not affected.
// Limits: it checks Edit/Write-style tools and a few Bash patterns, not arbitrary shell writes.
// CI and the reviewer agent remain the backstop (prompts/REVIEW.md, section 1).

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const { decideBash, decideEdit } = await import(join(here, '..', '..', 'scripts', 'lk-lib.mjs'));

let input;
try {
  input = JSON.parse(readFileSync(0, 'utf8'));
} catch {
  process.exit(0); // never block on a malformed hook payload
}

const tool = input.tool_name;
const args = input.tool_input || {};
let verdict = { allow: true };

try {
  if (tool === 'Bash') {
    verdict = decideBash(args.command || '', input.cwd);
  } else if (['Edit', 'Write', 'MultiEdit', 'NotebookEdit'].includes(tool)) {
    const path = args.file_path || args.notebook_path;
    const content = [args.content, args.new_string, args.new_source, ...(args.edits || []).map((e) => e.new_string)]
      .filter(Boolean)
      .join('\n');
    if (path) verdict = decideEdit(path, content);
  }
} catch (err) {
  // A guard bug must not wedge a run. Surface it, then allow.
  console.error(`guard: internal error, allowing: ${err.message}`);
}

if (!verdict.allow) {
  console.error(`Blocked by the Lanekeeper guard: ${verdict.reason}`);
  process.exit(2);
}
