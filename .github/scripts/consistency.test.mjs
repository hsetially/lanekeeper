// Pins that appear in more than one place must agree (09/T2; S19). The Justfile holds the versions of the tools, tools.env the
// images; the workflows install the same versions by name. A workflow that installs another version than the Justfile checks
// would pass here and fail there, or worse, run a tool that nobody reviewed.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
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

// Disk space on a hosted runner (PR 25). Main run 40166747415 failed in `cargo test --workspace` with Postgres error 53100 "No space
// left on device": the debug build of the workspace and the Postgres containers with large seeded tables share the runner's root
// disk. The job that ran those tests moved from ci.yml to rust.yml and lost the two steps that PR added, so a job that does the same
// kind of work must carry them: remove what the job never uses before the first heavy step, and print `df` at the end even on failure.
const HEAVY = /cargo test --workspace|cargo bench --workspace|reproducible-check\.sh|just kind-e2e/;
const FREE_STEP = /^ +- name: Free disk space\n(?: +if: .*\n)? +run: \|\n +df -h \/\n +(sudo rm -rf [^\n]+)\n +df -h \/\n/m;

/** The jobs of a workflow as [{ name, text }], from the `jobs:` block (job keys sit at two spaces). */
function jobsOf(text) {
  const at = text.search(/^jobs:\s*$/m);
  assert.ok(at >= 0, 'a workflow without a jobs: block');
  const parts = text.slice(at).split(/^ {2}([A-Za-z0-9_-]+):\s*$/m);
  const jobs = [];
  for (let i = 1; i + 1 < parts.length; i += 2) jobs.push({ name: parts[i], text: parts[i + 1] });
  return jobs;
}

test('a job that runs the workspace tests, the benchmarks or the reproducible image build frees disk first and prints what is left', () => {
  let heavy = 0;
  const removals = new Set();
  for (const { name: file, text } of workflows) {
    for (const j of jobsOf(text)) {
      const job = { name: j.name, text: j.text.replace(/^ *#.*\n/gm, '') }; // a comment that names a command is not a step
      const first = job.text.search(HEAVY);
      if (first < 0) continue;
      heavy += 1;
      const where = `${file} job ${job.name}`;
      const free = FREE_STEP.exec(job.text);
      assert.ok(free, `${where} has no "Free disk space" step (df, sudo rm -rf of the unused toolchains, df) before its heavy step`);
      assert.ok(free.index < first, `${where}: the "Free disk space" step comes after the first heavy step`);
      removals.add(free[1]);
      assert.match(job.text, /^ +- name: Disk left\n +if: always\(\)\n +run: df -h \/\n/m, `${where} has no "Disk left" step with if: always()`);
    }
  }
  assert.ok(heavy >= 5, `only ${heavy} heavy jobs were found (rust test, images image, images kind-e2e, bench, nightly bench): the check is not reading the workflows`);
  assert.equal(removals.size, 1, `the jobs remove different paths, so the copies of the step have drifted apart: ${[...removals].join(' | ')}`);
});

// The kind end-to-end test (09 acceptance 3) names what it installs in deploy/tests/kind/pins.env, so a person can read every pin in
// one place. Each is a version and a digest or a hash, read from the registry or the release and never guessed (S19).
test('the pins of the kind end-to-end test are by digest and by sha256, and agree with the version they name', () => {
  const env = read('deploy/tests/kind/pins.env');
  const get = (name) => {
    const m = new RegExp(`^${name}=(\\S+)$`, 'm').exec(env);
    assert.ok(m, `pins.env has no ${name}`);
    return m[1];
  };
  assert.match(get('LK_KIND_NODE_IMAGE'), /^kindest\/node:v\d+\.\d+\.\d+@sha256:[0-9a-f]{64}$/);
  assert.match(get('LK_CNPG_OPERATOR_IMAGE'), /^ghcr\.io\/cloudnative-pg\/cloudnative-pg:\d+\.\d+\.\d+@sha256:[0-9a-f]{64}$/);
  assert.match(get('LK_CNPG_MANIFEST_SHA256'), /^[0-9a-f]{64}$/);
  const version = get('LK_CNPG_VERSION');
  assert.ok(get('LK_CNPG_MANIFEST_URL').endsWith(`/v${version}/cnpg-${version}.yaml`), 'the manifest URL is not the version of LK_CNPG_VERSION');
  assert.ok(get('LK_CNPG_OPERATOR_IMAGE').includes(`:${version}@`), 'the operator image is not the version of LK_CNPG_VERSION');
  // The db chart's README names the operator version the vendored CRDs are from: the test runs what the chart was checked against.
  assert.ok(read('deploy/charts/db/README.md').includes(`CloudNativePG **${version}**`), 'the db chart README names another operator version');
});

test('the kind configuration is one control plane and three workers, all from the pinned node image, and containerd reads certs.d', () => {
  const pinned = /^LK_KIND_NODE_IMAGE=(\S+)$/m.exec(read('deploy/tests/kind/pins.env'));
  assert.ok(pinned, 'pins.env names no node image');
  const r = spawnSync('bash', ['deploy/tests/kind/run.sh', '--print-kind-config'], { cwd: root, encoding: 'utf8' });
  assert.equal(r.status, 0, r.stderr);
  const roles = [...r.stdout.matchAll(/^ {2}- role: (\S+)$/gm)].map((m) => m[1]);
  assert.deepEqual(roles, ['control-plane', 'worker', 'worker', 'worker']);
  const images = [...r.stdout.matchAll(/^ {4}image: (\S+)$/gm)].map((m) => m[1]);
  assert.deepEqual(images, Array(4).fill(pinned[1]), 'every node comes from the image of pins.env');
  assert.match(r.stdout, /config_path = "\/etc\/containerd\/certs\.d"/);
});
