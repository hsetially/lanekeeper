// Shared helpers for the agent automation: prompt metadata parsing, plan status,
// ownership matching. Used by scripts/lk.mjs and .claude/hooks/guard.mjs.
// Pure Node, no dependencies, so it runs on Windows, macOS and Linux.

import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { basename, dirname, join, relative, resolve, sep } from 'node:path';

export const PROMPT_ID = /^\d{2}[ab]?$/;
export const AGENT_BRANCH = /^agent\/(\d{2}[ab]?)-([a-z0-9-]+)$/;

// Files every prompt may edit: workspace dependency declarations and the per-prompt gate stubs.
// Parallel agents will conflict here; the reviewer checks that edits are append-only.
export const SHARED_FILES = ['Cargo.toml', 'Cargo.lock', 'Justfile'];

// Never edited by an agent unless the plan lists them as extra paths.
export const PROTECTED = ['.claude/', 'AGENTS.md', 'CLAUDE.md', 'prompts/', 'design/handoff/'];

// Contract paths (AGENTS.md, Ownership). Only prompt 01 or an approved contract-change may touch them.
export const CONTRACT_PATHS = [
  'crates/domain/',
  'crates/ports/',
  'proto/',
  'api/openapi.yaml',
  'db/migrations/',
  'docs/mcp-tools.md',
  'perf/budgets.toml',
  'design/handoff/',
];

export function git(cwd, ...args) {
  return execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

export function repoRoot(from) {
  return git(from, 'rev-parse', '--show-toplevel');
}

export function promptFiles(root) {
  const dir = join(root, 'prompts');
  return readdirSync(dir)
    .map((f) => /^(\d{2}[ab]?)-([a-z0-9-]+)\.md$/.exec(f))
    .filter(Boolean)
    .map((m) => ({ id: m[1], slug: `${m[1]}-${m[2]}`, file: join('prompts', m[0]) }))
    .sort((a, b) => a.id.localeCompare(b.id));
}

function metaRow(text, key) {
  const m = new RegExp(`^\\|\\s*\\*\\*${key}\\*\\*\\s*\\|\\s*(.*?)\\s*\\|\\s*$`, 'm').exec(text);
  return m ? m[1] : '';
}

const ticks = (s) => [...s.matchAll(/`([^`]+)`/g)].map((m) => m[1]);

export function parsePrompt(root, id) {
  const entry = promptFiles(root).find((p) => p.id === id);
  if (!entry) throw new Error(`no prompt ${id} in prompts/`);
  const text = readFileSync(join(root, entry.file), 'utf8');
  const title = (/^#\s+\d{2}[ab]?:\s*(.+)$/m.exec(text) || [])[1] || entry.slug;

  const waveCell = metaRow(text, 'Wave');
  const waveNum = /^(\d+)/.exec(waveCell);
  const wave = waveNum ? Number(waveNum[1]) : /^v2/i.test(waveCell) ? 'v2' : 'investigation';

  // "You own": backticked paths. A sentence marked "is read-only" lists read-only paths instead.
  const ownCell = metaRow(text, 'You own');
  const owns = [];
  const readOnly = [];
  for (const sentence of ownCell.split(/\.\s+(?=`)/)) {
    const target = /read-only/i.test(sentence) ? readOnly : owns;
    target.push(...ticks(sentence));
  }
  if (/workspace root files/i.test(ownCell)) owns.push('@root-files');

  const gate = ticks(metaRow(text, 'Gate'))[0] || null;
  const tasks = [...text.matchAll(/^###\s+(T\d+[a-z]?):\s*(.+)$/gm)].map((m) => ({ id: m[1], title: m[2] }));

  return {
    id,
    slug: entry.slug,
    file: entry.file,
    title,
    wave,
    dependsRaw: metaRow(text, 'Depends on'),
    owns,
    readOnly,
    gate,
    tasks,
    stack: stackOf(owns),
    branch: `agent/${entry.slug}`,
    plan: `plans/${entry.slug}.md`,
  };
}

export function stackOf(owns) {
  const web = owns.some((o) => o.startsWith('web/'));
  const rust = owns.some((o) => /^(crates|fuzz|perf|proto)\b/.test(o) || o === '@root-files');
  if (web && !rust) return 'web';
  if (rust) return 'rust';
  return 'ops';
}

// Returns { ids: [prompt ids that must be merged], manual: [conditions a human must confirm] }.
export function parseDepends(root, info) {
  const raw = info.dependsRaw;
  const manual = [];
  if (/^nothing/i.test(raw)) return { ids: [], manual };
  if (/everything in v1|^v1/i.test(raw)) {
    const ids = promptFiles(root)
      .map((p) => parsePrompt(root, p.id))
      .filter((p) => typeof p.wave === 'number' && p.wave <= 2)
      .map((p) => p.id);
    for (const q of raw.match(/Q\d+[^;.]*/g) || []) manual.push(q.trim());
    return { ids, manual };
  }
  if (/^access to/i.test(raw)) return { ids: [], manual: [raw] };
  const first = raw.replace(/`[^`]*`/g, '').split(/\.\s/)[0];
  const ids = [...first.matchAll(/\b(\d{2}[ab]?)\b/g)].map((m) => m[1]);
  if (/handoff/i.test(first)) manual.push('design handoff present in design/handoff/ (needed for the screens task)');
  return { ids: [...new Set(ids)], manual };
}

// A dependency counts as merged when `base` has a commit whose subject starts "NN:" or "NN/Tk:".
export function mergedIds(root, base) {
  let subjects = '';
  try {
    subjects = git(root, 'log', base, '--format=%s');
  } catch {
    return new Set();
  }
  const found = new Set();
  for (const s of subjects.split('\n')) {
    const m = /^(\d{2}[ab]?)(\/T\d+[a-z]?)?:/.exec(s);
    if (m) found.add(m[1]);
  }
  return found;
}

// ---- plan status ------------------------------------------------------------------------

export function planFile(root, id) {
  const entry = promptFiles(root).find((p) => p.id === id);
  return entry ? join(root, 'plans', `${entry.slug}.md`) : null;
}

export function readPlan(root, id) {
  const f = planFile(root, id);
  if (!f || !existsSync(f)) return { exists: false, approved: false, contractChange: false, extra: [] };
  const text = readFileSync(f, 'utf8');
  const head = text.split(/^## /m)[0];
  const extraLine = /^Extra-paths:[ \t]*(.*)$/m.exec(head);
  return {
    exists: true,
    approved: /^Status:\s*APPROVED\s*$/m.test(head),
    contractChange: /^Contract-change:\s*APPROVED\s*$/m.test(head),
    extra: extraLine ? ticks(extraLine[1]) : [],
  };
}

// ---- path rules -------------------------------------------------------------------------

function globToRegex(g) {
  let out = '';
  for (let i = 0; i < g.length; i++) {
    const c = g[i];
    if (c === '*' && g[i + 1] === '*') {
      out += '.*';
      i++;
    } else if (c === '*') out += '[^/]*';
    else out += c.replace(/[.+?^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`^${out}$`);
}

// Does repo-relative `path` (posix separators) fall under pattern `pat`?
export function matches(pat, path) {
  if (pat === '@root-files') return !path.includes('/');
  if (pat === 'CODEOWNERS') return path === '.github/CODEOWNERS';
  if (pat === 'Dockerfiles') return /(^|\/)Dockerfile[^/]*$/.test(path);
  if (/[*]/.test(pat)) {
    if (pat.endsWith('/**')) {
      const dir = pat.slice(0, -3);
      return path === dir || path.startsWith(`${dir}/`);
    }
    return globToRegex(pat).test(path);
  }
  if (pat.endsWith('/')) return path.startsWith(pat);
  if (!/\.[A-Za-z0-9]+$/.test(basename(pat))) return path === pat || path.startsWith(`${pat}/`); // bare directory
  return path === pat;
}

const anyMatch = (pats, path) => pats.some((p) => matches(p, path));

// Tests, benches and fixtures that live under an owned directory are always allowed.
function underOwnedTestDir(owns, path) {
  return owns.some((o) => {
    const dir = o.replace(/\/\*\*$/, '').replace(/\/$/, '');
    return !/[*.]/.test(basename(dir)) && path.startsWith(`${dir}/`) && /(^|\/)(tests?|benches|fixtures|testdata)\//.test(path);
  });
}

/**
 * Decide whether an edit to `absPath` is allowed. Returns { allow: true } or { allow: false, reason }.
 * Applies only inside agent worktrees (branch agent/NN-slug); everything else is allowed.
 */
export function decideEdit(absPath, newContent = '') {
  let dir = dirname(resolve(absPath));
  while (!existsSync(dir) && dirname(dir) !== dir) dir = dirname(dir);
  let root;
  let branch;
  try {
    root = repoRoot(dir);
    branch = git(root, 'rev-parse', '--abbrev-ref', 'HEAD');
  } catch {
    return { allow: true };
  }
  const m = AGENT_BRANCH.exec(branch);
  if (!m) return { allow: true };

  const id = m[1];
  const info = parsePrompt(root, id);
  const rel = relative(root, resolve(absPath)).split(sep).join('/');
  const plan = readPlan(root, id);
  const planRel = info.plan;

  if (rel.startsWith('..')) return { allow: false, reason: `${rel} is outside the worktree for prompt ${id}.` };

  if (rel === planRel) {
    if (plan.approved) {
      return { allow: false, reason: `plan ${planRel} is approved and frozen. Record problems in plans/${info.slug}.blockers.md and stop.` };
    }
    if (/^(Status|Contract-change):\s*APPROVED/m.test(newContent)) {
      return { allow: false, reason: 'only a human can approve a plan (the orchestrator runs `node scripts/lk.mjs approve`). Leave Status: DRAFT.' };
    }
    return { allow: true };
  }

  if (!plan.approved) {
    return { allow: false, reason: `plan ${planRel} is not approved yet. Until a human approves it, edit only ${planRel}.` };
  }

  if (rel.startsWith(`plans/${info.slug}.`)) return { allow: true }; // evidence, blockers, review

  if (anyMatch(info.readOnly, rel)) return { allow: false, reason: `${rel} is read-only for prompt ${id}.` };

  const extraOk = anyMatch(plan.extra, rel);
  const contract = CONTRACT_PATHS.some((p) => matches(p, rel));
  if (contract && id !== '01' && !extraOk && !plan.contractChange) {
    return { allow: false, reason: `${rel} is a contract path. Use the lanekeeper-contract-change skill: record the need in the plan (Contract-change) and escalate to a human.` };
  }
  if (anyMatch(PROTECTED, rel) && !extraOk && !anyMatch(info.owns, rel)) {
    return { allow: false, reason: `${rel} is protected (agent instructions, prompts or the design handoff). Escalate instead of editing it.` };
  }

  if (anyMatch(info.owns, rel) || SHARED_FILES.includes(rel) || extraOk || (contract && plan.contractChange) || underOwnedTestDir(info.owns, rel)) {
    return { allow: true };
  }
  return {
    allow: false,
    reason: `${rel} is not owned by prompt ${id} (owns: ${info.owns.join(', ') || 'nothing'}). Stop and escalate: write the need into plans/${info.slug}.blockers.md. A human can add it to the plan's Extra-paths.`,
  };
}

const FORCE_PUSH = /\bgit\b[^|;&]*\bpush\b[^|;&]*(--force\b|--force-with-lease\b|\s-f\b)/;
const PUSH_MAIN = /\bgit\b[^|;&]*\bpush\b[^|;&]*\b(main|master)\b/;

export function decideBash(command, cwd) {
  let branch = '';
  try {
    branch = git(cwd || '.', 'rev-parse', '--abbrev-ref', 'HEAD');
  } catch {
    return { allow: true };
  }
  if (!AGENT_BRANCH.test(branch)) return { allow: true };
  if (/--no-verify\b/.test(command)) return { allow: false, reason: 'never skip hooks or gates (--no-verify).' };
  if (FORCE_PUSH.test(command)) return { allow: false, reason: 'agents never force-push.' };
  if (PUSH_MAIN.test(command)) return { allow: false, reason: 'agents push only their own agent/* branch.' };
  return { allow: true };
}
