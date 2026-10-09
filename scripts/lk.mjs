#!/usr/bin/env node
// lk: deterministic helpers behind the /run-prompt skill. Prints JSON so agents can parse it.
//
//   node scripts/lk.mjs list [--wave N]
//   node scripts/lk.mjs info NN
//   node scripts/lk.mjs deps NN [--base REF]
//   node scripts/lk.mjs worktree NN [--base REF]
//   node scripts/lk.mjs approve NN [--contract-change]
//   node scripts/lk.mjs status

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  PROMPT_ID,
  git,
  mergedIds,
  parseDepends,
  parsePrompt,
  planFile,
  promptFiles,
  readPlan,
} from './lk-lib.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const out = (o) => console.log(JSON.stringify(o, null, 2));
const fail = (msg) => {
  console.error(`lk: ${msg}`);
  process.exit(1);
};

const [cmd, ...rest] = process.argv.slice(2);
const flag = (name) => {
  const i = rest.indexOf(name);
  return i >= 0 ? rest[i + 1] : undefined;
};
const has = (name) => rest.includes(name);
const idArg = () => {
  const id = rest.find((a) => PROMPT_ID.test(a));
  if (!id) fail('give a prompt id such as 04 or 03a');
  return id;
};
const baseRef = () => flag('--base') || git(ROOT, 'rev-parse', '--abbrev-ref', 'HEAD');
const worktreePath = (info) => join(ROOT, '.worktrees', info.slug);

function summary(info) {
  return { id: info.id, title: info.title, wave: info.wave, stack: info.stack, tasks: info.tasks.length };
}

switch (cmd) {
  case 'list': {
    const wave = flag('--wave');
    const rows = promptFiles(ROOT)
      .map((p) => parsePrompt(ROOT, p.id))
      .filter((p) => wave === undefined || String(p.wave) === wave);
    out(rows.map(summary));
    break;
  }
  case 'info': {
    const info = parsePrompt(ROOT, idArg());
    out({ ...info, depends: parseDepends(ROOT, info), worktree: worktreePath(info) });
    break;
  }
  case 'deps': {
    const info = parsePrompt(ROOT, idArg());
    const deps = parseDepends(ROOT, info);
    const merged = mergedIds(ROOT, baseRef());
    const unmet = deps.ids.filter((d) => !merged.has(d));
    out({ id: info.id, ok: unmet.length === 0, unmet, manual: deps.manual, base: baseRef() });
    break;
  }
  case 'worktree': {
    const info = parsePrompt(ROOT, idArg());
    if (typeof info.wave !== 'number' && !has('--force')) {
      fail(`prompt ${info.id} is not a build prompt (wave: ${info.wave}). Run it by hand, or pass --force.`);
    }
    const path = worktreePath(info);
    let reused = existsSync(path);
    if (!reused) {
      mkdirSync(dirname(path), { recursive: true });
      const branchExists = git(ROOT, 'branch', '--list', info.branch) !== '';
      if (branchExists) git(ROOT, 'worktree', 'add', path, info.branch);
      else git(ROOT, 'worktree', 'add', '-b', info.branch, path, baseRef());
    }
    mkdirSync(join(path, 'plans'), { recursive: true });
    out({ id: info.id, branch: info.branch, worktree: path, plan: join(path, info.plan), reused });
    break;
  }
  case 'approve': {
    const info = parsePrompt(ROOT, idArg());
    const wt = worktreePath(info);
    const file = planFile(wt, info.id);
    if (!existsSync(wt) || !existsSync(file)) fail(`no plan at ${file}. Run the planner first.`);
    let text = readFileSync(file, 'utf8');
    if (!/^Status:\s*DRAFT\s*$/m.test(text)) fail('plan header must contain "Status: DRAFT" to approve it.');
    text = text.replace(/^Status:\s*DRAFT\s*$/m, 'Status: APPROVED');
    if (has('--contract-change')) {
      text = /^Contract-change:.*$/m.test(text)
        ? text.replace(/^Contract-change:.*$/m, 'Contract-change: APPROVED')
        : text.replace(/^Status: APPROVED$/m, 'Status: APPROVED\nContract-change: APPROVED');
    }
    writeFileSync(file, text);
    git(wt, 'add', info.plan);
    git(wt, 'commit', '-m', `${info.id}/plan: approved by a human`);
    out({ id: info.id, approved: true, plan: readPlan(wt, info.id) });
    break;
  }
  case 'status': {
    const merged = mergedIds(ROOT, baseRef());
    const rows = promptFiles(ROOT).map((p) => {
      const info = parsePrompt(ROOT, p.id);
      const wt = worktreePath(info);
      const deps = parseDepends(ROOT, info);
      const plan = existsSync(wt) ? readPlan(wt, info.id) : null;
      let commits = 0;
      if (existsSync(wt)) {
        try {
          commits = git(wt, 'log', `${baseRef()}..HEAD`, '--format=%s').split('\n').filter((s) => s.startsWith(`${info.id}/T`)).length;
        } catch {
          commits = 0;
        }
      }
      return {
        id: info.id,
        wave: info.wave,
        merged: merged.has(info.id),
        ready: deps.ids.every((d) => merged.has(d)),
        plan: plan ? (plan.approved ? 'approved' : plan.exists ? 'draft' : 'none') : '-',
        taskCommits: `${commits}/${info.tasks.length}`,
      };
    });
    out(rows);
    break;
  }
  default:
    fail('commands: list | info NN | deps NN | worktree NN | approve NN | status');
}
