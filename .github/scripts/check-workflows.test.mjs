// Tests for check-workflows.mjs (09/T2; S18, S19, S20): the rules that actionlint and zizmor do not enforce for us.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..');
const checker = join(here, 'check-workflows.mjs');
const fixtures = join(here, 'fixtures', 'workflows');

function run(...args) {
  const r = spawnSync(process.execPath, [checker, ...args], { encoding: 'utf8', cwd: root });
  return { code: r.status, out: `${r.stdout}${r.stderr}` };
}
const fx = (name) => join(fixtures, name);

for (const name of ['good.yml', 'good-read-only-top.yml', 'good-idtoken-no-build.yml']) {
  test(`${name} passes`, () => {
    const r = run(fx(name));
    assert.equal(r.code, 0, r.out);
  });
}

const refused = [
  ['bad-no-permissions.yml', /bad-no-permissions\.yml:1: .*top-level permissions/],
  ['bad-write-all.yml', /bad-write-all\.yml:5: .*write-all/],
  ['bad-read-all.yml', /bad-read-all\.yml:5: .*read-all/],
  ['bad-top-write.yml', /bad-top-write\.yml:7: .*packages: write.*top level/],
  ['bad-no-timeout.yml', /bad-no-timeout\.yml:12: .*job slow.*timeout-minutes/],
  ['bad-timeout-too-long.yml', /bad-timeout-too-long\.yml:9: .*timeout-minutes.*1000.*360/],
  ['bad-pull-request-target.yml', /bad-pull-request-target\.yml:3: .*pull_request_target/],
  ['bad-pr-without-paths.yml', /bad-pr-without-paths\.yml:3: .*pull_request.*paths/],
  ['bad-pr-inline.yml', /bad-pr-inline\.yml:2: .*pull_request.*paths/],
  ['bad-job-write-all.yml', /bad-job-write-all\.yml:10: .*write-all/],
  // W6: a job that can mint an OIDC token runs no build step. Line numbers are those of the command.
  ['bad-idtoken-cargo-install.yml', /bad-idtoken-cargo-install\.yml:22: .*job sign-release.*id-token: write.*cargo install.*W6/],
  ['bad-idtoken-npm-flow.yml', /bad-idtoken-npm-flow\.yml:12: .*job publish.*npm ci.*W6/],
  ['bad-idtoken-build-action.yml', /bad-idtoken-build-action\.yml:14: .*job publish.*docker\/build-push-action.*W6/],
];
for (const [file, pattern] of refused) {
  test(`${file} is refused`, () => {
    const r = run(fx(file));
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, pattern);
  });
}

test('every build command behind sudo, env, a subshell or a chain is found in a job with the token', () => {
  const r = run(fx('bad-idtoken-chained.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-idtoken-chained\.yml:18: .*cargo \+stable build/);
  assert.match(r.out, /bad-idtoken-chained\.yml:20: .*pnpm install/);
  assert.match(r.out, /bad-idtoken-chained\.yml:21: .*go build/);
  assert.match(r.out, /bad-idtoken-chained\.yml:22: .*yarn/);
  assert.equal(r.out.split('\n').filter((l) => /W6/.test(l)).length, 4, r.out);
});

test('a job without the token may build, and prose that names a command is not a command', () => {
  const r = run(fx('good-idtoken-no-build.yml'));
  assert.equal(r.code, 0, r.out);
});

test('a job that calls a reusable workflow needs no timeout of its own', () => {
  const r = run(fx('good.yml'));
  assert.equal(r.code, 0, r.out);
  assert.doesNotMatch(r.out, /job call/);
});

test('nothing to check is an error, never a pass', () => {
  const r = run(join(fixtures, 'does-not-exist'));
  assert.equal(r.code, 2, r.out);
});

test('the repository workflows follow the rules', () => {
  const r = run();
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /\d+ workflows checked/);
});
