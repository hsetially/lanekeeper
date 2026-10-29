#!/usr/bin/env node
// Checks deploy/images/hub-model.lock (09/T1; S19, S20): the embedding model's files are pinned to one Hugging Face commit,
// and each has a size and a sha256 the image build verifies. No dependencies.
//
//   node deploy/images/model-lock.mjs check deploy/images/hub-model.lock
//
// The hub Dockerfile's `model` stage applies the same name, repo, revision and path patterns in shell, so a lock this
// checker accepts is one the build can fetch, and a path it refuses can never be written outside /models/<name>/.
import { readFileSync } from 'node:fs';
import { argv, exit } from 'node:process';
import { fileURLToPath } from 'node:url';

const MIN_AGE_DAYS = 14; // AGENTS.md rule 15
const MAX_SIZE = 2 * 1024 ** 3;
const NAME = /^[a-z0-9][a-z0-9.-]*$/;
const REPO = /^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/;
const REVISION = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const SIZE = /^[1-9][0-9]*$/;
// Segments start with a letter, digit, `_` or `-`: no `.`, `..` or hidden files, no `/` at either end.
const PATH = /^([A-Za-z0-9_-][A-Za-z0-9._-]*\/)*[A-Za-z0-9_-][A-Za-z0-9._-]*$/;
const DATE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/;

export function urlOf(lock, file) {
  return `https://huggingface.co/${lock.repo}/resolve/${lock.revision}/${file.path}`;
}

export function checkLock(text, { now = new Date() } = {}) {
  const errors = [];
  const single = { model: [], repo: [], revision: [], date: [] };
  const files = [];
  text.split('\n').forEach((raw, index) => {
    const line = raw.trim();
    const at = `line ${index + 1}`;
    if (line === '' || line.startsWith('#')) return;
    const [key, ...rest] = line.split(/\s+/);
    if (key in single) {
      if (rest.length !== 1) errors.push(`${at}: \`${key}\` takes one value`);
      single[key].push(rest[0] ?? '');
    } else if (key === 'file') {
      if (rest.length !== 3) {
        errors.push(`${at}: a file line is \`file <sha256> <size> <path>\`, got ${rest.length} fields`);
        return;
      }
      const [sha256, size, path] = rest;
      if (!SHA256.test(sha256)) errors.push(`${at}: the sha256 must be 64 lower-case hex digits: ${sha256}`);
      if (!SIZE.test(size) || Number(size) > MAX_SIZE) errors.push(`${at}: the size must be a byte count up to 2 GiB: ${size}`);
      if (!PATH.test(path)) errors.push(`${at}: the path must be relative, without . or .. segments: ${path}`);
      files.push({ sha256, size: Number(size), path });
    } else {
      errors.push(`${at}: unknown line \`${key}\``);
    }
  });

  for (const [key, values] of Object.entries(single)) {
    if (values.length !== 1) errors.push(`\`${key}\` must appear exactly once, found ${values.length}`);
  }
  const [model = '', repo = '', revision = '', date = ''] = ['model', 'repo', 'revision', 'date'].map((k) => single[k][0]);
  if (revision === 'UNRESOLVED') {
    errors.push('the lock is UNRESOLVED: run deploy/images/update-model-lock.sh where huggingface.co and its LFS CDN are reachable');
  } else if (single.revision.length === 1 && !REVISION.test(revision)) {
    errors.push(`the revision must be a full 40-hex commit, never a branch or tag: ${revision}`);
  }
  if (single.model.length === 1 && !NAME.test(model)) errors.push(`the model must be one directory name: ${model}`);
  if (single.repo.length === 1 && !REPO.test(repo)) errors.push(`the repo must be <owner>/<name> on huggingface.co: ${repo}`);
  if (revision !== 'UNRESOLVED') {
    if (single.date.length === 1 && !DATE.test(date)) {
      errors.push(`the date must be the revision's commit time in UTC (ISO 8601): ${date}`);
    } else if (single.date.length === 1) {
      const age = (now.getTime() - Date.parse(date)) / 86_400_000;
      if (age < MIN_AGE_DAYS) errors.push(`the revision is ${age.toFixed(1)} days old; it must be at least ${MIN_AGE_DAYS} days old`);
    }
    if (files.length === 0) errors.push('no file lines');
    const seen = new Set();
    for (const f of files) {
      if (seen.has(f.path)) errors.push(`the path ${f.path} is listed twice`);
      seen.add(f.path);
    }
    if (files.length > 0 && !files.some((f) => f.path.endsWith('.onnx'))) errors.push('no .onnx model file');
    if (files.length > 0 && !files.some((f) => f.path === 'tokenizer.json')) errors.push('no tokenizer.json');
  }
  return { errors, lock: { model, repo, revision, date, files } };
}

if (argv[1] === fileURLToPath(import.meta.url)) {
  const [command, path] = argv.slice(2);
  if (command !== 'check' || !path) {
    console.error('usage: model-lock.mjs check <lock file>');
    exit(2);
  }
  const { errors, lock } = checkLock(readFileSync(path, 'utf8'));
  if (errors.length > 0) {
    for (const e of errors) console.error(`${path}: ${e}`);
    exit(1);
  }
  console.log(`${path}: ${lock.files.length} files of ${lock.repo} at ${lock.revision}`);
}
