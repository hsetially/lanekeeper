// Pins that appear in more than one place must agree (09/T2; S19). The Justfile holds the versions of the tools, tools.env the
// images; the workflows install the same versions by name. A workflow that installs another version than the Justfile checks
// would pass here and fail there, or worse, run a tool that nobody reviewed.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const read = (path) => readFileSync(join(root, path), 'utf8');
const justfile = read('Justfile');
const workflows = readdirSync(join(root, '.github', 'workflows'))
  .filter((n) => /\.ya?ml$/.test(n))
  .map((n) => ({ name: n, text: read(`.github/workflows/${n}`) }));

/** The value of `name := "value"` in the Justfile. */
function variable(name) {
  const m = new RegExp(`^${name} := "([^"]*)"`, 'm').exec(justfile);
  assert.ok(m, `the Justfile has no ${name}`);
  return m[1];
}

test('every workflow installs the same just, cargo-deny and cargo-audit as the Justfile names', () => {
  const wanted = { just: variable('just_version'), 'cargo-deny': variable('cargo_deny_version'), 'cargo-audit': variable('cargo_audit_version') };
  let seen = 0;
  for (const { name, text } of workflows) {
    for (const [tool, version] of Object.entries(wanted)) {
      for (const m of text.matchAll(new RegExp(`(?<![\\w-])${tool}@([0-9][0-9.]*)`, 'g'))) {
        seen += 1;
        assert.equal(m[1], version, `${name} installs ${tool}@${m[1]}, the Justfile pins ${version}`);
      }
    }
  }
  assert.ok(seen >= 8, `only ${seen} installs were found: the check is not reading the workflows`);
});

test('sqlx-cli is the version of the sqlx crate', () => {
  const m = /^sqlx = \{ version = "=([0-9.]+)"/m.exec(read('Cargo.toml'));
  assert.ok(m, 'Cargo.toml pins no exact sqlx version');
  assert.equal(variable('sqlx_cli_version'), m[1]);
});

test('the Postgres image of every workflow is the one of deploy/dev/compose.yaml', () => {
  const compose = /image: pgvector\/pgvector:pg16@(sha256:[0-9a-f]{64})/.exec(read('deploy/dev/compose.yaml'));
  assert.ok(compose, 'compose.yaml pins no pgvector image');
  let seen = 0;
  for (const { name, text } of workflows) {
    for (const m of text.matchAll(/pgvector\/pgvector:pg16@(sha256:[0-9a-f]{64})/g)) {
      seen += 1;
      assert.equal(m[1], compose[1], `${name} uses another pgvector digest than deploy/dev/compose.yaml`);
    }
  }
  assert.ok(seen >= 3, `only ${seen} pgvector images were found`);
});

test('the Docker Hub mirror of the action is the one named in tools.env', () => {
  const env = /^LK_CI_DOCKER_HUB_MIRROR=(\S+)$/m.exec(read('deploy/images/tools.env'));
  assert.ok(env, 'tools.env names no mirror');
  const action = /default: (\S+)\s*\n\s*restart:/.exec(read('.github/actions/docker-hub-mirror/action.yml'));
  assert.ok(action, 'the action has no default mirror');
  assert.equal(action[1], env[1]);
  for (const { name, text } of workflows) {
    for (const m of text.matchAll(/mirror\.gcr\.io|[a-z0-9.-]+\/pgvector\/pgvector/g)) {
      if (m[0].endsWith('/pgvector/pgvector')) assert.equal(m[0].split('/')[0], env[1], `${name} reads a service image from another mirror`);
    }
  }
});

test('every fuzz recipe of the Justfile is in the fuzz matrix of rust.yml and nightly.yml', () => {
  const recipes = [...justfile.matchAll(/^([a-z][a-z0-9]*-fuzz|fuzz-smoke)(?: [^\n:]*)?:/gm)].map((m) => m[1]).sort();
  assert.ok(recipes.length >= 8, `only ${recipes.length} fuzz recipes were found: ${recipes}`);
  for (const file of ['rust.yml', 'nightly.yml']) {
    const text = workflows.find((w) => w.name === file).text;
    const block = /matrix:\s*\n\s*recipe:\s*\n((?:\s+- [a-z-]+\n)+)/.exec(text);
    assert.ok(block, `${file} has no fuzz matrix`);
    const listed = [...block[1].matchAll(/- ([a-z-]+)/g)].map((m) => m[1]).sort();
    assert.deepEqual(listed, recipes, `${file}: the fuzz matrix and the Justfile's fuzz recipes differ`);
  }
});

test('every tool pin of the Justfile has a version, and a sha256 of 64 hex digits', () => {
  for (const tool of ['actionlint', 'kubeconform', 'kind', 'osv_scanner', 'zizmor', 'cosign', 'crane', 'shellcheck']) {
    assert.match(variable(`${tool}_version`), /^\d+\.\d+\.\d+$/, `${tool} version`);
    assert.match(variable(`${tool}_sha256`), /^[0-9a-f]{64}$/, `${tool} sha256`);
  }
});

test('every tool image of tools.env is pinned by digest', () => {
  const lines = read('deploy/images/tools.env').split('\n').filter((l) => /^LK_[A-Z_]+_IMAGE=/.test(l));
  assert.ok(lines.length >= 5);
  for (const l of lines) assert.match(l, /@sha256:[0-9a-f]{64}$/, l);
});
