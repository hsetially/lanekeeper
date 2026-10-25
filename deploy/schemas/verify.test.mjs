// Tests for verify.mjs: the committed schemas equal the lock, and the lock agrees with sources.json. Run: node --test deploy/schemas/*.test.mjs
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

import { verify } from './verify.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const sha = (text) => createHash('sha256').update(text).digest('hex');
const COMMIT = 'a'.repeat(40);

/** A schema directory with one file of each kind, a lock and a sources file that agree. */
function fixture() {
  const dir = mkdtempSync(join(tmpdir(), 'schemas-'));
  const files = { 'k8s/v1.34.0-standalone-strict/service-v1.json': '{"a":1}\n', 'crds/example.dev/widget_v1.json': '{"b":2}\n' };
  const lockFiles = {};
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), text);
    lockFiles[path] = {
      sha256: sha(text),
      bytes: Buffer.byteLength(text),
      source: `https://raw.githubusercontent.com/example/repo/${COMMIT}/${path}`,
      sourceSha256: sha(text),
      how: 'copied',
    };
  }
  writeFileSync(join(dir, 'schemas.lock.json'), JSON.stringify({ schema: 1, kubernetes: '1.34.0', files: lockFiles }));
  writeFileSync(
    join(dir, 'sources.json'),
    JSON.stringify({
      kubernetes: { version: '1.34.0', repo: 'example/repo', commit: COMMIT, directory: 'v1.34.0-standalone-strict', kinds: ['service-v1'] },
      crds: [{ repo: 'example/repo', commit: COMMIT, files: ['crds/example.dev/widget_v1.json'] }],
      ready: [],
    }),
  );
  return dir;
}

const withFixture = (change) => {
  const dir = fixture();
  try {
    change?.(dir);
    return verify(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

test('a directory that equals its lock passes', () => {
  assert.deepEqual(withFixture(), []);
});

test('a changed byte in a schema is found', () => {
  const problems = withFixture((dir) => writeFileSync(join(dir, 'crds/example.dev/widget_v1.json'), '{"b":3}\n'));
  assert.ok(problems.some((p) => p.includes('widget_v1.json') && p.includes('sha256')), problems.join('\n'));
});

test('a schema that is not in the lock is found', () => {
  const problems = withFixture((dir) => writeFileSync(join(dir, 'crds/example.dev/extra_v1.json'), '{}\n'));
  assert.ok(problems.some((p) => p.includes('extra_v1.json') && p.includes('not in the lock')), problems.join('\n'));
});

test('a schema that the lock lists but is missing is found', () => {
  const problems = withFixture((dir) => rmSync(join(dir, 'k8s/v1.34.0-standalone-strict/service-v1.json')));
  assert.ok(problems.some((p) => p.includes('service-v1.json') && p.includes('missing')), problems.join('\n'));
});

test('a lock entry whose source commit is not the one pinned in sources.json is found', () => {
  const problems = withFixture((dir) => {
    const lock = JSON.parse(readFileSync(join(dir, 'schemas.lock.json'), 'utf8'));
    lock.files['crds/example.dev/widget_v1.json'].source = `https://raw.githubusercontent.com/example/repo/${'b'.repeat(40)}/x.yaml`;
    writeFileSync(join(dir, 'schemas.lock.json'), JSON.stringify(lock));
  });
  assert.ok(problems.some((p) => p.includes('widget_v1.json') && p.includes('commit')), problems.join('\n'));
});

test('a source that is not a pinned 40-character commit is found', () => {
  const problems = withFixture((dir) => {
    const sources = JSON.parse(readFileSync(join(dir, 'sources.json'), 'utf8'));
    sources.crds[0].commit = 'main';
    writeFileSync(join(dir, 'sources.json'), JSON.stringify(sources));
  });
  assert.ok(problems.some((p) => p.includes('main') && p.includes('40')), problems.join('\n'));
});

test('a kubernetes version that differs between the lock and sources.json is found', () => {
  const problems = withFixture((dir) => {
    const lock = JSON.parse(readFileSync(join(dir, 'schemas.lock.json'), 'utf8'));
    lock.kubernetes = '1.33.0';
    writeFileSync(join(dir, 'schemas.lock.json'), JSON.stringify(lock));
  });
  assert.ok(problems.some((p) => p.includes('1.33.0')), problems.join('\n'));
});

test('a file that is not valid JSON is found even if the lock matches it', () => {
  const problems = withFixture((dir) => {
    const path = 'crds/example.dev/widget_v1.json';
    writeFileSync(join(dir, path), 'not json');
    const lock = JSON.parse(readFileSync(join(dir, 'schemas.lock.json'), 'utf8'));
    lock.files[path].sha256 = sha('not json');
    lock.files[path].bytes = 8;
    writeFileSync(join(dir, 'schemas.lock.json'), JSON.stringify(lock));
  });
  assert.ok(problems.some((p) => p.includes('widget_v1.json') && p.includes('JSON')), problems.join('\n'));
});

test('the committed schemas equal the committed lock', () => {
  assert.ok(existsSync(join(here, 'schemas.lock.json')));
  assert.deepEqual(verify(here), []);
});

test('the committed copy is not the fixture: it has the kinds the charts render', () => {
  const lock = JSON.parse(readFileSync(join(here, 'schemas.lock.json'), 'utf8'));
  for (const path of [
    'k8s/v1.34.0-standalone-strict/statefulset-apps-v1.json',
    'crds/gateway.networking.k8s.io/gateway_v1.json',
    'crds/networking.gke.io/gcpbackendpolicy_v1.json',
    'crds/compute.cnrm.cloud.google.com/computesecuritypolicy_v1beta1.json',
  ]) {
    assert.ok(path in lock.files, `${path} is not vendored`);
  }
});
