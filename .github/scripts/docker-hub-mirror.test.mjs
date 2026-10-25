// Tests for docker-hub-mirror.mjs (09/T2; S19, S20): the Docker daemon settings that send Docker Hub pulls through a mirror.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const script = join(here, 'docker-hub-mirror.mjs');

function run(args, existing) {
  const dir = mkdtempSync(join(tmpdir(), 'mirror-'));
  try {
    const argv = [script, ...args];
    if (existing !== undefined) {
      const file = join(dir, 'daemon.json');
      writeFileSync(file, existing);
      argv.push(file);
    } else {
      argv.push(join(dir, 'missing.json'));
    }
    const r = spawnSync(process.execPath, argv, { encoding: 'utf8' });
    return { code: r.status, out: r.stdout, err: r.stderr };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('without a daemon.json the result names only the mirror', () => {
  const r = run(['mirror.gcr.io']);
  assert.equal(r.code, 0, r.err);
  assert.deepEqual(JSON.parse(r.out), { 'registry-mirrors': ['https://mirror.gcr.io'] });
});

test('other settings of the daemon are kept', () => {
  const r = run(['mirror.gcr.io'], '{"features":{"containerd-snapshotter":true},"log-level":"warn"}');
  assert.equal(r.code, 0, r.err);
  assert.deepEqual(JSON.parse(r.out), {
    features: { 'containerd-snapshotter': true },
    'log-level': 'warn',
    'registry-mirrors': ['https://mirror.gcr.io'],
  });
});

test('a mirror that is already listed is not listed twice, and others stay after it', () => {
  const r = run(['mirror.gcr.io'], '{"registry-mirrors":["https://other.example","https://mirror.gcr.io"]}');
  assert.equal(r.code, 0, r.err);
  assert.deepEqual(JSON.parse(r.out)['registry-mirrors'], ['https://mirror.gcr.io', 'https://other.example']);
});

test('an empty daemon.json is the same as none', () => {
  const r = run(['mirror.gcr.io'], '');
  assert.equal(r.code, 0, r.err);
  assert.deepEqual(JSON.parse(r.out), { 'registry-mirrors': ['https://mirror.gcr.io'] });
});

for (const bad of ['', 'https://mirror.gcr.io', 'mirror.gcr.io/library', 'mirror gcr.io', 'mirror.gcr.io;rm', '-x', 'Mirror.GCR.io:99999999']) {
  test(`the mirror host ${JSON.stringify(bad)} is refused`, () => {
    const r = run([bad]);
    assert.equal(r.code, 2, r.out + r.err);
    assert.match(r.err, /host/);
  });
}

test('a daemon.json that is not an object is refused, never overwritten', () => {
  for (const text of ['[1]', '"x"', '{', 'null']) {
    const r = run(['mirror.gcr.io'], text);
    assert.equal(r.code, 2, `${text}: ${r.out}${r.err}`);
    assert.match(r.err, /daemon\.json/);
  }
});

test('registry-mirrors that is not a list is refused', () => {
  const r = run(['mirror.gcr.io'], '{"registry-mirrors":"https://x"}');
  assert.equal(r.code, 2, r.out + r.err);
  assert.match(r.err, /registry-mirrors/);
});
