#!/usr/bin/env node
// Structural rules for workflows that neither actionlint nor zizmor enforces (09/T2; S18, S19, S20).
//
//   node .github/scripts/check-workflows.mjs [PATH...]
//
// A PATH is a workflow file or a directory of them; with none, .github/workflows of the checkout this script lives in.
// No dependencies: it reads the YAML by indentation, which is enough for the few keys it looks at.
//
// Rules, each with the reason it exists:
//   W1  A top-level `permissions:` block exists and grants no write (`{}`, or scopes set to read or none). A job that
//       needs more asks for it itself, so the widest token is never the default for every job. `read-all` and
//       `write-all` are refused: the grant is written out.
//   W2  A job's own `permissions:` is not `read-all` or `write-all`.
//   W3  Every job has `timeout-minutes` (a number from 1 to 360, or an expression), unless it only calls a reusable
//       workflow. A job that hangs must not hold a runner for the default six hours.
//   W4  No `pull_request_target` trigger. It runs with a write token on code from a fork.
//   W5  A `pull_request` trigger names `paths` or `paths-ignore`, so a change to one area does not start every job.
//   W6  A job that holds `id-token: write` runs no build step. `cargo install` and `npm ci` run the build scripts, proc-macros and
//       install scripts of every dependency as the runner user, in a process whose environment carries the variables that let it
//       ask GitHub for an OIDC token; with that token the code is the job's identity to Sigstore, Google Cloud or whatever the
//       workflow federates with. So the job that signs or federates handles files that another job built, with tools fetched by a
//       digest or a hash. Refused, as a command in a `run:` step: cargo, rustc, rustdoc, npm, npx, pnpm, yarn, corepack, bun,
//       pip, pipx, poetry, go build/install/run/test/get/generate, gem install, bundle, mvn, gradle, dotnet build/restore/publish/
//       run/test, make, cmake, ninja; and the actions docker/build-push-action and docker/bake-action. NOT seen: what a script or a
//       `just` recipe called by a step does, and a `docker build` run by a script (that code runs in a container which is not given
//       the job's variables). Those are read by a person; the threat model says which jobs rely on that.
//
// Exit codes: 0 all rules hold; 1 a rule is broken (printed as file:line: reason); 2 nothing to check or an unreadable path.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const MAX_TIMEOUT = 360;

const stripComment = (s) => {
  const t = s.trim();
  if (t[0] === '"' || t[0] === "'") return t;
  const i = t.search(/\s#/);
  return (i >= 0 ? t.slice(0, i) : t).trim();
};

/** The lines that carry structure: [{ no (1-based), indent, text }], with blank lines and comments dropped. */
function structure(text) {
  const out = [];
  text.split(/\r?\n/).forEach((raw, i) => {
    const indent = raw.search(/\S/);
    if (indent < 0 || raw[indent] === '#') return;
    out.push({ no: i + 1, indent, text: raw.slice(indent) });
  });
  return out;
}

/** A `key: value` line as { key, value }, or null. */
function keyValue(text) {
  const m = /^([A-Za-z0-9_.-]+|"[^"]+"|'[^']+')\s*:(?:\s+(.*))?$/.exec(text);
  return m ? { key: m[1].replace(/^["']|["']$/g, ''), value: stripComment(m[2] ?? '') } : null;
}

/** The lines below `lines[at]` that are indented deeper than it. */
function bodyOf(lines, at) {
  const body = [];
  for (let i = at + 1; i < lines.length && lines[i].indent > lines[at].indent; i += 1) body.push(lines[i]);
  return body;
}

/** The direct children of a body: the lines at its smallest indent. */
function children(body) {
  if (body.length === 0) return [];
  const min = Math.min(...body.map((l) => l.indent));
  return body.filter((l) => l.indent === min);
}

// W6. A build tool, where a command can start: at the start of a line or after `&&`, `||`, `;`, `|`, `(`, `{`, a backtick, `$(`,
// `then`, `do` or `else`, past the words that only prefix a command (sudo, env and VAR=value, time, exec, nice, xargs). A word inside
// a string that echo prints, or an argument of grep, does not start a command.
const BUILD_TOOLS = String.raw`(?:cargo|rustc|rustdoc|npm|npx|pnpm|yarn|corepack|bun|pip3?|pipx|poetry|go\s+(?:build|install|run|test|get|generate)|gem\s+install|bundle|mvn|gradle|dotnet\s+(?:build|restore|publish|run|test)|make|cmake|ninja)`;
const COMMAND_START = String.raw`(?:^|&&|\|\||[;|({\`]|\$\(|\b(?:then|do|else)\b)\s*(?:(?:sudo|time|exec|command|nice|xargs|env)\s+(?:-\S+\s+)*|[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*`;
const BUILD_COMMAND = new RegExp(`${COMMAND_START}(${BUILD_TOOLS})(?=\\s|$)`);
const BUILD_ACTION = /^docker\/(?:build-push-action|bake-action)@/;

/** The steps of a job that run something, as [{ no, text }] per command line: each line of a `run:` value, or the action of a `uses:`. */
function stepCommands(lines, job) {
  const out = [];
  const body = bodyOf(lines, lines.indexOf(job));
  for (let i = 0; i < body.length; i += 1) {
    const dash = /^-\s+/.exec(body[i].text);
    const kv = keyValue(dash ? body[i].text.slice(dash[0].length) : body[i].text);
    if (!kv) continue;
    if (kv.key === 'uses' && BUILD_ACTION.test(kv.value)) out.push({ no: body[i].no, text: kv.value, action: true });
    if (kv.key !== 'run') continue;
    if (!/^[|>][+-]?\d*$/.test(kv.value)) {
      out.push({ no: body[i].no, text: kv.value });
      continue;
    }
    const keyIndent = body[i].indent + (dash ? dash[0].length : 0);
    for (let k = i + 1; k < body.length && body[k].indent > keyIndent; k += 1) out.push({ no: body[k].no, text: body[k].text });
  }
  return out;
}

/** Does the job's `permissions:` grant id-token: write (a mapping, a flow mapping, or write-all)? */
function holdsIdToken(lines, perms) {
  const { value } = keyValue(perms.text);
  if (value === 'write-all') return true;
  if (value !== '') return /(^|[{,\s])id-token\s*:\s*write\b/.test(value);
  return children(bodyOf(lines, lines.indexOf(perms))).some((l) => {
    const kv = keyValue(l.text);
    return kv?.key === 'id-token' && kv.value === 'write';
  });
}

/**
 * @param {string} text
 * @returns {{ line: number, message: string }[]}
 */
export function checkWorkflow(text) {
  const lines = structure(text);
  const found = [];
  const add = (line, message) => found.push({ line, message });
  const at = (key) => lines.findIndex((l) => l.indent === 0 && keyValue(l.text)?.key === key);

  // W1: the top-level permissions.
  const p = at('permissions');
  if (p < 0) {
    add(1, 'no top-level permissions: block; write `permissions: {}` and grant each job what it needs (W1)');
  } else {
    const { value } = keyValue(lines[p].text);
    if (value === 'write-all' || value === 'read-all') {
      add(lines[p].no, `permissions: ${value} is too wide; write out the scopes, or {} (W1)`);
    } else if (value !== '' && value !== '{}') {
      add(lines[p].no, `top-level permissions: ${value} is not {} or a mapping of scopes (W1)`);
    }
    for (const l of bodyOf(lines, p)) {
      const kv = keyValue(l.text);
      if (kv && kv.value === 'write') add(l.no, `permissions: ${kv.key}: write at the top level; grant a write in the job that needs it (W1)`);
    }
  }

  // W4 and W5: the triggers.
  const o = at('on');
  if (o >= 0) {
    const { value } = keyValue(lines[o].text);
    const body = bodyOf(lines, o);
    for (const l of [lines[o], ...body]) {
      if (/\bpull_request_target\b/.test(l.text)) add(l.no, 'the pull_request_target trigger runs a write token on code from a fork (W4)');
    }
    if (value !== '') {
      if (/\bpull_request\b/.test(value)) add(lines[o].no, 'pull_request without paths: write the trigger as a mapping with `paths:` (W5)');
    } else {
      for (const c of children(body)) {
        const kv = keyValue(c.text);
        if (!kv || kv.key !== 'pull_request') continue;
        const inner = bodyOf(lines, lines.indexOf(c));
        const hasPaths = children(inner).some((l) => ['paths', 'paths-ignore'].includes(keyValue(l.text)?.key)) || /paths/.test(kv.value);
        if (!hasPaths) add(c.no, 'pull_request without paths or paths-ignore; limit the trigger to what the workflow checks (W5)');
      }
    }
  }

  // W2 and W3: the jobs.
  const j = at('jobs');
  if (j >= 0) {
    for (const job of children(bodyOf(lines, j))) {
      const kv = keyValue(job.text);
      if (!kv) continue;
      const own = children(bodyOf(lines, lines.indexOf(job)));
      const get = (key) => own.find((l) => keyValue(l.text)?.key === key);
      const perms = get('permissions');
      if (perms) {
        const v = keyValue(perms.text).value;
        if (v === 'write-all' || v === 'read-all') add(perms.no, `job ${kv.key}: permissions: ${v} is too wide; write out the scopes (W2)`);
      }
      if (perms && holdsIdToken(lines, perms)) {
        for (const c of stepCommands(lines, job)) {
          const hit = c.action ? c.text : BUILD_COMMAND.exec(c.text)?.[1];
          if (!hit) continue;
          const what = c.action ? `uses ${hit}` : `runs \`${c.text.length > 90 ? `${c.text.slice(0, 90)}...` : c.text}\``;
          add(c.no, `job ${kv.key} holds id-token: write and ${what}; a build step in the job that can ask for an OIDC token lets a dependency's code ask for one too. Build in a job without the token and hand the files on (W6)`);
        }
      }
      if (get('uses')) continue; // a call of a reusable workflow: the timeout belongs to the called jobs
      const t = get('timeout-minutes');
      if (!t) {
        add(job.no, `job ${kv.key} has no timeout-minutes; a hung job would hold its runner for hours (W3)`);
        continue;
      }
      const v = keyValue(t.text).value;
      if (/^\$\{\{.*\}\}$/.test(v)) continue;
      const n = Number(v);
      if (!Number.isInteger(n) || n < 1 || n > MAX_TIMEOUT) add(t.no, `job ${kv.key}: timeout-minutes: ${v} must be a whole number from 1 to ${MAX_TIMEOUT} (W3)`);
    }
  }
  return found.sort((a, b) => a.line - b.line);
}

function* workflowFiles(path) {
  if (statSync(path).isFile()) {
    yield path;
    return;
  }
  for (const entry of readdirSync(path, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const full = join(path, entry.name);
    if (entry.isDirectory()) yield* workflowFiles(full);
    else if (entry.isFile() && /\.ya?ml$/.test(entry.name)) yield full;
  }
}

function main(argv) {
  const repo = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
  const targets = argv.length > 0 ? argv : [join(repo, '.github', 'workflows')];
  const files = [];
  for (const t of targets) {
    if (!existsSync(t)) {
      console.error(`check-workflows: ${t} does not exist`);
      return 2;
    }
    files.push(...workflowFiles(t));
  }
  if (files.length === 0) {
    console.error(`check-workflows: no workflow files under ${targets.join(', ')}; refusing to pass on nothing`);
    return 2;
  }
  let bad = 0;
  for (const file of files) {
    const rel = relative(process.cwd(), file);
    for (const v of checkWorkflow(readFileSync(file, 'utf8'))) {
      console.log(`${rel.startsWith('..') ? file : rel}:${v.line}: ${v.message}`);
      bad += 1;
    }
  }
  if (bad > 0) {
    console.log(`check-workflows: ${bad} problems in ${files.length} workflows`);
    return 1;
  }
  console.log(`check-workflows: ${files.length} workflows checked, all follow the rules`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
