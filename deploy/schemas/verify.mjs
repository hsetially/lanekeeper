// The committed schemas equal their lock (09/T3; S19). No network, no dependencies; CI runs it on every pull request.
//
// fetch.py (a developer machine) writes the schemas and schemas.lock.json from the commits pinned in sources.json. This check
// is the other half: it fails when a schema file was edited by hand, added without the lock, or left out, or when the lock names
// a source that sources.json does not pin. A change to a schema therefore always shows up as a change to the lock, which a
// person reads.
//
//   node deploy/schemas/verify.mjs [directory]
import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const HEX64 = /^[0-9a-f]{64}$/;
const HEX40 = /^[0-9a-f]{40}$/;

function* walk(dir) {
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* walk(path);
    else yield path;
  }
}

/** Every reason the directory does not equal its lock. Empty when it does. */
export function verify(dir) {
  const problems = [];
  const lock = JSON.parse(readFileSync(join(dir, 'schemas.lock.json'), 'utf8'));
  const sources = JSON.parse(readFileSync(join(dir, 'sources.json'), 'utf8'));

  // The pinned commits: repo -> every commit that sources.json names for it.
  const pinned = new Map();
  const pin = (repo, commit, where) => {
    if (!HEX40.test(commit ?? '')) problems.push(`sources.json: ${where} is pinned to '${commit}', which is not a 40-character commit`);
    pinned.set(repo, new Set([...(pinned.get(repo) ?? []), commit]));
  };
  pin(sources.kubernetes.repo, sources.kubernetes.commit, `kubernetes (${sources.kubernetes.repo})`);
  for (const group of [...sources.crds, ...sources.ready]) pin(group.repo, group.commit, group.repo);

  if (lock.kubernetes !== sources.kubernetes.version) {
    problems.push(`the lock is for Kubernetes ${lock.kubernetes}, sources.json says ${sources.kubernetes.version}`);
  }

  for (const [path, entry] of Object.entries(lock.files)) {
    const file = join(dir, path);
    if (!existsSync(file)) {
      problems.push(`${path}: missing (the lock lists it)`);
      continue;
    }
    const bytes = readFileSync(file);
    if (createHash('sha256').update(bytes).digest('hex') !== entry.sha256) problems.push(`${path}: sha256 differs from the lock`);
    if (bytes.length !== entry.bytes) problems.push(`${path}: ${bytes.length} bytes, the lock says ${entry.bytes}`);
    try {
      JSON.parse(bytes.toString('utf8'));
    } catch {
      problems.push(`${path}: not valid JSON`);
    }
    if (!HEX64.test(entry.sourceSha256 ?? '') || !HEX64.test(entry.sha256 ?? '')) problems.push(`${path}: a sha256 in the lock is not 64 hex digits`);
    const match = /^https:\/\/raw\.githubusercontent\.com\/([^/]+\/[^/]+)\/([0-9a-f]{40})\//.exec(entry.source ?? '');
    if (!match) problems.push(`${path}: its source is not a raw.githubusercontent.com URL at a 40-character commit`);
    else if (!pinned.get(match[1])?.has(match[2])) {
      problems.push(`${path}: the lock's source commit ${match[2]} of ${match[1]} is not a commit that sources.json pins`);
    }
  }

  for (const top of ['k8s', 'crds']) {
    if (!existsSync(join(dir, top))) continue;
    for (const file of walk(join(dir, top))) {
      const path = relative(dir, file).split('\\').join('/');
      if (!(path in lock.files)) problems.push(`${path}: not in the lock (run fetch.py, or delete the file)`);
    }
  }
  return problems;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dir = process.argv[2] ?? dirname(fileURLToPath(import.meta.url));
  const problems = verify(dir);
  for (const problem of problems) console.error(`schemas: ${problem}`);
  if (problems.length > 0) process.exit(1);
  const count = Object.keys(JSON.parse(readFileSync(join(dir, 'schemas.lock.json'), 'utf8')).files).length;
  console.log(`schemas: ${count} files equal the lock`);
}
