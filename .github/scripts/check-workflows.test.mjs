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
  // W1 and W2 read a quoted value as YAML does: "write-all" and 'write' are the same grants as the bare words.
  ['bad-write-all-quoted.yml', /bad-write-all-quoted\.yml:5: permissions: write-all is too wide/],
  ['bad-job-write-all-quoted.yml', /bad-job-write-all-quoted\.yml:10: job a: permissions: write-all is too wide/],
  ['bad-top-write-quoted.yml', /bad-top-write-quoted\.yml:7: .*packages: write.*top level/],
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

// W6, the forms that earlier versions let through (review round 2): quoted values, a shell that is given the command as a string, a build
// inside a container or by docker, a package manager behind python, and code that is downloaded and run. Each line is one finding.
const found = (out, file) => out.split('\n').filter((l) => l.includes(`${file}:`) && /W6/.test(l));

test('a quoted id-token value, in a block or in a flow mapping, still holds the token', () => {
  const r = run(fx('bad-idtoken-quoted-token.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-idtoken-quoted-token\.yml:14: .*job sign.*id-token: write.*cargo install.*W6/);
  assert.match(r.out, /bad-idtoken-quoted-token\.yml:20: .*job flow.*id-token: write.*npm ci.*W6/);
  assert.equal(found(r.out, 'bad-idtoken-quoted-token.yml').length, 2, r.out);
});

test('a quoted run and a quoted uses are read as the commands they are', () => {
  const r = run(fx('bad-idtoken-quoted-run.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-idtoken-quoted-run\.yml:14: .*cargo install.*W6/);
  assert.match(r.out, /bad-idtoken-quoted-run\.yml:15: .*npm ci.*W6/);
  assert.match(r.out, /bad-idtoken-quoted-run\.yml:16: .*docker\/build-push-action.*W6/);
  assert.equal(found(r.out, 'bad-idtoken-quoted-run.yml').length, 3, r.out);
});

test('a command given to sh -c or bash -c as a string is read, behind sudo, env, timeout and flags', () => {
  const r = run(fx('bad-idtoken-shell-c.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-idtoken-shell-c\.yml:14: .*cargo build.*W6/);
  assert.match(r.out, /bad-idtoken-shell-c\.yml:15: .*npm ci.*W6/);
  assert.match(r.out, /bad-idtoken-shell-c\.yml:16: .*pip install.*W6/);
  assert.match(r.out, /bad-idtoken-shell-c\.yml:19: .*yarn install.*W6/);
  assert.match(r.out, /bad-idtoken-shell-c\.yml:20: .*make all.*W6/);
  assert.equal(found(r.out, 'bad-idtoken-shell-c.yml').length, 5, r.out);
});

test('a shell inside a shell is opened, and sh -c of a download is refused', () => {
  const r = run(fx('bad-idtoken-shell-nested.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-idtoken-shell-nested\.yml:14: .*sh -c "\$\(curl.*W6/);
  assert.match(r.out, /bad-idtoken-shell-nested\.yml:15: .*bash -c "bash -c 'cargo build'".*W6/);
  assert.equal(found(r.out, 'bad-idtoken-shell-nested.yml').length, 2, r.out);
});

test('a docker run of a build tool, and docker build, buildx build and compose build, are build steps; a continued line is one command', () => {
  const r = run(fx('bad-idtoken-docker-run.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-idtoken-docker-run\.yml:14: .*docker run.*cargo build.*W6/);
  assert.match(r.out, /bad-idtoken-docker-run\.yml:16: .*docker run.*npm ci.*W6/);
  assert.match(r.out, /bad-idtoken-docker-run\.yml:19: .*docker build.*W6/);
  assert.match(r.out, /bad-idtoken-docker-run\.yml:20: .*docker buildx build.*W6/);
  assert.match(r.out, /bad-idtoken-docker-run\.yml:21: .*docker compose build.*W6/);
  assert.equal(found(r.out, 'bad-idtoken-docker-run.yml').length, 5, r.out);
});

test('a package manager behind python or uv, rustup, and code that is downloaded and run are refused', () => {
  const r = run(fx('bad-idtoken-fetch.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-idtoken-fetch\.yml:14: .*pip install.*W6/);
  assert.match(r.out, /bad-idtoken-fetch\.yml:15: .*curl .*\| sh.*W6/);
  assert.match(r.out, /bad-idtoken-fetch\.yml:16: .*wget .*sudo bash.*W6/);
  assert.match(r.out, /bad-idtoken-fetch\.yml:17: .*bash <\(curl.*W6/);
  assert.match(r.out, /bad-idtoken-fetch\.yml:18: .*eval .*curl.*W6/);
  assert.match(r.out, /bad-idtoken-fetch\.yml:19: .*rustup run.*W6/);
  assert.match(r.out, /bad-idtoken-fetch\.yml:20: .*uv pip install.*W6/);
  assert.equal(found(r.out, 'bad-idtoken-fetch.yml').length, 7, r.out);
});

// W6, review round 3: a tool called by its path is the tool (`/usr/bin/cargo build`, `~/.cargo/bin/cargo install`, `$HOME/.cargo/bin/cargo`),
// and a download that is saved and run in the same command line (`wget -O f URL && sh f`) is a download that is run.
test('a build tool, a shell, docker and a download called by a path are refused, and so is a saved download that is run at once', () => {
  const r = run(fx('bad-idtoken-tool-path.yml'));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:14: .*\/usr\/bin\/cargo build.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:15: .*~\/\.cargo\/bin\/cargo install.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:16: .*\$HOME\/\.cargo\/bin\/cargo build.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:18: .*rustup.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:19: .*cargo install.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:20: .*apt-get install.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:21: .*npm ci.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:22: .*bash -c 'cargo build'.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:23: .*docker build.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:24: .*docker run.*cargo.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:25: .*curl .*\| \/bin\/sh.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:26: .*wget -O f .*sh f.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:27: .*curl .*bash get\.sh.*W6/);
  assert.match(r.out, /bad-idtoken-tool-path\.yml:28: .*\/usr\/bin\/env CI=1 cargo build.*W6/);
  assert.equal(found(r.out, 'bad-idtoken-tool-path.yml').length, 14, r.out);
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
