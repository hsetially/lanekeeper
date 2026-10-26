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

// ---- the registry stage (CI first run, 2026-10-26: `hub image` failed in 0 s with "dial tcp [::1]:32768: connect: connection refused") ----
//
// The first run showed that the registry answered on 127.0.0.1:32768 when it was started (the cluster stage passed through its readiness
// loop) and refused a connection on the same port 46 s later, when crane pushed to `localhost:32768`. Nothing in the log said what the
// container was doing at that moment. These tests drive the real run.sh and lib.sh with stand-ins for docker, kind, helm and crane (a
// cluster cannot start where they run), and a real HTTP server for the registry, so that what the script does when the registry stops
// answering is shown: it waits within a bound, says which route answered, reaches the registry by another route when one is gone, and
// prints the container's state and log when none answers.
import { spawn } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { after } from 'node:test';

const libSh = join(root, 'deploy', 'tests', 'kind', 'lib.sh');
const scratch = mkdtempSync(join(tmpdir(), 'kind-run-test-'));
after(() => rmSync(scratch, { recursive: true, force: true }));

/** Runs a command without blocking this process (a local HTTP server has to answer while it runs). */
function exec(cmd, args, { env = {}, cwd, input } = {}) {
  return new Promise((resolve) => {
    const child = spawn(cmd, args, { cwd, env: { ...process.env, ...env } });
    let out = '';
    let err = '';
    child.stdout.on('data', (d) => (out += d));
    child.stderr.on('data', (d) => (err += d));
    child.on('close', (code) => resolve({ code, out, err, all: `${out}${err}` }));
    child.stdin.end(input ?? '');
  });
}

/** A local HTTP server that answers 200 to everything, the way a registry answers /v2/. */
function listen() {
  return new Promise((resolve) => {
    const server = createServer((req, res) => res.end('{}'));
    server.listen(0, '127.0.0.1', () => resolve({ server, port: server.address().port }));
  });
}

/** A port on which nothing listens. */
async function closedPort() {
  const { server, port } = await listen();
  await new Promise((r) => server.close(r));
  return port;
}

/** Runs one lib.sh function: bash -c 'set -u; . lib.sh; BODY'. */
function lib(body, opts) {
  return exec('bash', ['-c', `set -u; . "${libSh}"; ${body}`], opts);
}

test('wait_for_http returns when the URL answers after a delay, and says which URL answered', async () => {
  const port = await closedPort();
  const pending = lib(`wait_for_http 10 "http://127.0.0.1:${port}/v2/"; code=$?; echo "answered=\${HTTP_ANSWERED:-none}"; exit $code`, { env: { LK_WAIT_STEP: '0.2' } });
  await new Promise((r) => setTimeout(r, 800));
  const server = createServer((req, res) => res.end('{}'));
  await new Promise((r) => server.listen(port, '127.0.0.1', r));
  const r = await pending;
  server.close();
  assert.equal(r.code, 0, r.all);
  assert.match(r.out, new RegExp(`answered=http://127\\.0\\.0\\.1:${port}/v2/`));
});

test('wait_for_http gives up within its bound and says what each URL said', async () => {
  const a = await closedPort();
  const b = await closedPort();
  const started = Date.now();
  const r = await lib(`wait_for_http 2 "http://127.0.0.1:${a}/v2/" "http://127.0.0.1:${b}/v2/"`, { env: { LK_WAIT_STEP: '0.2' } });
  assert.equal(r.code, 1, r.all);
  assert.ok(Date.now() - started < 15000, 'the wait is bounded');
  assert.match(r.err, new RegExp(`127\\.0\\.0\\.1:${a}/v2/ .*Failed to connect|127\\.0\\.0\\.1:${a}/v2/ .*\\(7\\)`));
  assert.match(r.err, new RegExp(`127\\.0\\.0\\.1:${b}/v2/`));
  assert.match(r.err, /nothing answered in 2 s/);
});

test('wait_for_http tries every URL, so a route that is dead does not hide one that works', async () => {
  const dead = await closedPort();
  const { server, port } = await listen();
  const r = await lib(`wait_for_http 5 "http://127.0.0.1:${dead}/v2/" "http://127.0.0.1:${port}/v2/"; code=$?; echo "answered=$HTTP_ANSWERED"; exit $code`, { env: { LK_WAIT_STEP: '0.2' } });
  server.close();
  assert.equal(r.code, 0, r.all);
  assert.match(r.out, new RegExp(`answered=http://127\\.0\\.0\\.1:${port}/v2/`));
});

test('wait_for_http refuses a budget that is not a whole number, and a call with no URL', async () => {
  assert.equal((await lib('wait_for_http abc http://127.0.0.1:1/')).code, 2);
  assert.equal((await lib('wait_for_http 5')).code, 2);
});

/** A stand-in `docker` that records its calls and answers what lib.sh and run.sh ask, from the environment. */
const DOCKER = `#!/usr/bin/env bash
echo "docker $*" >> "$SHIM_LOG"
case "$1" in
  info | rm | network) exit 0 ;;
  run)
    [ "\${SHIM_DOCKER_RUN_EXIT:-0}" = 0 ] || { echo "docker: Error response from daemon: boom" >&2; exit "$SHIM_DOCKER_RUN_EXIT"; }
    echo fakecontainerid ;;
  port) echo "127.0.0.1:$SHIM_REGISTRY_PORT" ;;
  inspect)
    case "$*" in
      *State.Status*) echo "status=\${SHIM_STATE:-running} running=$([ "\${SHIM_STATE:-running}" = running ] && echo true || echo false) exit_code=\${SHIM_EXIT_CODE:-0} oom_killed=false error=\\"\\" started=2026-10-26T17:29:12Z finished=0001-01-01T00:00:00Z" ;;
      *"range \\$n"*) echo "bridge 172.17.0.2"; echo "kind \${SHIM_REGISTRY_IP:-}" ;;
      *"index .NetworkSettings.Networks"*) echo "\${SHIM_REGISTRY_IP:-}" ;;
    esac ;;
  logs) echo "time=2026-10-26T17:29:12Z level=info msg=listening addr=:80"; echo "registry log, last line" ;;
  exec)
    shift
    while [ $# -gt 0 ] && [ "$1" = "-i" ]; do shift; done
    shift # the container
    case "$*" in
      *"get nodes --no-headers"*) printf 'n1 Ready\\nn2 Ready\\nn3 Ready\\nn4 Ready\\n' ;;
      *"get nodes"*) echo "NAME STATUS" ;;
      *"get pods"*) echo '{"items":[]}' ;;
      *" wait "*) [ "\${SHIM_WAIT_EXIT:-0}" = 0 ] || { echo "error: timed out waiting for the condition on nodes/lanekeeper-e2e-worker" >&2; exit "$SHIM_WAIT_EXIT"; } ;;
      *crictl*) [ "\${SHIM_CRICTL_EXIT:-0}" = 0 ] || { echo "FATA[0000] pulling image: failed to resolve reference" >&2; exit "$SHIM_CRICTL_EXIT"; } ;;
      sh\\ *) cat > /dev/null ;;
    esac ;;
  *) exit 0 ;;
esac
`;

/** A stand-in curl: the first SHIM_LOOPBACK_ANSWERS requests to the registry's loopback URL reach the real server, later ones reach a closed
 *  port (what the CI log showed: answered at the start, refused later); 10.9.8.7:80 is the registry's address on the kind network. */
const CURL = `#!/usr/bin/env bash
args=()
for a in "$@"; do
  case "$a" in
    "http://127.0.0.1:$SHIM_REGISTRY_PORT/"*)
      n=$(cat "$SHIM_DIR/loopback-count" 2>/dev/null || echo 0); echo $((n + 1)) > "$SHIM_DIR/loopback-count"
      [ "$n" -lt "\${SHIM_LOOPBACK_ANSWERS:-1000}" ] || a="http://127.0.0.1:$SHIM_CLOSED_PORT/\${a#*$SHIM_REGISTRY_PORT/}" ;;
    "http://10.9.8.7:80/"*) a="http://127.0.0.1:\${SHIM_ROUTE2_PORT:-$SHIM_CLOSED_PORT}/\${a#*:80/}" ;;
  esac
  args+=("$a")
done
exec "$REAL_CURL" "\${args[@]}"
`;

const CRANE = `#!/usr/bin/env bash
echo "crane $*" >> "$SHIM_LOG"
case "$1" in
  push) echo "\${3%:*}@$(cat "$SHIM_IMAGES/hub.digest")" ;;
  digest) cat "$SHIM_IMAGES/hub.digest" ;;
esac
`;

/** A copy of the scripts under test in a scratch repository (so that run.sh finds no real tools in target/ops-tools/bin), the stand-ins,
 *  and a tiny image archive with its recorded digest. */
async function scene(name, env = {}) {
  const dir = join(scratch, name);
  const repo = join(dir, 'repo');
  const bin = join(dir, 'bin');
  const images = join(dir, 'images');
  mkdirSync(join(repo, 'deploy', 'tests', 'kind'), { recursive: true });
  mkdirSync(join(repo, 'deploy', 'images'), { recursive: true });
  mkdirSync(bin);
  mkdirSync(images);
  for (const f of ['run.sh', 'lib.sh', 'pins.env']) copyFileSync(join(root, 'deploy', 'tests', 'kind', f), join(repo, 'deploy', 'tests', 'kind', f));
  for (const f of ['publish.sh', 'tools.env']) copyFileSync(join(root, 'deploy', 'images', f), join(repo, 'deploy', 'images', f));
  chmodSync(join(repo, 'deploy', 'tests', 'kind', 'run.sh'), 0o755);
  chmodSync(join(repo, 'deploy', 'images', 'publish.sh'), 0o755);
  assert.equal((await exec('git', ['init', '-q'], { cwd: repo })).code, 0);
  assert.equal((await exec('python3', ['-I', join(root, 'deploy', 'images', 'tests', 'tiny-oci.py'), join(images, 'hub.oci.tar')])).code, 0);
  const index = JSON.parse((await exec('tar', ['-xOf', join(images, 'hub.oci.tar'), 'index.json'])).out);
  writeFileSync(join(images, 'hub.digest'), `${index.manifests[0].digest}\n`);
  const which = (await exec('bash', ['-c', 'command -v curl'])).out.trim();
  for (const [file, body] of [['docker', DOCKER], ['curl', CURL], ['crane', CRANE],
    ['kind', '#!/usr/bin/env bash\ncase "$1" in version) echo "kind v0.33.0 go1.25 linux/amd64" ;; get) printf "lanekeeper-e2e-control-plane\\nlanekeeper-e2e-worker\\nlanekeeper-e2e-worker2\\nlanekeeper-e2e-worker3\\n" ;; esac\n'],
    ['helm', '#!/usr/bin/env bash\ncase "$1" in version) echo "v4.3.0+gbec5b06" ;; esac\n']]) {
    writeFileSync(join(bin, file), body);
    chmodSync(join(bin, file), 0o755);
  }
  writeFileSync(join(dir, 'kind-config.yaml'), 'kind: Cluster\n');
  const { server, port } = await listen();
  const closed = await closedPort();
  const log = join(dir, 'calls.log');
  writeFileSync(log, '');
  const shimEnv = {
    PATH: `${bin}:${process.env.PATH}`, SHIM_LOG: log, SHIM_DIR: dir, SHIM_IMAGES: images, REAL_CURL: which,
    SHIM_REGISTRY_PORT: String(port), SHIM_CLOSED_PORT: String(closed), LK_WAIT_STEP: '0.2', LK_KIND_REGISTRY_WAIT: '3',
    LK_KIND_CONFIG_FILE: join(dir, 'kind-config.yaml'), ...env,
  };
  const runSh = (...args) => exec(join(repo, 'deploy', 'tests', 'kind', 'run.sh'), [...args, '--images-dir', images, '--out', join(dir, 'out')], { env: shimEnv });
  return { dir, port, closed, log, server, shimEnv, runSh, calls: () => (existsSync(log) ? exec('cat', [log]).then((r) => r.out) : '') };
}

test('registry_report prints the state of the container, its exit code, its address and the end of its log', async () => {
  const s = await scene('report', { SHIM_STATE: 'exited', SHIM_EXIT_CODE: '137', SHIM_REGISTRY_IP: '172.18.0.6' });
  const r = await lib('registry_report kind-registry.test 32768', { env: s.shimEnv });
  s.server.close();
  assert.equal(r.code, 0, r.all);
  assert.match(r.err, /the registry container kind-registry\.test/);
  assert.match(r.err, /status=exited .*exit_code=137/);
  assert.match(r.err, /kind 172\.18\.0\.6/);
  assert.match(r.err, /registry log, last line/);
});

test('the registry stage: answers at the start, so the push goes to 127.0.0.1 (never localhost) and the registry stage passes', async () => {
  const s = await scene('loopback');
  const r = await s.runSh('--registry-only');
  s.server.close();
  assert.equal(r.code, 0, r.all);
  assert.match(r.out, new RegExp(`the local registry answers on 127\\.0\\.0\\.1:${s.port}`));
  const calls = await s.calls();
  assert.match(calls, new RegExp(`crane push \\S+ 127\\.0\\.0\\.1:${s.port}/lanekeeper/hub:kind`));
  assert.doesNotMatch(calls, /localhost/);
  assert.match(r.out, /\| registry \| PASS \|/);
  assert.match(r.out, /\| hub image \| PASS \|/);
});

test('the registry stops answering on its published port after it started: the push goes to its address on the kind network, and says so', async () => {
  const route2 = await listen();
  const s = await scene('route2', { SHIM_LOOPBACK_ANSWERS: '1', SHIM_REGISTRY_IP: '10.9.8.7', SHIM_ROUTE2_PORT: String(route2.port) });
  const r = await s.runSh('--registry-only');
  s.server.close();
  route2.server.close();
  assert.equal(r.code, 0, r.all);
  assert.match(r.out, new RegExp(`NOTE: the published port 127\\.0\\.0\\.1:${s.port} does not answer any more; the registry answers at its address on the kind network, 10\\.9\\.8\\.7:80`));
  assert.match(await s.calls(), /crane push \S+ 10\.9\.8\.7:80\/lanekeeper\/hub:kind/);
});

test('no route answers: the stage fails, names the routes, and prints the container state and log', async () => {
  const s = await scene('noroute', { SHIM_LOOPBACK_ANSWERS: '1', SHIM_REGISTRY_IP: '10.9.8.7', SHIM_STATE: 'exited', SHIM_EXIT_CODE: '137' });
  const r = await s.runSh('--registry-only');
  s.server.close();
  assert.notEqual(r.code, 0, r.all);
  assert.match(r.all, /FAILED in the stage 'hub image'/);
  assert.match(r.all, new RegExp(`127\\.0\\.0\\.1:${s.port}/v2/ `));
  assert.match(r.all, /10\.9\.8\.7:80\/v2\/ /);
  assert.match(r.all, /nothing answered in 3 s/);
  assert.match(r.all, /status=exited .*exit_code=137/);
  assert.match(r.all, /registry log, last line/);
  assert.doesNotMatch(await s.calls(), /crane push/, 'nothing is pushed to a registry that does not answer');
});

test('a command that fails with no reason of its own is named with its exit code, and the registry state is printed', async () => {
  const s = await scene('dockerrun', { SHIM_DOCKER_RUN_EXIT: '125' });
  const r = await s.runSh('--registry-only');
  s.server.close();
  assert.notEqual(r.code, 0, r.all);
  assert.match(r.all, /FAILED in the stage 'registry': .*exit 125/);
  assert.match(r.all, /at run\.sh:\d+, and nothing handled it: docker run -d --name "\$REGISTRY"/);
  assert.match(r.all, /\| registry \| FAIL \|/);
});

test('the four nodes must be Ready before the cluster stage passes, and the wait comes after the registry joined the kind network', async () => {
  const s = await scene('nodes-wait', { SHIM_WAIT_EXIT: '1' });
  const r = await s.runSh();
  s.server.close();
  assert.notEqual(r.code, 0, r.all);
  assert.match(r.all, /FAILED in the stage 'cluster': .*nodes were Ready/);
  const calls = await s.calls();
  assert.ok(calls.indexOf('docker network connect') < calls.indexOf('wait --for=condition=Ready node'), calls);
  assert.doesNotMatch(calls, /crane push/, 'no image is pushed while the cluster stage has failed');
});

test('with Ready nodes the run goes on to the hub image stage, which pushes, then asks a node to pull by digest', async () => {
  const s = await scene('nodes-ready', { SHIM_CRICTL_EXIT: '1' });
  const r = await s.runSh();
  s.server.close();
  assert.notEqual(r.code, 0, r.all);
  assert.match(r.all, /\| cluster \| PASS \|/);
  assert.match(r.all, /FAILED in the stage 'hub image': a node cannot pull kind-registry\.test\/lanekeeper\/hub@sha256:[0-9a-f]{64}/);
  const calls = await s.calls();
  const at = (needle) => calls.indexOf(needle);
  assert.ok(at('wait --for=condition=Ready node') > at('network connect') && at('crane push') > at('wait --for=condition=Ready node') && at('crictl pull') > at('crane push'), calls);
});
