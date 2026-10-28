// The install guide (deploy/INSTALL-GKE.md) must say what the repository says (09/fix; AGENTS.md rule 12; S19).
//
// A person who does not know this codebase installs the whole application by hand from that guide, so a path that was renamed, a `just`
// recipe that was removed, a setting that the hub no longer reads, a secret that the chart no longer makes or a chart value that moved
// would send them to a step that cannot work. This test fails the build when that happens. What it reads is the repository itself (the
// Justfile, the three contract documents, the charts' values and the workflows), so there is no second list to keep in step.
//
// WHAT IT CHECKS (each rule has a name; a failure says which):
//   paths         every repository path in backticks, or in a command, that starts with deploy/, crates/, .github/, plans/, docs/ or
//                 scripts/ exists (a <PLACEHOLDER> in a path stands for one path segment)
//   recipes       every `just <recipe>` is a recipe of the Justfile
//   lk            every LK_* name is a setting that crates/hub/HARDENING.md, crates/hub/README.md or crates/agent/HARDENING.md defines, a
//                 repository variable that a workflow reads as vars.LK_*, or a shell variable that deploy/tests/kind/pins.env sets
//   secrets       the secret table of step 8 names only secrets that deploy/charts/{hub,gcp} define, and names every one of them
//   values        every chart value path in backticks exists in that chart's values.yaml: `config.`, `seed.` and `gateway.` paths are the
//                 hub chart's; any other chart is written `hub:`, `gcp:`, `db:` or `agent:` and the path
//   steps         the numbered steps are 0, 1, 2 ... without a gap, and the last is the list of known gaps
//   labels        steps 1 to 17 each say why, what to do, what to see and what to do if it did not work
//   notverified   a block with a gcloud command carries the notice "Not verified" or a "Source:" line in the paragraph right before it
//   placeholders  every <UPPER_CASE> placeholder is defined in the table of step 0, and every one defined there is used
//   identifiers   nothing looks like a real secret, key, token, project id, host name, address or tenant id
//   gaps          the list of known gaps names what the inventory and the blockers say is open
//   ca-binding    a block that grants a role on a Cloud KMS key names at most one of the two CA keys, so that no identity can sign with both
//                 CAs at once (S5): the roles go on one key at a time and come off before the next
//   secret-print  a command that reads a secret value (gcloud secrets versions access, kubectl get secret -o ...) does not print it: it
//                 redirects it to a file or pipes it to something that is not cat, tee or a pager
//   secret-file   the file such a command writes is in a directory made by mktemp -d under umask 077, not in the current directory
//                 (which may be a checkout of this repository), and the block removes the directory
//   fqdn          `network.fqdnPolicy: false` is offered nowhere but as "for kind only"; it is never in a command or a values block
//   links         deploy/argocd/README.md and deploy/ADDING-A-SWIMLANE.md point to the guide
//   triggers      deploy.yml and ci.yml run when the guide changes, and deploy-check runs this test
//
// The checker is shown to catch each of these by changing a copy of the real guide in memory (the mutants at the end). To try a copy by
// hand: LK_INSTALL_GUIDE=/path/to/copy.md node --test .github/scripts/install-guide.test.mjs
//
// What it cannot see: a claim that is wrong but well-formed, a command that is mistyped but built from real words, and a repository file
// that is deleted by a pull request that does not touch the files that start this test (ci.yml runs on every push to main, which is
// where such a change is caught).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const GUIDE_PATH = process.env.LK_INSTALL_GUIDE ? resolve(process.env.LK_INSTALL_GUIDE) : join(root, 'deploy', 'INSTALL-GKE.md');
const read = (path) => readFileSync(join(root, path), 'utf8');

// ---- the repository ------------------------------------------------------------------------------------------------------------

/** Every file the repository has (tracked, or new and not ignored), and every directory above one. */
function listRepository() {
  const git = spawnSync('git', ['ls-files', '--cached', '--others', '--exclude-standard'], { cwd: root, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });
  let files;
  if (git.status === 0) {
    files = git.stdout.split('\n').filter(Boolean);
  } else {
    files = [];
    const walk = (dir) => {
      for (const name of readdirSync(join(root, dir))) {
        if (name === 'node_modules' || name === 'target' || name === '.git') continue;
        const path = dir ? `${dir}/${name}` : name;
        if (statSync(join(root, path)).isDirectory()) walk(path);
        else files.push(path);
      }
    };
    walk('');
  }
  const dirs = new Set();
  for (const f of files) {
    const parts = f.split('/');
    for (let i = 1; i < parts.length; i += 1) dirs.add(parts.slice(0, i).join('/'));
  }
  return { files: new Set(files), dirs };
}

/**
 * The key paths of a block-style YAML file (a chart's values.yaml) and the scalar at each. No dependency: the files are plain
 * (no anchors, no flow mappings with content, no multi-document), and the guide only needs to know that a key exists.
 */
function yamlPaths(text) {
  const keys = new Set();
  const scalars = new Map();
  const stack = [];
  let skipAbove = -1;
  for (const line of text.split('\n')) {
    if (/^\s*(#|$)/.test(line)) continue;
    const indent = /^ */.exec(line)[0].length;
    if (skipAbove >= 0) {
      if (indent > skipAbove) continue;
      skipAbove = -1;
    }
    const m = /^( *)([A-Za-z0-9_-]+):(?:\s+(.*))?$/.exec(line);
    if (!m) continue;
    while (stack.length > 0 && stack[stack.length - 1].indent >= indent) stack.pop();
    stack.push({ indent, key: m[2] });
    const path = stack.map((s) => s.key).join('.');
    keys.add(path);
    let value = (m[3] ?? '').replace(/\s+#.*$/, '').trim();
    if (/^[|>][+-]?$/.test(value)) {
      skipAbove = indent;
      value = '';
    }
    if (value !== '') scalars.set(path, value.replace(/^["']|["']$/g, ''));
  }
  return { keys, scalars };
}

/** Whether a JSON schema describes the property at `path` (a list of keys). */
function schemaHas(schema, path) {
  let node = schema;
  for (const key of path) {
    node = node?.properties?.[key];
    if (!node) return false;
  }
  return true;
}

function loadRepository() {
  const { files, dirs } = listRepository();
  const justfile = read('Justfile');
  const recipes = new Set([...justfile.matchAll(/^([a-z][a-z0-9_-]*)(?:[ \t][^:\n=]*)?:(?!=)/gm)].map((m) => m[1]));
  const lkDefined = new Set();
  for (const doc of ['crates/hub/HARDENING.md', 'crates/hub/README.md', 'crates/agent/HARDENING.md']) {
    for (const m of read(doc).matchAll(/\bLK_[A-Z0-9]+(?:_[A-Z0-9]+)*/g)) lkDefined.add(m[0]);
  }
  const lkVariables = new Set();
  for (const m of read('deploy/tests/kind/pins.env').matchAll(/^(LK_[A-Z0-9]+(?:_[A-Z0-9]+)*)=/gm)) lkVariables.add(m[1]);
  for (const name of readdirSync(join(root, '.github', 'workflows'))) {
    if (!/\.ya?ml$/.test(name)) continue;
    for (const m of read(`.github/workflows/${name}`).matchAll(/\bvars\.(LK_[A-Z0-9]+(?:_[A-Z0-9]+)*)/g)) lkVariables.add(m[1]);
  }
  const charts = {};
  for (const chart of ['hub', 'gcp', 'db', 'agent']) {
    const values = yamlPaths(read(`deploy/charts/${chart}/values.yaml`));
    charts[chart] = { ...values, schema: JSON.parse(read(`deploy/charts/${chart}/values.schema.json`)) };
  }
  // The secret names the charts define: the hub chart's six (config.secretManager.*), and the four constants of the hub's code that
  // the gcp chart's helper lists.
  const hubSecrets = [...charts.hub.scalars].filter(([path]) => path.startsWith('config.secretManager.')).map(([, v]) => v);
  const gcpHelpers = read('deploy/charts/gcp/templates/_helpers.tpl');
  const constants = ['write-preview-key', 'github-read-token', 'github-webhook-secret', 'mcp-token-hmac-key'];
  for (const c of constants) assert.ok(gcpHelpers.includes(`"${c}"`), `the gcp chart's helper no longer lists the constant secret ${c}: update this test and the guide`);
  const secretNames = new Set([...hubSecrets, ...constants]);
  assert.equal(hubSecrets.length, 6, 'deploy/charts/hub/values.yaml should name six secrets under config.secretManager');
  return { files, dirs, recipes, lkDefined, lkVariables, charts, secretNames };
}

// ---- the guide -----------------------------------------------------------------------------------------------------------------

/** The fenced blocks (with the line each starts at, 0-based, counting the fence) and the lines that are outside them. */
function splitMarkdown(text) {
  const lines = text.split('\n');
  const blocks = [];
  const prose = [];
  let open = null;
  lines.forEach((line, i) => {
    if (/^\s*```/.test(line)) {
      if (open) {
        blocks.push({ start: open.start, end: i, lines: open.lines });
        open = null;
      } else {
        open = { start: i, lines: [] };
      }
      return;
    }
    if (open) open.lines.push(line);
    else prose.push({ line: i, text: line });
  });
  if (open) blocks.push({ start: open.start, end: lines.length, lines: open.lines, unclosed: true });
  return { lines, blocks, prose };
}

/** The paragraph right before line `start` (0-based): the lines back to the previous blank line, skipping blank lines directly above `start`. */
function paragraphBefore(lines, start) {
  let end = start - 1;
  while (end >= 0 && lines[end].trim() === '') end -= 1;
  let begin = end;
  while (begin > 0 && lines[begin - 1].trim() !== '') begin -= 1;
  return end < 0 ? { begin: 0, end: -1, text: '' } : { begin, end, text: lines.slice(begin, end + 1).join('\n') };
}

/** Inline code spans of the prose lines, as { line, text }. */
function inlineSpans(prose) {
  const spans = [];
  for (const { line, text } of prose) for (const m of text.matchAll(/`([^`\n]+)`/g)) spans.push({ line, text: m[1] });
  return spans;
}

/** Sections by their "## N. title" heading: { number, title, line, body }. */
function sections(lines) {
  const found = [];
  lines.forEach((text, line) => {
    const m = /^## (\d+)\. (.+)$/.exec(text);
    if (m) found.push({ number: Number(m[1]), title: m[2], line, body: '' });
  });
  found.forEach((s, i) => {
    const end = i + 1 < found.length ? found[i + 1].line : lines.length;
    s.body = lines.slice(s.line + 1, end).join('\n');
  });
  return found;
}

const PATH_START = /^(deploy|crates|\.github|plans|docs|scripts)\//;
const PLACEHOLDER = /<[A-Z][A-Z0-9_]*>/g;

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function pathExists(repo, token) {
  const bare = token.replace(/\/+$/, '');
  if (!/[<*]/.test(bare)) return repo.files.has(bare) || repo.dirs.has(bare);
  const pattern = bare
    .split(/(<[A-Z][A-Z0-9_]*>|\*)/)
    .map((part) => (/^(<[A-Z][A-Z0-9_]*>|\*)$/.test(part) ? '[^/]+' : escapeRegExp(part)))
    .join('');
  const re = new RegExp(`^${pattern}$`);
  return [...repo.files].some((f) => re.test(f)) || [...repo.dirs].some((d) => re.test(d));
}

/** All the code in the guide, as { line, text }: inline spans, and the lines of the fenced blocks. */
function codeLines(md) {
  const out = inlineSpans(md.prose);
  md.blocks.forEach((b) => b.lines.forEach((text, i) => out.push({ line: b.start + 1 + i, text })));
  return out;
}

/** The commands of a fenced block: lines joined at a trailing backslash, comments and blank lines left out. */
function commandsOf(lines) {
  const out = [];
  let current = '';
  for (const raw of lines) {
    if (current === '' && /^\s*(#|$)/.test(raw)) continue;
    const line = raw.replace(/\s+$/, '');
    if (line.endsWith('\\')) {
      current += `${line.slice(0, -1)} `;
    } else {
      out.push(`${current}${line}`);
      current = '';
    }
  }
  if (current !== '') out.push(current);
  return out;
}

/** The paragraph (the lines up to the blank ones around it) that holds line `index`. */
function paragraphAround(lines, index) {
  let begin = index;
  while (begin > 0 && lines[begin - 1].trim() !== '') begin -= 1;
  let end = index;
  while (end + 1 < lines.length && lines[end + 1].trim() !== '') end += 1;
  return lines.slice(begin, end + 1).join('\n');
}

// ---- the checks ----------------------------------------------------------------------------------------------------------------

const GAPS_MUST_NAME = [
  ['D137', 'the 12 MiB edit bodies through the load balancer'],
  ['Mcp-Session-Id', 'MCP session affinity'],
  ['publishNotReadyAddresses', 'the headless Service'],
  ['LK_GITHUB_API_BASE', 'a GitHub Enterprise Server'],
  ['LK_ENTRA_AUTHORITY_HOST', 'a sovereign Entra cloud'],
  ['LK_MCP_NOTIFY_SERVICES', 'the optional settings the chart does not expose'],
  ['LK_CONFIG_SERVER_DEPLOYMENT', 'the config-server start time (C11)'],
  ['P11', 'readiness on three replicas never measured in CI'],
  ['kind', 'why the production image cannot be Ready in kind'],
  ['B6', 'Google-specific shapes that were not verified'],
  ['Q24', 'Binary Authorization in dry run'],
  ['drain', 'the shutdown against the health check'],
  ['branch protection', 'the required-check names'],
  ['ca-init', 'the identity that runs hub ca-init'],
];

const SECRET_PATTERNS = [
  [/-----BEGIN [A-Z ]*PRIVATE KEY-----/, 'a private key'],
  [/\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}/, 'a GitHub token'],
  [/\bgithub_pat_[A-Za-z0-9_]{20,}/, 'a GitHub token'],
  [/\bAIza[0-9A-Za-z_-]{35}/, 'a Google API key'],
  [/\bAKIA[0-9A-Z]{16}\b/, 'an AWS access key'],
  [/\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]+/, 'a signed token (JWT)'],
  [/\bxox[abprs]-[A-Za-z0-9-]{10,}/, 'a Slack token'],
  [/\bGOCSPX-[A-Za-z0-9_-]{20,}/, 'a Google OAuth client secret'],
  [/"private_key"\s*:/, 'a service-account key file'],
];

// Hosts the guide may name. Everything else with a scheme must be a <PLACEHOLDER> or an example.com name.
const ALLOWED_HOSTS = new Set([
  'github.com', 'api.github.com', 'raw.githubusercontent.com', 'ghcr.io', 'login.microsoftonline.com', 'localhost', '127.0.0.1',
  'metadata.google.internal', 'kubernetes.default.svc', 'token.actions.githubusercontent.com', 'registry.k8s.io', 'console.cloud.google.com',
  'entra.microsoft.com', 'portal.azure.com', 'docs.cloud.google.com', 'argo-cd.readthedocs.io', 'cloud.google.com',
]);
const ALLOWED_IPS = /^(192\.0\.2\.|198\.51\.100\.|203\.0\.113\.|127\.0\.0\.1$|0\.0\.0\.0$|169\.254\.169\.254$|35\.191\.0\.0$|130\.211\.0\.0$)/;

function check(text, repo) {
  const problems = [];
  const fail = (rule, message) => problems.push({ rule, message });
  const md = splitMarkdown(text);
  const code = codeLines(md);
  const secs = sections(md.lines);

  // paths
  for (const { line, text: span } of code) {
    for (const raw of span.split(/[\s'"=,;()|]+/)) {
      const token = raw.replace(/[.,:;]+$/, '');
      if (!PATH_START.test(token)) continue;
      if (!pathExists(repo, token)) fail('paths', `line ${line + 1}: ${token} is not a path of the repository`);
    }
  }

  // recipes
  for (const { line, text: span } of code) {
    for (const m of span.matchAll(/(?:^|[\s;&|(`$])just\s+([a-z][a-z0-9_-]*)/g)) {
      if (!repo.recipes.has(m[1])) fail('recipes', `line ${line + 1}: \`just ${m[1]}\` is not a recipe of the Justfile`);
    }
  }

  // lk
  for (const m of text.matchAll(/\bLK_[A-Z0-9]+(?:_[A-Z0-9]+)*/g)) {
    if (!repo.lkDefined.has(m[0]) && !repo.lkVariables.has(m[0])) {
      fail('lk', `${m[0]} is neither a setting of crates/hub/HARDENING.md, crates/hub/README.md or crates/agent/HARDENING.md, nor a repository variable of a workflow, nor a variable of deploy/tests/kind/pins.env`);
    }
  }

  // secrets: the table of step 8
  const eight = secs.find((s) => s.number === 8);
  if (!eight) {
    fail('secrets', 'there is no step 8, which holds the table of secrets');
  } else {
    const rows = eight.body.split('\n').filter((l) => /^\|/.test(l));
    const header = rows.findIndex((l) => /^\|\s*Secret\s*\|/.test(l));
    if (header < 0) {
      fail('secrets', 'step 8 has no table whose first column is "Secret"');
    } else {
      const named = new Set();
      for (const row of rows.slice(header + 2)) {
        const first = row.split('|')[1] ?? '';
        for (const m of first.matchAll(/`([^`]+)`/g)) {
          named.add(m[1]);
          if (!repo.secretNames.has(m[1]) && !/<[A-Z][A-Z0-9_]*>/.test(m[1])) fail('secrets', `${m[1]} is in the table of step 8 and is no secret of deploy/charts/hub or deploy/charts/gcp`);
        }
      }
      for (const name of repo.secretNames) if (!named.has(name)) fail('secrets', `the table of step 8 does not name ${name}, which the charts make`);
    }
  }

  // values
  for (const { line, text: span } of inlineSpans(md.prose)) {
    const tail = '(?:\\s*[:=].*)?$';
    const prefixed = new RegExp(`^(hub|gcp|db|agent):([A-Za-z][A-Za-z0-9]*(?:\\.[A-Za-z][A-Za-z0-9_]*)*)${tail}`).exec(span);
    const bare = new RegExp(`^((?:config|seed|gateway)(?:\\.[A-Za-z][A-Za-z0-9_]*)+)${tail}`).exec(span);
    const chart = prefixed ? prefixed[1] : bare ? 'hub' : null;
    const path = prefixed ? prefixed[2] : bare ? bare[1] : null;
    if (!chart) continue;
    if (/\.(ya?ml|json|md|mjs|sh|toml|pem|env|rego|tpl|txt|lock)$/.test(path)) continue; // a file name such as seed.yaml, not a value
    const info = repo.charts[chart];
    const known = info.keys.has(path) || (() => {
      const parts = path.split('.');
      const parent = parts.slice(0, -1).join('.');
      return info.scalars.get(parent) === '{}' && schemaHas(info.schema, parts);
    })();
    if (!known) fail('values', `line ${line + 1}: ${path} is not a value of deploy/charts/${chart}/values.yaml`);
  }

  // steps
  const numbers = secs.map((s) => s.number);
  numbers.forEach((n, i) => {
    if (n !== i) fail('steps', `the step headings are ${numbers.join(', ')}: step ${i} is missing or misplaced`);
  });
  if (secs.length < 19) fail('steps', `only ${secs.length} numbered steps were found; the guide has steps 0 to 18`);
  const lastStep = secs[secs.length - 1];
  if (!lastStep || !/known gaps/i.test(lastStep.title)) fail('steps', 'the last step is not the list of known gaps');
  if (!md.lines.some((l) => /^## \d+\. .*known gaps/i.test(l))) fail('steps', 'there is no "Known gaps" section');

  // labels
  for (const s of secs) {
    if (s.number < 1 || s.number > 17) continue;
    for (const label of ['**Why.**', '**Do.**', '**You should see.**', '**If not.**']) {
      if (!s.body.includes(label)) fail('labels', `step ${s.number} (${s.title}) has no ${label}`);
    }
  }

  // notverified
  for (const b of md.blocks) {
    if (!b.lines.some((l) => /(^|[\s|$(])(gcloud|gsutil|bq)\s/.test(l))) continue;
    if (!/Not verified|Source:/.test(paragraphBefore(md.lines, b.start).text)) {
      fail('notverified', `line ${b.start + 1}: a block with a gcloud command has no "Not verified" notice or "Source:" line in the paragraph right before it`);
    }
  }

  // placeholders
  const zero = secs.find((s) => s.number === 0);
  const defined = new Set();
  if (!zero) {
    fail('placeholders', 'there is no step 0, which defines the placeholders');
  } else {
    for (const row of zero.body.split('\n').filter((l) => /^\|/.test(l))) {
      const first = row.split('|')[1] ?? '';
      for (const m of first.matchAll(PLACEHOLDER)) defined.add(m[0]);
    }
  }
  const used = new Set(text.match(PLACEHOLDER) ?? []);
  for (const p of used) if (!defined.has(p)) fail('placeholders', `${p} is used and is not defined in the table of step 0`);
  for (const p of defined) {
    const inRows = (zero?.body.match(new RegExp(escapeRegExp(p), 'g')) ?? []).length;
    const total = (text.match(new RegExp(escapeRegExp(p), 'g')) ?? []).length;
    if (total === inRows) fail('placeholders', `${p} is defined in step 0 and used nowhere else`);
  }

  // identifiers
  const lines = md.lines;
  lines.forEach((l, i) => {
    const at = `line ${i + 1}`;
    for (const [re, what] of SECRET_PATTERNS) if (re.test(l)) fail('identifiers', `${at}: looks like ${what}`);
    for (const m of l.matchAll(/--project(?:=|\s+)(\S+)/g)) {
      if (!/^["']?[<$]/.test(m[1])) fail('identifiers', `${at}: --project ${m[1]} is not a placeholder`);
    }
    for (const m of l.matchAll(/\bprojects\/([^/\s`"')]+)/g)) {
      if (!/^([<$]|\.\.\.$)/.test(m[1])) fail('identifiers', `${at}: projects/${m[1]} is not a placeholder`);
    }
    for (const m of l.matchAll(/@([A-Za-z0-9<>_$.-]+)\.iam\.gserviceaccount\.com/g)) {
      if (!/[<$]/.test(m[1])) fail('identifiers', `${at}: a service account of the project ${m[1]} is not a placeholder`);
    }
    for (const m of l.matchAll(/\bgs:\/\/([^/\s`"')]+)/g)) {
      if (!/^[<$]/.test(m[1])) fail('identifiers', `${at}: gs://${m[1]} is not a placeholder`);
    }
    for (const m of l.replace(/\\\./g, '.').matchAll(/\bhttps?:\/\/([A-Za-z0-9._<>$-]+)/g)) {
      const host = m[1];
      const ok = /[<$]/.test(host) || ALLOWED_HOSTS.has(host) || /(^|\.)(example\.(com|org|net)|googleapis\.com)$/.test(host);
      if (!ok) fail('identifiers', `${at}: the host ${host} is neither a placeholder nor an example or well-known name`);
    }
    for (const m of l.matchAll(/\b(\d{1,3}(?:\.\d{1,3}){3})(?:\/\d{1,2})?\b/g)) {
      if (!ALLOWED_IPS.test(m[1])) fail('identifiers', `${at}: the address ${m[1]} is not a documentation address`);
    }
    for (const m of l.matchAll(/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi)) {
      fail('identifiers', `${at}: ${m[0]} looks like a tenant, client or object id`);
    }
    for (const m of l.matchAll(/\b[0-9a-f]{40}\b|\b[0-9a-f]{64}\b/gi)) {
      if (!/^0+$/.test(m[0])) fail('identifiers', `${at}: ${m[0].slice(0, 12)}... is a commit or a digest, which belongs to a placeholder`);
    }
    for (const m of l.matchAll(/\b[0-9]{12}\b/g)) {
      fail('identifiers', `${at}: ${m[0]} looks like a project number`);
    }
  });

  // ca-binding: the two CA keys are never bound at once
  for (const b of md.blocks) {
    const block = b.lines.join('\n');
    if (!/kms\s+keys\s+add-iam-policy-binding/.test(block)) continue;
    const named = ['<AGENT_CA_KEY_NAME>', '<REPLICA_CA_KEY_NAME>'].filter((k) => block.includes(k));
    if (named.length > 1) {
      fail('ca-binding', `line ${b.start + 1}: a block that grants a role on a Cloud KMS key names both CA keys (${named.join(', ')}); the roles go on one key at a time and come off before the next, so that no identity can sign with both CAs at once (S5)`);
    }
  }

  // secret-print and secret-file
  const readsSecret = /gcloud\s+secrets\s+versions\s+access\b|\bkubectl\b.*\bget\s+secrets?\b.*(?:-o|--output)[ =]?(?:yaml|json|jsonpath|go-template)/;
  for (const b of md.blocks) {
    const block = b.lines.join('\n');
    for (const raw of commandsOf(b.lines)) {
      // <PLACEHOLDER> has angle brackets, which are not a redirect.
      const command = raw.replace(/<[A-Z][A-Z0-9_]*>/g, 'PLACEHOLDER');
      if (!readsSecret.test(command)) continue;
      const redirect = /(?:^|[^>0-9&])>\s*("[^"]*"|'[^']*'|[^\s;|)&]+)/.exec(command);
      const piped = /\|\s*(?!cat\b|tee\b|less\b|more\b)\S/.test(command);
      if (!redirect && !piped) {
        fail('secret-print', `line ${b.start + 1}: a command prints a secret value to the terminal, where the scrollback and the shell's history keep it: ${command.slice(0, 90)}`);
      } else if (redirect) {
        if (!/^["']?(\$|\/|~)/.test(redirect[1])) {
          fail('secret-file', `line ${b.start + 1}: a secret value is written into ${redirect[1]}, which is in the current directory (it may be a checkout of this repository); write it into a directory made by mktemp -d`);
        }
        for (const [re, what] of [[/\bmktemp\s+-d\b/, 'mktemp -d'], [/\bumask\s+077\b/, 'umask 077'], [/\brm\s+-r\w*\s/, 'rm -r of the directory']]) {
          if (!re.test(block)) fail('secret-file', `line ${b.start + 1}: the block that writes a secret value to a file has no ${what}`);
        }
      }
    }
  }

  // fqdn: fqdnPolicy false is for kind only
  md.lines.forEach((l, i) => {
    if (!/fqdnPolicy/.test(l)) return;
    const around = paragraphAround(md.lines, i);
    if (!/fqdnPolicy[^\n]{0,24}false|false[^\n]{0,24}fqdnPolicy/.test(around)) return;
    if (!/kind only/.test(around)) fail('fqdn', `line ${i + 1}: fqdnPolicy false is mentioned and the paragraph does not say that it is for kind only`);
  });
  for (const b of md.blocks) {
    const block = b.lines.join('\n');
    if (/fqdnPolicy\s*[:=]\s*false/.test(block)) fail('fqdn', `line ${b.start + 1}: a command or values block sets fqdnPolicy to false, which the policies refuse for staging and prod`);
  }

  // gaps
  const gaps = secs[secs.length - 1];
  if (gaps) {
    for (const [word, about] of GAPS_MUST_NAME) {
      if (!gaps.body.toLowerCase().includes(word.toLowerCase())) fail('gaps', `the known gaps do not mention ${word} (${about})`);
    }
  }

  return problems;
}

// ---- the real guide ------------------------------------------------------------------------------------------------------------

const repo = loadRepository();
const guide = readFileSync(GUIDE_PATH, 'utf8');
const rulesOf = (problems, rule) => problems.filter((p) => p.rule === rule);

test('the guide agrees with the repository: paths, recipes, settings, secrets, values, steps, notices, placeholders, identifiers, gaps', () => {
  const problems = check(guide, repo);
  assert.deepEqual(problems.map((p) => `${p.rule}: ${p.message}`), []);
});

test('the guide is big enough to be the guide (the checker is reading it)', () => {
  assert.ok(guide.length > 20000, `only ${guide.length} characters were read from ${GUIDE_PATH}`);
  assert.ok(codeLines(splitMarkdown(guide)).length > 100);
});

test('deploy/argocd/README.md and deploy/ADDING-A-SWIMLANE.md link to the guide in their first lines', () => {
  for (const file of ['deploy/argocd/README.md', 'deploy/ADDING-A-SWIMLANE.md']) {
    const head = read(file).split('\n').slice(0, 6).join('\n');
    assert.match(head, /INSTALL-GKE\.md/, `${file} does not point to deploy/INSTALL-GKE.md at its top`);
  }
});

test('deploy.yml and ci.yml run when the guide or the files this test reads change, and deploy-check runs this test', () => {
  const pathsOf = (file) => {
    const m = /\n  pull_request:\s*\n    paths:\s*\n((?:      (?:- .+|#.*)\n)+)/.exec(read(file));
    assert.ok(m, `${file} has no pull_request paths filter`);
    return m[1].split('\n').filter((l) => /^\s*- /.test(l)).map((l) => l.replace(/^\s*- /, '').replace(/^"|"$/g, ''));
  };
  for (const file of ['.github/workflows/deploy.yml', '.github/workflows/ci.yml']) {
    const paths = pathsOf(file);
    for (const needed of ['deploy/INSTALL-GKE.md', 'crates/hub/HARDENING.md', 'crates/hub/README.md', 'crates/agent/HARDENING.md', 'deploy/charts/hub/values.yaml', 'deploy/charts/gcp/values.yaml']) {
      const covered = paths.some((p) => p === needed || (p.endsWith('/**') && needed.startsWith(p.slice(0, -2))));
      assert.ok(covered, `${file} does not run when ${needed} changes`);
    }
  }
  const justfile = read('Justfile');
  assert.match(justfile, /^install-guide-test:\s*\n\s+node --test \.github\/scripts\/install-guide\.test\.mjs$/m, 'the Justfile has no recipe install-guide-test that runs this file');
  assert.match(justfile, /^deploy-check:[^\n]*\binstall-guide-test\b/m, 'deploy-check does not run install-guide-test');
});

// ---- the checker catches each kind of drift --------------------------------------------------------------------------------------
//
// Each mutant changes a copy of the real guide in memory the way a repository change would have, and the checker must report the rule
// that guards it. A mutant that changes nothing is itself an error (the anchor is gone: update the mutant).

/** Replace the first match of `re` with `to(match)`; fail if there is none. */
function replaceFirst(text, re, to) {
  const m = re.exec(text);
  assert.ok(m, `the guide has nothing that matches ${re}: the mutant has no anchor`);
  return text.slice(0, m.index) + to(m) + text.slice(m.index + m[0].length);
}

/** Replace the paragraph right before the first fenced block that holds a gcloud command by a line that says nothing. */
function withoutNoticeBeforeFirstGcloud(text) {
  const md = splitMarkdown(text);
  const block = md.blocks.find((b) => b.lines.some((l) => /(^|[\s|$(])(gcloud|gsutil|bq)\s/.test(l)));
  assert.ok(block, 'the guide has no block with a gcloud command: the mutant has no anchor');
  const { begin, end } = paragraphBefore(md.lines, block.start);
  md.lines.splice(begin, end - begin + 1, 'Run this.');
  return md.lines.join('\n');
}

const MUTANTS = [
  ['paths', 'a renamed file', (t) => replaceFirst(t, /deploy\/charts\/hub\/values\.yaml/, () => 'deploy/charts/hub/valuez.yaml')],
  ['paths', 'a path with a placeholder in a directory that is gone', (t) => replaceFirst(t, /deploy\/values\/hub\/<ENV>\.yaml/, () => 'deploy/valuez/hub/<ENV>.yaml')],
  ['paths', 'a renamed script in a command', (t) => replaceFirst(t, /deploy\/tools\/github-hook-ranges\.mjs/, () => 'deploy/tools/github-hook-range.mjs')],
  ['recipes', 'a removed recipe', (t) => replaceFirst(t, /just (deploy-check|ops-tools-install|images-repro)/, (m) => `just ${m[1]}-gone`)],
  ['lk', 'a renamed setting', (t) => replaceFirst(t, /LK_PUBLIC_ORIGIN/, () => 'LK_PUBLIC_ORIGINS')],
  ['lk', 'a setting nobody defines', (t) => `${t}\nSet \`LK_NOT_A_SETTING\` to 1.\n`],
  ['secrets', 'a renamed secret', (t) => replaceFirst(t, /^\| `lanekeeper-web-cert`/m, () => '| `lanekeeper-web-certs`')],
  ['secrets', 'a secret that the table forgets', (t) => replaceFirst(t, /^\| `github-read-token`.*\n/m, () => '')],
  ['values', 'a renamed hub chart value', (t) => replaceFirst(t, /`config\.git\.baseUrl`/, () => '`config.git.baseUrI`')],
  ['values', 'a renamed value of another chart', (t) => replaceFirst(t, /`gcp:kms\.keyRing`/, () => '`gcp:kms.keyRings`')],
  ['values', 'a value under tuning that the schema does not know', (t) => `${t}\nSet \`config.tuning.LK_NOT_A_TUNABLE\`.\n`],
  ['steps', 'a step number removed', (t) => replaceFirst(t, /^## 7\. /m, () => '## ')],
  ['steps', 'a step number repeated', (t) => replaceFirst(t, /^## 9\. /m, () => '## 8. ')],
  ['steps', 'the known gaps removed', (t) => replaceFirst(t, /^## 18\. .*$/m, () => '## 18. Notes')],
  ['labels', 'a step without its "what you should see"', (t) => replaceFirst(t, /\*\*You should see\.\*\*/, () => '**Result.**')],
  ['notverified', 'a gcloud block without its notice', withoutNoticeBeforeFirstGcloud],
  ['placeholders', 'a placeholder that step 0 does not define', (t) => `${t}\nUse <NOT_DEFINED_ANYWHERE> here.\n`],
  ['placeholders', 'a placeholder that nothing uses', (t) => replaceFirst(t, /^\| `<PROJECT_ID>`/m, () => '| `<PROJECT_ID>` `<NEVER_USED_PLACEHOLDER>`')],
  ['identifiers', 'a GitHub token', (t) => `${t}\nToken: ghp_${'a1B2c3D4e5'.repeat(4)}\n`],
  ['identifiers', 'a private key', (t) => `${t}\n-----BEGIN PRIVATE KEY-----\n`],
  ['identifiers', 'a real project id', (t) => `${t}\n\`\`\`\ngcloud services list --project acme-prod-4711\n\`\`\`\n`],
  ['identifiers', 'a real host', (t) => `${t}\nOpen https://hub.acme-corp.net/ now.\n`],
  ['identifiers', 'a tenant id', (t) => `${t}\nTenant 3f2a9c1e-7b44-4d0e-9a5c-0123456789ab.\n`],
  ['identifiers', 'a public address', (t) => `${t}\nAllow 8.8.4.4 in.\n`],
  ['identifiers', 'a service account of a real project', (t) => `${t}\nas hub@acme-prod-4711.iam.gserviceaccount.com\n`],
  ['gaps', 'a known gap that the list forgets', (t) => t.replaceAll('publishNotReadyAddresses', 'publishing')],
  // 09/fix, round 2 (NB-4): the rules that hold what round 1 fixed in the guide.
  ['ca-binding', 'a loop that binds both CA keys at once', (t) => replaceFirst(t, /for role in roles\/cloudkms\.publicKeyViewer roles\/cloudkms\.signerVerifier; do\n  gcloud kms keys add-iam-policy-binding <AGENT_CA_KEY_NAME>/, () => 'for key in <AGENT_CA_KEY_NAME> <REPLICA_CA_KEY_NAME>; do for role in roles/cloudkms.publicKeyViewer roles/cloudkms.signerVerifier; do\n  gcloud kms keys add-iam-policy-binding "$key"')],
  ['ca-binding', 'the replica key bound in the block of the agent key', (t) => replaceFirst(t, /(  gcloud kms keys add-iam-policy-binding <AGENT_CA_KEY_NAME>[^\n]*\n[^\n]*\n)done/, (m) => `${m[1]}done\ngcloud kms keys add-iam-policy-binding <REPLICA_CA_KEY_NAME> --keyring=<KEYRING> --location=<LOCATION> --member=serviceAccount:<CA_INIT_GSA>@<PROJECT_ID>.iam.gserviceaccount.com --role=roles/cloudkms.signerVerifier`)],
  ['secret-print', 'a secret read and printed', (t) => `${t}\n\`\`\`bash\ngcloud secrets versions access latest --secret=github-webhook-secret --project=<PROJECT_ID>\n\`\`\`\n`],
  ['secret-print', 'a secret read and piped to cat', (t) => `${t}\n\`\`\`bash\ngcloud secrets versions access latest --secret=github-webhook-secret --project=<PROJECT_ID> | cat\n\`\`\`\n`],
  ['secret-print', 'a Kubernetes Secret printed over a continued line', (t) => `${t}\n\`\`\`bash\nkubectl -n lanekeeper get secret lanekeeper-db-app \\\n  -o jsonpath='{.data.uri}'\n\`\`\`\n`],
  ['secret-file', 'the webhook secret written into the current directory', (t) => replaceFirst(t, /> "\$dir\/webhook-secret\.txt"\)/, () => '> webhook-secret.txt)')],
  ['secret-file', 'the private directory not made', (t) => replaceFirst(t, /dir="\$\(umask 077; mktemp -d\)"\n/, () => 'dir=/tmp/hook\n')],
  ['secret-file', 'the directory not removed', (t) => replaceFirst(t, /\nrm -r "\$dir"/, () => '')],
  ['fqdn', 'fqdnPolicy false set in a values block', (t) => `${t}\n\`\`\`yaml\nnetwork:\n  fqdnPolicy: false\n\`\`\`\n`],
  ['fqdn', 'fqdnPolicy false in a values block that says it is for kind only', (t) => `${t}\nA value for kind only:\n\`\`\`yaml\nnetwork:\n  fqdnPolicy: false\n\`\`\`\n`],
  ['fqdn', 'fqdnPolicy false offered for any cluster', (t) => replaceFirst(t, /is for\nkind only/, () => 'is for\nclusters without the feature')],
];

for (const [rule, what, mutate] of MUTANTS) {
  test(`the checker reports ${rule} for ${what}`, () => {
    const before = rulesOf(check(guide, repo), rule).length;
    const mutated = mutate(guide);
    assert.notEqual(mutated, guide, 'the mutant changed nothing');
    const after = rulesOf(check(mutated, repo), rule).length;
    assert.ok(after > before, `the checker did not notice ${what}: ${after} ${rule} problems against ${before} before`);
  });
}

test('a guide that is empty fails every structural rule', () => {
  const problems = check('# nothing\n', repo);
  for (const rule of ['steps', 'secrets', 'placeholders']) assert.ok(rulesOf(problems, rule).length > 0, `an empty guide passes ${rule}`);
});
