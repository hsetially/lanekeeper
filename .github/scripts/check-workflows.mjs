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
//       digest or a hash. Values are read as YAML reads them (`id-token: "write"`, `run: 'cargo install x'`), and a command is
//       looked for where one can start (the start of a line or after `&&`, `||`, `;`, `|`, `(`, `$(`, then/do/else, past sudo, env,
//       timeout, time, nice, xargs and VAR=value). A line that ends in `\` is joined with the next. Refused, as a command in a `run:`:
//         - cargo, rustc, rustdoc, rustup, npm, npx, pnpm, yarn, corepack, bun, bunx, uv, uvx, pip, pip3, pipx, poetry, conda, mamba,
//           `python -m pip|pipx|build|poetry|uv|venv|ensurepip`, `python setup.py`, go build/install/run/test/get/generate, gem install,
//           bundle, mvn, gradle, dotnet build/restore/publish/run/test, make, cmake, ninja, and apt, apt-get, dnf, yum, zypper, brew
//           install and apk add (the maintainer scripts of a package run as the runner's root);
//         - the same inside `sh -c`, `bash -c`, `dash`, `zsh` or `ksh -c '...'` (also `-ec`, `-lc`, `-o pipefail -c`), to a depth of 3;
//         - `docker run|exec|create` with one of those tools on its command line (`docker run IMAGE cargo build`), and
//           `docker build`, `docker buildx build|bake`, `docker image build`, `docker builder build`, `docker compose build`;
//         - a download that is run: `curl|wget ... | sh|bash|python|perl|ruby|node`, `bash <(curl ...)`, `eval|source|. "$(curl ...)"`,
//           `sh -c "$(curl ...)"`;
//         and, as a step, the actions docker/build-push-action and docker/bake-action.
//       NOT SEEN, and read by a person (the threat model lists each): what a script, a `just` recipe or a `make` target called by a
//       step does (`./build.sh`, `deploy/sentinel/build.sh`, `just build` start no command that this rule knows); the steps of a local
//       composite action (`uses: ./.github/actions/...`; the file is not opened) and of a reusable workflow; a tool that a step
//       installs and then runs under a name this list does not hold (a binary from a download that is checked, an `npx`-free
//       `node script.js`, `python script.py`, `deno run`, `java -jar`); a command assembled in a variable or by `eval` of a string that
//       is not a download; a quoted `run:` that spans several lines; and a multi-line value of `sh -c` whose command starts on a line
//       after the first. A tool behind `rustup run stable cargo` is seen because rustup itself is refused. A `docker build` that a
//       script runs is not seen (that build runs in a container which is given none of the job's variables; sentinel.yml's
//       `sign-release` is the one case, and its tools stage is held by dockerfile_artifact.rego and tools-graph.sh instead).
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

/**
 * The text of a quoted YAML scalar at the start of `s` and what follows its closing quote, or null when `s` does not start with one.
 * Double quotes: a backslash takes the next character as it is. Single quotes: `''` is one quote.
 */
function unquote(s) {
  const q = s[0];
  if (q !== '"' && q !== "'") return null;
  let text = '';
  for (let i = 1; i < s.length; i += 1) {
    const c = s[i];
    if (q === '"' && c === '\\' && i + 1 < s.length) {
      text += s[i + 1];
      i += 1;
    } else if (c === q && q === "'" && s[i + 1] === "'") {
      text += "'";
      i += 1;
    } else if (c === q) {
      return { text, rest: s.slice(i + 1) };
    } else {
      text += c;
    }
  }
  return null;
}

/**
 * A `key: value` line as { key, value, quoted }, or null. A quoted value is the text inside its quotes, as YAML reads it (`"write"` and
 * `write` are one grant; `'cargo install x'` and `cargo install x` are one command), with `quoted` set so that `"|"` is not a block scalar.
 */
function keyValue(text) {
  const m = /^([A-Za-z0-9_.-]+|"[^"]+"|'[^']+')\s*:(?:\s+(.*))?$/.exec(text);
  if (!m) return null;
  const key = m[1].replace(/^["']|["']$/g, '');
  const raw = (m[2] ?? '').trim();
  const q = unquote(raw);
  if (q && /^\s*(#.*)?$/.test(q.rest)) return { key, value: q.text.trim(), quoted: true };
  return { key, value: stripComment(raw), quoted: false };
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

// W6. Where a command can start: at the start of a line or after `&&`, `||`, `;`, `|`, `(`, `{`, a backtick, `$(`, `then`, `do` or `else`,
// past the words that only prefix a command (sudo, env and VAR=value, time, exec, nice, xargs, timeout N). A word inside a string that
// echo prints, or an argument of grep, does not start a command.
const BUILD_TOOLS = String.raw`(?:cargo|rustc|rustdoc|rustup|npm|npx|pnpm|yarn|corepack|bunx?|uvx?|pip3?|pipx|poetry|conda|mamba|python3?\s+-m\s+(?:pip|pipx|build|poetry|uv|venv|ensurepip)|python3?\s+(?:\S+/)?setup\.py|go\s+(?:build|install|run|test|get|generate)|gem\s+install|bundle|mvn|gradle|dotnet\s+(?:build|restore|publish|run|test)|make|cmake|ninja|(?:apt|apt-get|dnf|yum|zypper|brew)\s+install|apk\s+add)`;
const COMMAND_START = String.raw`(?:^|&&|\|\||[;|({\`]|\$\(|\b(?:then|do|else)\b)\s*(?:(?:sudo|time|exec|command|nice|xargs|env)\s+(?:-\S+\s+)*|timeout\s+(?:-\S+\s+)*\d+[smhd]?\s+|[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*`;
const BUILD_COMMAND = new RegExp(`${COMMAND_START}(${BUILD_TOOLS})(?=\\s|$)`);
const BUILD_ACTION = /^docker\/(?:build-push-action|bake-action)@/;
// A shell that is given its command as a string: `bash -c '...'`, `sh -ec "..."`, `bash -o pipefail -c '...'`. Group 1 is a double-quoted
// string, group 2 a single-quoted one.
const SHELL_C = new RegExp(String.raw`${COMMAND_START}(?:ba|da|z|k)?sh\s+(?:(?:-o\s+\S+|-\S+)\s+)*-[A-Za-z]*c[A-Za-z]*\s+(?:--\s+)?(?:"((?:\\.|[^"\\])*)"|'([^']*)')`, 'g');
// docker run IMAGE cargo build: the tool is a word of the command line (or the first word of a quoted string, as in `sh -c 'cargo build'`).
const DOCKER_RUN = new RegExp(String.raw`${COMMAND_START}docker\s+(?:container\s+)?(?:run|exec|create)\s[^|;&\n]*?[\s'"](${BUILD_TOOLS})(?=[\s'"]|$)`);
const DOCKER_BUILD = new RegExp(String.raw`${COMMAND_START}docker\s+(?:buildx\s+(?:build|bake)|compose\s+build|(?:image\s+|builder\s+)?build)(?=\s|$)`);
// Code that is downloaded and run without being kept and checked.
const INTERPRETER = String.raw`(?:(?:ba|da|z|k)?sh|python3?|perl|ruby|node)`;
const FETCH_AND_RUN = [
  new RegExp(String.raw`${COMMAND_START}(?:curl|wget)(?=\s)[^|;&\n]*\|\s*(?:sudo\s+(?:-\S+\s+)*)?${INTERPRETER}(?=\s|$)`),
  new RegExp(String.raw`${COMMAND_START}${INTERPRETER}\s+(?:-\S+\s+)*<\(\s*(?:curl|wget)(?=\s)`),
  new RegExp(String.raw`${COMMAND_START}(?:eval|source|\.)\s+"?(?:\$\(|<\()\s*(?:curl|wget)(?=\s)`),
];
const FETCHED_SUBSTITUTION = /^\s*"?\$\(\s*(?:curl|wget)(?=\s)/;
const MAX_SHELL_DEPTH = 3;

/** What this command line starts that W6 refuses, as a few words, or null. `depth` counts the shells whose strings were opened. */
export function refusedCommand(text, depth = 0) {
  const build = BUILD_COMMAND.exec(text);
  if (build) return build[1];
  if (DOCKER_BUILD.test(text)) return 'docker build';
  const run = DOCKER_RUN.exec(text);
  if (run) return `docker run ${run[1]}`;
  if (FETCH_AND_RUN.some((re) => re.test(text))) return 'a download that is run';
  if (depth < MAX_SHELL_DEPTH) {
    for (const sh of text.matchAll(SHELL_C)) {
      const inner = sh[1] !== undefined ? sh[1].replace(/\\(.)/g, '$1') : sh[2];
      const hit = refusedCommand(inner, depth + 1) ?? (FETCHED_SUBSTITUTION.test(inner) ? 'a download that is run' : null);
      if (hit) return hit;
    }
  }
  return null;
}

/**
 * The steps of a job that run something, as [{ no, text }] per command line: each logical line of a `run:` value (a line that ends in a
 * backslash is joined with the next, and `no` is the first line's), a quoted or one-line `run:`, or the action of a `uses:`.
 */
function stepCommands(lines, job) {
  const out = [];
  const body = bodyOf(lines, lines.indexOf(job));
  for (let i = 0; i < body.length; i += 1) {
    const dash = /^-\s+/.exec(body[i].text);
    const kv = keyValue(dash ? body[i].text.slice(dash[0].length) : body[i].text);
    if (!kv) continue;
    if (kv.key === 'uses' && BUILD_ACTION.test(kv.value)) out.push({ no: body[i].no, text: kv.value, action: true });
    if (kv.key !== 'run') continue;
    if (kv.quoted || !/^[|>][+-]?\d*$/.test(kv.value)) {
      out.push({ no: body[i].no, text: kv.value });
      continue;
    }
    const keyIndent = body[i].indent + (dash ? dash[0].length : 0);
    let open = null; // a command whose line ended in a backslash
    for (let k = i + 1; k < body.length && body[k].indent > keyIndent; k += 1) {
      const line = open ? { no: open.no, text: `${open.text} ${body[k].text.trim()}` } : { no: body[k].no, text: body[k].text };
      if (/\\\s*$/.test(line.text)) {
        open = { no: line.no, text: line.text.replace(/\\\s*$/, '').trimEnd() };
      } else {
        open = null;
        out.push(line);
      }
    }
    if (open) out.push(open);
  }
  return out;
}

/** Does the job's `permissions:` grant id-token: write (a mapping, a flow mapping, or write-all)? */
function holdsIdToken(lines, perms) {
  const { value } = keyValue(perms.text);
  if (value === 'write-all') return true;
  if (value !== '') return /(^|[{,\s])["']?id-token["']?\s*:\s*["']?write\b/.test(value);
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
          const hit = c.action ? c.text : refusedCommand(c.text);
          if (!hit) continue;
          const what = c.action ? `uses ${hit}` : `runs \`${c.text.length > 90 ? `${c.text.slice(0, 90)}...` : c.text}\``;
          add(c.no, `job ${kv.key} holds id-token: write and ${what}; code that is built or fetched in the job that can ask for an OIDC token can ask for one too. Build in a job without the token and hand the files on (W6)`);
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
