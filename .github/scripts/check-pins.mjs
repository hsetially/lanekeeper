#!/usr/bin/env node
// Every external reference in a workflow or action must name immutable content (09/T2; S19, S20).
//
//   node .github/scripts/check-pins.mjs [PATH...]
//
// A PATH is a YAML file or a directory (searched recursively for *.yml and *.yaml). With no PATH the checker reads
// .github/workflows and .github/actions of the checkout it lives in. It needs no dependencies, so CI can run it before
// anything is installed.
//
// What it requires:
//   uses: owner/repo[/path]@<40 lowercase hex>     an action or a reusable workflow: a full commit SHA, never a tag, a
//                                                  branch or a short SHA, because a tag can be moved to other code
//   uses: docker://image@sha256:<64 hex>           a container action: a digest
//   image: ref@sha256:<64 hex>                     the image of a service, of a container job, of a docker action
//   container: ref@sha256:<64 hex>                 the scalar form of a container job
// A local reference (uses: ./path) is part of this repository and is not checked further. In an action.yml, `image:` may name
// a Dockerfile of the action itself. An expression (${{ ... }}) cannot be verified by reading the file, so it is refused.
//
// Text inside a block scalar (`run: |`) is a script, not a reference, and is skipped, and so are the keys of a `matrix:` (a
// matrix key called `image` names a variant; an image pulled by one of them must be written as an expression, which is refused).
// A version comment after the SHA (`# v4.2.2`) is allowed and not required.
//
// Exit codes: 0 every reference is pinned; 1 at least one is not (each is printed as file:line: reason); 2 nothing to check
// or a path that cannot be read.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const SHA40 = /^[0-9a-f]{40}$/;
const DIGEST = /@sha256:[0-9a-f]{64}$/;

/** The value after `key:`, without a trailing comment or surrounding quotes. */
function scalar(raw) {
  let v = raw.trim();
  const q = v[0];
  if (q === '"' || q === "'") {
    const end = v.indexOf(q, 1);
    return end > 0 ? v.slice(1, end) : v.slice(1);
  }
  const hash = v.search(/\s#/);
  if (hash >= 0) v = v.slice(0, hash);
  return v.trim();
}

/** Why a `uses:` value is refused, or null when it is pinned. */
function checkUses(value) {
  if (value === '') return 'uses: is empty';
  if (value.includes('${{')) return `${value} is an expression; a reference must be written out so that it can be checked`;
  if (value.startsWith('./') || value.startsWith('../')) return null;
  if (value.startsWith('docker://')) {
    return DIGEST.test(value) ? null : `${value} has no image digest; write docker://image@sha256:<64 hex>`;
  }
  const at = value.lastIndexOf('@');
  if (at < 0) return `${value} has no @ref; pin it to a full commit SHA`;
  const ref = value.slice(at + 1);
  if (SHA40.test(ref)) return null;
  if (/^[0-9A-F]{40}$/.test(ref)) return `${value} is a SHA in capitals; write the 40 hex digits in lowercase`;
  if (/^[0-9a-f]{7,39}$/i.test(ref)) return `${value} is pinned to a short SHA; use the full 40-character commit SHA`;
  return `${value} is pinned to a tag or branch; use the full 40-character commit SHA (a tag can be moved)`;
}

/** Why an `image:` or `container:` value is refused, or null when it carries a digest. */
function checkImage(value, isActionFile) {
  if (value === '') return null; // `container:` or `image:` that opens a mapping
  if (value.includes('${{')) return `${value} is an expression; a reference must be written out so that it can be checked`;
  const ref = value.startsWith('docker://') ? value.slice('docker://'.length) : value;
  if (isActionFile && !value.startsWith('docker://') && /^(\.\/)?[\w./-]*Dockerfile[\w.-]*$/.test(value)) return null;
  return DIGEST.test(ref) ? null : `${value} has no image digest; write ref@sha256:<64 hex>`;
}

/**
 * Reads one file's text. Returns the number of references seen and the violations, each with its 1-based line.
 * @param {string} text
 * @param {{ isActionFile?: boolean }} [opts]
 */
export function checkText(text, { isActionFile = false } = {}) {
  const lines = text.split(/\r?\n/);
  const violations = [];
  let refs = 0;
  let blockCol = null; // the column of the key that opened a block scalar, while its lines are being skipped
  let matrixCol = null; // the column of a `matrix:` key, while its lines are being read: its keys are names, not references

  const visit = (kind, raw, lineNo) => {
    const value = scalar(raw);
    if (kind !== 'uses' && value === '') return; // opens a mapping
    refs += 1;
    const why = kind === 'uses' ? checkUses(value) : checkImage(value, isActionFile);
    if (why) violations.push({ line: lineNo, message: why });
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const first = line.search(/\S/);
    if (first < 0) continue; // blank
    if (blockCol !== null) {
      if (first > blockCol) continue; // a line of the script
      blockCol = null;
    }
    if (line[first] === '#') continue;

    // The column of the key: after any `- ` markers of a sequence entry.
    const dash = /^(\s*(?:-\s+)*)/.exec(line)[1];
    const keyCol = dash.length;
    const rest = line.slice(keyCol);

    // A matrix names variants (`image: [hub, agent]`). An image that a job pulls by one of them is written as an expression,
    // which is refused wherever it stands, so skipping the keys of the matrix hides no reference.
    if (matrixCol !== null && first <= matrixCol) matrixCol = null;
    if (matrixCol === null && /^matrix\s*:\s*(#.*)?$/.test(rest)) {
      matrixCol = keyCol;
      continue;
    }
    if (matrixCol !== null) continue;

    if (/^[^#]*?:\s+[|>][+-]?\d*[+-]?\s*(#.*)?$/.test(rest) || /^[|>][+-]?\d*[+-]?\s*(#.*)?$/.test(rest)) {
      blockCol = keyCol;
      // A key such as `uses: |` is not a reference; fall through only for the flow-style scan below.
    }

    const key = /^(uses|image|container)\s*:(?:\s+(.*))?$/.exec(rest);
    if (key && blockCol === null) {
      visit(key[1], key[2] ?? '', i + 1);
      continue;
    }
    // Flow style: `- { uses: actions/checkout@v4 }`.
    for (const m of rest.matchAll(/[{,]\s*(uses|image)\s*:\s*([^,}]+)/g)) visit(m[1], m[2], i + 1);
  }
  return { refs, violations };
}

function* yamlFiles(path) {
  const st = statSync(path);
  if (st.isFile()) {
    yield path;
    return;
  }
  for (const entry of readdirSync(path, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const full = join(path, entry.name);
    if (entry.isDirectory()) yield* yamlFiles(full);
    else if (entry.isFile() && /\.ya?ml$/.test(entry.name)) yield full;
  }
}

function main(argv) {
  const here = dirname(fileURLToPath(import.meta.url));
  const repo = resolve(here, '..', '..');
  let targets = argv;
  if (targets.length === 0) {
    targets = [join(repo, '.github', 'workflows')];
    if (existsSync(join(repo, '.github', 'actions'))) targets.push(join(repo, '.github', 'actions'));
  }

  const files = [];
  for (const t of targets) {
    if (!existsSync(t)) {
      console.error(`check-pins: ${t} does not exist`);
      return 2;
    }
    files.push(...yamlFiles(t));
  }
  if (files.length === 0) {
    console.error(`check-pins: no workflow or action files under ${targets.join(', ')}; refusing to pass on nothing`);
    return 2;
  }

  let refs = 0;
  let bad = 0;
  for (const file of files) {
    const result = checkText(readFileSync(file, 'utf8'), { isActionFile: /^action\.ya?ml$/.test(basename(file)) });
    refs += result.refs;
    bad += result.violations.length;
    const shown = relative(process.cwd(), file) || file;
    for (const v of result.violations) console.log(`${shown.startsWith('..') ? file : shown}:${v.line}: ${v.message}`);
  }
  if (bad > 0) {
    console.log(`check-pins: ${bad} of ${refs} references are not pinned (S19)`);
    return 1;
  }
  console.log(`check-pins: ${refs} references checked in ${files.length} files, all pinned`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
