// Tests for check-pins.mjs (09/T2; S19, S20). Run with `node --test .github/scripts/*.test.mjs` (the Justfile's pins-test).
//
// The checker is run as a child process, the way CI runs it, against fixtures: one file per kind of reference that must be
// refused (a tag, a branch, a short SHA, an undigested docker:// reference, ...) and one that must be accepted. The last
// tests run it on the repository's own workflows, and on copies of them with one reference unpinned.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..');
const checker = join(here, 'check-pins.mjs');
const fixtures = join(here, 'fixtures', 'pins');

function run(...args) {
  const r = spawnSync(process.execPath, [checker, ...args], { encoding: 'utf8', cwd: root });
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}
const fx = (name) => join(fixtures, name);

test('a file whose every reference is pinned passes', () => {
  const r = run(fx('good.yml'));
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /8 references checked/, 'the count shows the checker read the references');
});

test('text inside a block scalar is not a reference', () => {
  const r = run(fx('block-scalar-text.yml'));
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /1 references checked/);
});

test('a matrix key called image is a name, not an image', () => {
  const r = run(fx('matrix-image-key.yml'));
  assert.equal(r.code, 0, r.out);
});

const refused = [
  ['bad-matrix-expression.yml', /bad-matrix-expression\.yml:11: .*expression/],
  ['bad-after-matrix.yml', /bad-after-matrix\.yml:12: .*postgres:16.*digest/],
  ['bad-tag.yml', /bad-tag\.yml:7: .*actions\/checkout@v4.*tag or branch/],
  ['bad-branch.yml', /bad-branch\.yml:7: .*actions\/checkout@main.*tag or branch/],
  ['bad-short-sha.yml', /bad-short-sha\.yml:7: .*short SHA/],
  ['bad-uppercase-sha.yml', /bad-uppercase-sha\.yml:7: .*lowercase/],
  ['bad-no-ref.yml', /bad-no-ref\.yml:7: .*no @ref/],
  ['bad-expression.yml', /bad-expression\.yml:7: .*expression/],
  ['bad-docker-tag.yml', /bad-docker-tag\.yml:7: .*docker:\/\/alpine:3\.20.*digest/],
  ['bad-docker-sha-tag-only.yml', /bad-docker-sha-tag-only\.yml:7: .*digest/],
  ['bad-service-image.yml', /bad-service-image\.yml:8: .*pgvector\/pgvector:pg16.*digest/],
  ['bad-container-scalar.yml', /bad-container-scalar\.yml:6: .*node:22.*digest/],
  ['bad-container-image.yml', /bad-container-image\.yml:7: .*node:22.*digest/],
  ['bad-reusable-workflow.yml', /bad-reusable-workflow\.yml:5: .*w\.yml@v1.*tag or branch/],
  ['bad-flow-style.yml', /bad-flow-style\.yml:7: .*actions\/checkout@v4/],
];
for (const [file, pattern] of refused) {
  test(`${file} is refused`, () => {
    const r = run(fx(file));
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, pattern);
  });
}

test('every violation in a file is reported, not only the first', () => {
  const r = run(fx('bad-many.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-many\.yml:7: /);
  assert.match(r.out, /bad-many\.yml:8: /);
  assert.match(r.out, /bad-many\.yml:9: /);
  assert.match(r.out, /3 of 3 references are not pinned/);
});

test('a composite action is read like a workflow', () => {
  assert.equal(run(fx('action-good')).code, 0);
  const r = run(fx('action-bad-uses'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /action\.yml:6: .*actions\/checkout@v4/);
});

test('a docker action may build its own Dockerfile but not pull an image by tag', () => {
  assert.equal(run(fx('action-good-docker')).code, 0);
  const r = run(fx('action-bad-docker'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /action\.yml:5: .*docker:\/\/alpine:3\.20.*digest/);
});

test('a directory is searched recursively', () => {
  const r = run(fixtures);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /action-bad-uses\/action\.yml:6/);
  assert.doesNotMatch(r.out, /good\.yml:\d+/);
});

test('nothing to check is an error, never a pass', () => {
  const empty = mkdtempSync(join(tmpdir(), 'pins-empty-'));
  try {
    const r = run(empty);
    assert.equal(r.code, 2, r.out);
    assert.match(r.out, /no workflow or action files/);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});

test('a path that does not exist is an error', () => {
  const r = run(join(fixtures, 'does-not-exist.yml'));
  assert.equal(r.code, 2, r.out);
  assert.match(r.out, /does-not-exist\.yml/);
});

test('the repository workflows and actions are all pinned', () => {
  const r = run();
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /\d+ references checked in \d+ files/);
});

test('unpinning any single reference of a real workflow fails the check', () => {
  const dir = join(root, '.github', 'workflows');
  const tmp = mkdtempSync(join(tmpdir(), 'pins-mutant-'));
  try {
    let mutated = 0;
    for (const name of readdirSync(dir).filter((n) => /\.ya?ml$/.test(n))) {
      const text = readFileSync(join(dir, name), 'utf8');
      const lines = text.split('\n');
      const idx = lines.findIndex((l) => /^\s*(-\s+)?uses:\s*[^./\s][^@\s]*@[0-9a-f]{40}/.test(l));
      if (idx < 0) continue;
      lines[idx] = lines[idx].replace(/@[0-9a-f]{40}/, '@v1');
      const file = join(tmp, name);
      writeFileSync(file, lines.join('\n'));
      const r = run(file);
      assert.equal(r.code, 1, `${name}: ${r.out}`);
      assert.match(r.out, new RegExp(`${name}:${idx + 1}: `));
      mutated += 1;
    }
    assert.ok(mutated > 0, 'at least one real workflow was mutated');
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});

test('a tag in a workflow under the default search path fails the default run', () => {
  const tmp = mkdtempSync(join(tmpdir(), 'pins-default-'));
  try {
    // The default run reads <root>/.github/workflows and <root>/.github/actions of the checkout the script lives in.
    mkdirSync(join(tmp, '.github', 'scripts'), { recursive: true });
    mkdirSync(join(tmp, '.github', 'workflows'), { recursive: true });
    cpSync(checker, join(tmp, '.github', 'scripts', 'check-pins.mjs'));
    cpSync(fx('bad-tag.yml'), join(tmp, '.github', 'workflows', 'bad-tag.yml'));
    const r = spawnSync(process.execPath, [join(tmp, '.github', 'scripts', 'check-pins.mjs')], { encoding: 'utf8' });
    assert.equal(r.status, 1, `${r.stdout}${r.stderr}`);
    assert.match(`${r.stdout}${r.stderr}`, /bad-tag\.yml:7/);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});
