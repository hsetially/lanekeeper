// Tests for deploy/tests/kind/run.sh that need no cluster (09 acceptance 3; P11; review round 2, non-blocking 2). The cluster itself cannot be
// started on a cgroup v1 host, so what can be proved without one is proved here: the start-up verdict (deploy/tests/kind/lib.sh), and that
// the script no longer asks a pod for /readyz through the API server's proxy, which the hub chart's NetworkPolicy does not admit.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const run = readFileSync(join(root, 'deploy', 'tests', 'kind', 'run.sh'), 'utf8');

/** Runs p11_verdict from lib.sh with `input` on its standard input; returns { code, out, worst }. */
function verdict(budget, input) {
  const script = `set -u; . "${join(root, 'deploy', 'tests', 'kind', 'lib.sh')}"; p11_verdict ${budget} 2>&1; code=$?; echo "worst=\${P11_WORST:-unset}"; exit $code`;
  const r = spawnSync('bash', ['-c', script], { input, encoding: 'utf8' });
  const out = `${r.stdout}${r.stderr}`;
  return { code: r.status, out, worst: /worst=(\S+)/.exec(out)?.[1] };
}

test('replicas that are Ready within the budget pass, and the slowest is reported', () => {
  const r = verdict(10, 'lanekeeper-hub-0 4\nlanekeeper-hub-1 7\nlanekeeper-hub-2 10\n');
  assert.equal(r.code, 0, r.out);
  assert.equal(r.worst, '10');
  assert.match(r.out, /lanekeeper-hub-2: 10 s .*within the budget of 10 s/);
});

test('one replica over the budget fails, and it is named', () => {
  const r = verdict(10, 'lanekeeper-hub-0 4\nlanekeeper-hub-1 11\nlanekeeper-hub-2 5\n');
  assert.equal(r.code, 1, r.out);
  assert.equal(r.worst, '11');
  assert.match(r.out, /lanekeeper-hub-1: 11 s .*OVER/);
  assert.doesNotMatch(r.out, /lanekeeper-hub-0: .*OVER/);
});

test('no measurement is not a pass', () => {
  assert.equal(verdict(10, '').code, 2);
  assert.equal(verdict(10, '\n\n').code, 2);
});

test('a time that is not a whole number of seconds is refused, not read as zero', () => {
  for (const bad of ['lanekeeper-hub-0 null\n', 'lanekeeper-hub-0 -3\n', 'lanekeeper-hub-0 1.5\n', 'lanekeeper-hub-0\n']) {
    const r = verdict(10, bad);
    assert.equal(r.code, 2, `${JSON.stringify(bad)}: ${r.out}`);
  }
});

test('the budget is an argument that must be a whole number', () => {
  assert.equal(verdict('abc', 'lanekeeper-hub-0 1\n').code, 2);
});

test('the budget of the test is the registered budget of P11 (10000 ms)', () => {
  const budgets = readFileSync(join(root, 'perf', 'budgets.toml'), 'utf8');
  const block = /\[\[budget\]\]\s*\nid = "P11"\n([\s\S]*?)(?=\n\[\[budget\]\]|$)/.exec(budgets)?.[1] ?? '';
  const ms = Number(/threshold = (\d+)/.exec(block)?.[1]);
  assert.equal(/unit = "ms"/.test(block), true, block);
  const lib = readFileSync(join(root, 'deploy', 'tests', 'kind', 'lib.sh'), 'utf8');
  assert.equal(Number(/^P11_BUDGET_S=(\d+)$/m.exec(lib)?.[1]) * 1000, ms);
});

test('run.sh no longer reads /readyz through the API server proxy, and keeps the Ready condition as the check', () => {
  const code = run.split('\n').filter((l) => !/^\s*#/.test(l)).join('\n');
  assert.doesNotMatch(code, /get --raw/);
  assert.doesNotMatch(code, /\/proxy\/readyz/);
  assert.match(code, /condition == "Ready"|type == "Ready"/);
});

test('run.sh asserts P11 per replica once they are Ready, and says so when it could not measure', () => {
  assert.match(run, /^\. "\$here\/lib\.sh"/m);
  assert.match(readFileSync(join(root, 'deploy', 'tests', 'kind', 'lib.sh'), 'utf8'), /^P11_BUDGET_S=10$/m);
  assert.match(run, /p11_verdict "\$P11_BUDGET_S"/);
  assert.match(run, /NOT MEASURED/);
});

test('the A1 failure path is unchanged: the refusal is named, nothing is faked', () => {
  assert.match(run, /THE HUB REFUSES TO START, ON PURPOSE \(plan 09 A1\)/);
  assert.match(run, /fail_reason="A1: the hub binary exits 1 by design until it is composed \(03a and 05\); the replicas started and refused"/);
  assert.match(run, /\[ "\$refused" -eq 3 \]/);
});
