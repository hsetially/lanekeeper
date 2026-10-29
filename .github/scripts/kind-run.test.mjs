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
const libSh0 = join(root, 'deploy', 'tests', 'kind', 'lib.sh');

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

// The production hub cannot become Ready in kind (it needs real Google and Entra, by design: prompt 18, G23), so what this job proves is not
// what its first version meant to prove. The job says so in one sentence, kept in lib.sh and printed by run.sh and by the workflow.
test('lib.sh holds the one sentence that says what the kind job does not measure, and run.sh and images.yml print that one', () => {
  const lib = readFileSync(libSh0, 'utf8');
  const sentence = /^KIND_NOT_MEASURED='([^']+)'$/m.exec(lib)?.[1];
  assert.ok(sentence, 'lib.sh has no KIND_NOT_MEASURED');
  for (const part of ['/readyz on 3 replicas', 'P11 on the pod', 'NOT MEASURED', 'cannot be Ready without Google and Entra', 'acceptance 3 is not met by this job']) {
    assert.ok(sentence.includes(part), `the sentence lacks "${part}": ${sentence}`);
  }
  assert.match(run, /\$KIND_NOT_MEASURED/, 'run.sh does not print it');
  const workflow = readFileSync(join(root, '.github', 'workflows', 'images.yml'), 'utf8');
  const job = /^ {2}kind-e2e:\n([\s\S]*?)(?=^ {2}[a-z][a-z0-9-]*:\n)/m.exec(workflow)?.[1] ?? '';
  assert.ok(job, 'images.yml has no kind-e2e job');
  assert.match(job, /- name: [^\n]+\n\s+if: always\(\)\n\s+run: \|\n\s+\. deploy\/tests\/kind\/lib\.sh\n\s+printf '%s\\n' "\$KIND_NOT_MEASURED" >> "\$GITHUB_STEP_SUMMARY"/,
    'the kind-e2e job has no step that always appends the sentence to the job summary');
});

test('nothing in the kind test, its README, the workflow or the Justfile still describes the refusal of the old stub (plan 09 A1)', () => {
  const texts = {
    'run.sh': run,
    'lib.sh': readFileSync(libSh0, 'utf8'),
    'README.md': readFileSync(join(root, 'deploy', 'tests', 'kind', 'README.md'), 'utf8'),
    'images.yml': readFileSync(join(root, '.github', 'workflows', 'images.yml'), 'utf8'),
    Justfile: readFileSync(join(root, 'Justfile'), 'utf8'),
    'charts/hub/README.md': readFileSync(join(root, 'deploy', 'charts', 'hub', 'README.md'), 'utf8'),
    'charts/gcp/README.md': readFileSync(join(root, 'deploy', 'charts', 'gcp', 'README.md'), 'utf8'),
    'ADDING-A-SWIMLANE.md': readFileSync(join(root, 'deploy', 'ADDING-A-SWIMLANE.md'), 'utf8'),
  };
  for (const [name, text] of Object.entries(texts)) {
    assert.doesNotMatch(text, /HUB_REFUSAL_TEXT|no identity module|THE HUB REFUSES TO START|plan 09 A1\b|\(03a and 05\)|until it is composed|a1-partial/, `${name} still describes the old refusal`);
  }
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
import { createHash } from 'node:crypto';
import { appendFileSync, chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
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
    child.stdin.on('error', () => {}); // a child that has already exited does not read its standard input
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
      *"exec -i lanekeeper-db-1"*) # psql in the database pod, as run.sh asks for it
        cat > /dev/null
        case "$*" in
          *sslmode=disable*)
            case "\${SHIM_S6_DISABLE:-reject}" in
              connect) echo 1; exit 0 ;;
              wrongpw) echo 'psql: error: connection to server at "lanekeeper-db-rw.lanekeeper.svc" (10.96.150.117), port 5432 failed: FATAL:  password authentication failed for user "lanekeeper_app"' >&2; exit 2 ;;
              *) echo 'psql: error: connection to server at "lanekeeper-db-rw.lanekeeper.svc" (10.96.150.117), port 5432 failed: FATAL:  pg_hba.conf rejects connection for host "10.244.2.1", user "lanekeeper_app", database "lanekeeper", no encryption' >&2; exit 2 ;;
            esac ;;
          *ssl_max_protocol_version*) echo 'psql: error: connection to server at "lanekeeper-db-rw.lanekeeper.svc" (10.96.150.117), port 5432 failed: SSL error: tlsv1 alert protocol version' >&2; exit 2 ;;
          *pg_stat_ssl*) echo TLSv1.3 ;;
          *pg_extension*) echo pg_trgm,vector ;;
          *"vector(3)"*) echo '[1,2,3]' ;;
          *datcollate*) echo en_US.utf8 ;;
        esac ;;
      *"get nodes --no-headers"*) printf 'n1 Ready\\nn2 Ready\\nn3 Ready\\nn4 Ready\\n' ;;
      *"get nodes"*) echo "NAME STATUS" ;;
      *"get pods -l app.kubernetes.io/name=lanekeeper-hub -o json"*) cat "$SHIM_DIR/pods.json" ;;
      *"get pods -l"*) echo "NAME READY STATUS RESTARTS" ;;
      *"get pods"*) echo '{"items":[]}' ;;
      *"get jobs -l"*) printf '%s' "\${SHIM_SEED_JOBS-lanekeeper-hub-seed-4f2a9c1b7e}" ;;
      *"get job lanekeeper-hub-"*) printf '%s' "\${SHIM_JOB_SUCCEEDED-1}" ;;
      *" logs -l job-name="*) echo '{"level":"INFO","fields":{"message":"the schema is up to date","applied":8,"version":8},"target":"hub::migrate"}' ;;
      *"get pod lanekeeper-hub-"*) n="$(sed -n 's/.*get pod lanekeeper-hub-\\([0-9]\\).*/\\1/p' <<<"$*")"; cat "$SHIM_DIR/pod-$n.json" ;;
      *" logs lanekeeper-hub-"*)
        pod="$(sed -n 's/.* logs \\(lanekeeper-hub-[0-9]\\).*/\\1/p' <<<"$*")"
        n="$(cat "$SHIM_DIR/logcalls-$pod" 2>/dev/null || echo 0)"; echo $((n + 1)) > "$SHIM_DIR/logcalls-$pod"
        var="SHIM_LOGS_\${pod##*-}"; mode="\${!var:-ok}"
        unavailable() { # what the first runner showed: exit 0 and a sentence in place of a log, for a container that the kubelet had removed
          case "$*" in *--previous*) echo "unable to retrieve container logs for containerd://f0ccf06c64bd0f461849de9c1599469ae5bd5b93d369e9a69c2a8134efce14a5"; exit 0 ;; esac
          echo "Error from server (BadRequest): container \\"hub\\" in pod \\"$pod\\" is waiting to start: CrashLoopBackOff" >&2; exit 1
        }
        case "$mode" in
          ok) cat "$SHIM_DIR/log-\${pod##*-}.txt" ;;
          retry:*) if [ "$n" -lt "\${mode#retry:}" ]; then unavailable "$@"; else cat "$SHIM_DIR/log-\${pod##*-}.txt"; fi ;;
          never) unavailable "$@" ;;
        esac ;;
      *"wait --for=condition=complete job/"*)
        job="$(sed -n 's/.*job\\/\\([^ ]*\\).*/\\1/p' <<<"$*")"
        case "$job" in lanekeeper-hub-migrate) var=SHIM_JOB_MIGRATE ;; *) var=SHIM_JOB_SEED ;; esac
        [ "\${!var:-complete}" = complete ] || { echo "error: timed out waiting for the condition on jobs.batch/$job" >&2; exit 1; } ;;
      *"wait --for=condition=Ready node"*) [ "\${SHIM_WAIT_EXIT:-0}" = 0 ] || { echo "error: timed out waiting for the condition on nodes/lanekeeper-e2e-worker" >&2; exit "$SHIM_WAIT_EXIT"; } ;;
      *" apply "*) cat > /dev/null ;;
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
const crashed = (restarts = 2, exitCode = 1) => ({ name: 'hub', ready: false, restartCount: restarts,
  state: { waiting: { reason: 'CrashLoopBackOff', message: 'back-off 10s restarting failed container=hub pod=lanekeeper-hub-0' } },
  lastState: { terminated: { containerID: 'containerd://2679bc8a', exitCode, reason: 'Error', startedAt: '2026-10-28T10:00:00Z', finishedAt: '2026-10-28T10:00:09Z' } } });
const imagePull = () => ({ name: 'hub', ready: false, restartCount: 0, state: { waiting: { reason: 'ImagePullBackOff', message: 'Back-off pulling image' } } });
/** A replica that is Ready (the production image cannot be, in kind): started at 10:00:00, Ready after `seconds`. */
const readyPod = (seconds) => ({ name: 'hub', ready: true, restartCount: 0, state: { running: { startedAt: '2026-10-28T10:00:00Z' } }, readyAt: `2026-10-28T10:00:${String(seconds).padStart(2, '0')}Z` });

// ---- what a hub that cannot reach Google logs (one JSON object per line, as crates/hub/src/telemetry.rs writes them) ----
// Fixtures copied from the Display of StartError, PlatformError, CaStep and WebTlsError in crates/hub/src (process.rs, wiring/mod.rs,
// wiring/grpc.rs, web_tls.rs); the literals themselves are checked against the source below.
const jsonLine = (level, message, extra = {}) => JSON.stringify({ timestamp: '2026-10-28T10:00:08.000000Z', level, fields: { message, ...extra }, target: 'hub::process' });
const WEB_CERT = 'the web certificate could not be loaded (LK_WEB_CERT_REF, LK_WEB_KEY_REF): the certificate secret could not be read';
const WEB_TIMEOUT = 'starting took too long: web certificate';
const CA_AGENT_SECRET = 'the agent CA is not usable (LK_AGENT_CA_KMS_REF, LK_AGENT_CA_CERT_REF): the CA certificate secret could not be read: secret manager is unavailable';
const CA_REPLICA_TIMEOUT = 'the replica CA is not usable (LK_REPLICA_CA_KMS_REF, LK_REPLICA_CA_CERT_REF): setting it up took too long';
const DB_ERROR = 'the database is not usable: could not connect to the database';
const DB_TIMEOUT = 'starting took too long: database';
const SCHEMA_ERROR = 'the database schema is not usable: migration 0001 (init) has not been applied; run `hub migrate` with the migrator credential first';
/** The log of a replica that stopped at its first Google step: the CA errors that load() logs beside it, and the error that main logs last. */
const googleLog = (last = WEB_CERT) => [
  jsonLine('INFO', 'starting', { version: '0.1.0' }),
  jsonLine('ERROR', 'the hub could not start', { ca: 'replica', error: CA_REPLICA_TIMEOUT }),
  jsonLine('ERROR', 'the hub could not start', { error: CA_AGENT_SECRET }),
  jsonLine('ERROR', 'the hub could not start', { error: last }),
].join('\n');
/** The same, with one more error that is not Google's: the log of a replica whose database or schema check failed too. */
const withError = (error, last = WEB_CERT) => [jsonLine('INFO', 'starting', { version: '0.1.0' }), jsonLine('ERROR', 'the hub could not start', { error }), jsonLine('ERROR', 'the hub could not start', { error: last })].join('\n');
const SETTING_LOG = 'hub: LK_WEB_CERT_REF is required';
const GOOGLE_LOGS = [googleLog(), googleLog(WEB_TIMEOUT), googleLog()];

async function scene(name, env = {}, { pods = [crashed(1), crashed(2), crashed(1)], logs = GOOGLE_LOGS } = {}) {
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
    ['helm', `#!/usr/bin/env bash
echo "helm $*" >> "$SHIM_LOG"
case "$1" in
  version) echo "v4.3.0+gbec5b06" ;;
  upgrade) case "$*" in *"upgrade --install lanekeeper-hub "*) [ "\${SHIM_HELM_HUB_EXIT:-0}" = 0 ] || { echo "Error: failed pre-install: 1 error occurred: * job lanekeeper-hub-migrate failed: BackoffLimitExceeded" >&2; exit "$SHIM_HELM_HUB_EXIT"; } ;; esac ;;
esac
`]]) {
    writeFileSync(join(bin, file), body);
    chmodSync(join(bin, file), 0o755);
  }
  writeFileSync(join(dir, 'kind-config.yaml'), 'kind: Cluster\n');
  // The operator's manifest is local, with the pinned image named twice as the release's file names it, so that no stage needs the network.
  const pinsFile = join(repo, 'deploy', 'tests', 'kind', 'pins.env');
  const base = /^LK_CNPG_OPERATOR_IMAGE=(\S+?)@sha256:/m.exec(readFileSync(pinsFile, 'utf8'))[1];
  const manifest = `        - name: OPERATOR_IMAGE_NAME\n          value: ${base}\n        image: ${base}\n`;
  writeFileSync(join(dir, 'cnpg.yaml'), manifest);
  appendFileSync(pinsFile, `LK_CNPG_MANIFEST_URL=file://${join(dir, 'cnpg.yaml')}\nLK_CNPG_MANIFEST_SHA256=${createHash('sha256').update(manifest).digest('hex')}\n`);
  // The hub pods as the API server shows them: one JSON document of the list, and one per pod.
  const condition = ({ ready, readyAt }) => ({ type: 'Ready', status: ready ? 'True' : 'False', ...(readyAt ? { lastTransitionTime: readyAt } : {}) });
  writeFileSync(join(dir, 'pods.json'), JSON.stringify({ items: pods.map((status, i) => ({ metadata: { name: `lanekeeper-hub-${i}` }, status: { conditions: [condition(status)], containerStatuses: [status] } })) }));
  pods.forEach((status, i) => writeFileSync(join(dir, `pod-${i}.json`), JSON.stringify({ status: { conditions: [condition(status)], containerStatuses: [status] } })));
  logs.forEach((text, i) => writeFileSync(join(dir, `log-${i}.txt`), `${text}\n`));
  const { server, port } = await listen();
  const closed = await closedPort();
  const log = join(dir, 'calls.log');
  writeFileSync(log, '');
  const shimEnv = {
    PATH: `${bin}:${process.env.PATH}`, SHIM_LOG: log, SHIM_DIR: dir, SHIM_IMAGES: images, REAL_CURL: which,
    SHIM_REGISTRY_PORT: String(port), SHIM_CLOSED_PORT: String(closed), LK_WAIT_STEP: '0.2', LK_KIND_REGISTRY_WAIT: '3', LK_KIND_LOG_WAIT: '2', LK_KIND_LOG_STEP: '0.2', LK_KIND_POLL_STEP: '0.2', LK_KIND_HUB_TIMEOUT: '3', LK_KIND_JOBS_TIMEOUT: '30',
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

// ---- the hub stage: what the PRODUCTION image can do in kind (decision of 2026-10-28) ----
//
// The production hub cannot become Ready in kind: it needs real Google (Secret Manager, KMS) and Entra, by design (prompt 18, G23), and kind
// has neither. So the stage passes when (1) the migration Job completes against CloudNativePG, (2) the seed Job completes, and (3) each
// replica reaches its own refusal at the first Google step: exit code 1, the hub's typed start error for the web certificate or a CA (or the
// start timeout of that step), and no setting error, no database error and no schema error in its log (the database and schema checks run
// in parallel with the Google reads, crates/hub/src/process.rs `load`, and every failure is logged, so a log with only a Google error shows
// that they passed). Every other state is a failure that names what was seen. These scenes give the real run.sh a kubectl that answers as
// each state would; the stage itself has not run in this sandbox (no Docker daemon, cgroup v1).
const NOT_MEASURED = /\/readyz on 3 replicas and P11 on the pod are NOT MEASURED in kind \(the production image cannot be Ready without Google and Entra\), acceptance 3 is not met by this job/;

/** The string literals of the non-test code of a Rust source: comments are left out, and so is everything from the first #[cfg(test)] on. */
function rustLiterals(text) {
  const end = text.indexOf('#[cfg(test)]');
  const src = end >= 0 ? text.slice(0, end) : text;
  const found = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const raw = c === 'r' && !/[A-Za-z0-9_]/.test(src[i - 1] ?? ' ') ? /^r(#*)"/.exec(src.slice(i, i + 12)) : null;
    if (c === '/' && src[i + 1] === '/') {
      while (i < src.length && src[i] !== '\n') i += 1;
    } else if (c === '/' && src[i + 1] === '*') {
      const e = src.indexOf('*/', i + 2);
      i = e < 0 ? src.length : e + 2;
    } else if (raw) {
      const close = `"${raw[1]}`;
      const from = i + raw[0].length;
      const e = src.indexOf(close, from);
      found.push(src.slice(from, e < 0 ? src.length : e));
      i = e < 0 ? src.length : e + close.length;
    } else if (c === '"') {
      let lit = '';
      i += 1;
      while (i < src.length && src[i] !== '"') {
        if (src[i] === '\\') {
          if (src[i + 1] === '\n') {
            i += 2;
            while (/[ \t]/.test(src[i] ?? '')) i += 1;
            continue;
          }
          lit += src[i + 1];
          i += 2;
          continue;
        }
        lit += src[i];
        i += 1;
      }
      found.push(lit);
      i += 1;
    } else if (c === "'") {
      // a character literal ('"', '\n', 'a') is skipped whole; a lifetime ('a) is just the quote
      if (src[i + 1] === '\\') i = src.indexOf("'", i + 3) + 1 || src.length;
      else if (src[i + 2] === "'") i += 3;
      else i += 1;
    } else {
      i += 1;
    }
  }
  return found;
}

function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(join(dir, e.name)) : e.name.endsWith('.rs') ? [join(dir, e.name)] : []));
}

test('rustLiterals reads string literals of the code and not comments, test modules or other text (so the coupling check below can fail)', () => {
  const lits = rustLiterals([
    '// "in a line comment"',
    '/* "in a block comment" */',
    '#[error("a typed error: {0}")]',
    'let r = r#"raw "quoted" text"#;',
    "let q = '\"'; let s = \"after a quote char\";",
    'let c = "continued \\',
    '           on the next line";',
    "fn f<'a>(x: &'a str) -> &'a str { \"after a lifetime\" }",
    '#[cfg(test)]',
    'mod tests { const T: &str = "only in a test"; }',
  ].join('\n'));
  assert.deepEqual(lits, ['a typed error: {0}', 'raw "quoted" text', 'after a quote char', 'continued on the next line', 'after a lifetime']);
});

// The ONE place that couples the script to prompt 18's wording is the block of lib.sh that holds the markers. This test is the replacement for
// the check of the old refusal text against main.rs: it fails when a string that the script looks for no longer exists in the Rust source.
const ANCHORS = {
  // StartError::WebCertificate: #[error("the web certificate could not be loaded (LK_WEB_CERT_REF, LK_WEB_KEY_REF): {0}")]
  'the web certificate could not be loaded (LK_WEB_CERT_REF, LK_WEB_KEY_REF)': [['has', 'the web certificate could not be loaded (LK_WEB_CERT_REF, LK_WEB_KEY_REF)']],
  // StartError::TimedOut: #[error("starting took too long: {0}")], and load() bounds the web certificate with the label "web certificate"
  'starting took too long: web certificate': [['has', 'starting took too long: {0}'], ['is', 'web certificate']],
  // CaStep::Secret and CaStep::TimedOut (wiring/grpc.rs), shown inside PlatformError::Ca: "the {} CA is not usable ({}): {step}"
  'the CA certificate secret could not be read': [['has', 'the CA certificate secret could not be read: {0}'], ['has', ' CA is not usable (']],
  'setting it up took too long': [['is', 'setting it up took too long'], ['has', ' CA is not usable (']],
  // StartError::Database, StartError::TimedOut("database") (the connection and the schema check are one bounded step)
  'the database is not usable': [['has', 'the database is not usable: {0}']],
  'starting took too long: database': [['has', 'starting took too long: {0}'], ['is', 'database']],
  // StartError::Schema
  'the database schema is not usable': [['has', 'the database schema is not usable: {0}']],
  // process.rs `say`: eprintln!("hub: {message}"), for a refused setting, the command line, telemetry, and the process before the start
  'hub: ': [['has', 'hub: {message}']],
};

test('every string that run.sh looks for in a hub log is a literal of the Rust source of the hub (outside its tests), and every one has an anchor here', async () => {
  const r = await lib('hub_markers');
  assert.equal(r.code, 0, r.all);
  const markers = r.out.split('\n').filter((l) => l !== '');
  assert.ok(markers.length >= 8, `lib.sh lists only ${markers.length} markers: ${markers}`);
  assert.deepEqual([...markers].sort(), Object.keys(ANCHORS).sort(), 'the markers of lib.sh and the anchors of this test differ');
  const files = walk(join(root, 'crates', 'hub', 'src'));
  assert.ok(files.length > 20, `only ${files.length} Rust files were found`);
  const literals = files.flatMap((f) => rustLiterals(readFileSync(f, 'utf8')));
  for (const [marker, anchors] of Object.entries(ANCHORS)) {
    for (const [how, text] of anchors) {
      const ok = how === 'is' ? literals.includes(text) : literals.some((l) => l.includes(text));
      assert.ok(ok, `"${marker}": no ${how === 'is' ? 'string literal equal to' : 'string literal that holds'} "${text}" in crates/hub/src`);
    }
  }
});

test('the check can fail: the refusal text of the old stub is in no literal of the source, and a template that the markers rely on is', () => {
  const literals = walk(join(root, 'crates', 'hub', 'src')).flatMap((f) => rustLiterals(readFileSync(f, 'utf8')));
  assert.ok(!literals.some((l) => l.includes('no identity module')), 'the old refusal is gone from the source: a script that looked for it would fail');
  assert.ok(literals.some((l) => l.includes('starting took too long: {0}')));
});

/** Runs the real run.sh in a scene and returns the result with the calls that the stand-ins recorded. */
async function runScene(name, env, options) {
  const s = await scene(name, env, options);
  const r = await s.runSh();
  s.server.close();
  return { ...r, calls: await s.calls() };
}

test('hub: the Jobs complete and every replica stops at its first Google step: the stage passes, and says what it did not measure', async () => {
  const r = await runScene('hub-pass', {});
  assert.equal(r.code, 0, r.all);
  assert.match(r.all, /\| s6 \| PASS \|/);
  assert.match(r.all, /\| hub \| PASS \| \d+ \| [^|\n]*3 replicas[^|\n]*Google/);
  assert.match(r.all, NOT_MEASURED);
  assert.match(r.all, /\| \/readyz on 3 replicas \| NOT MEASURED \|/);
  assert.match(r.all, /\| P11 hub start-up \(10 s per replica\) \| NOT MEASURED \|/);
  assert.match(r.all, /kind-e2e: PASS/);
  assert.doesNotMatch(r.all, /FAILED in the stage|A1/);
  assert.match(r.all, /lanekeeper-hub-1: google/);
});

test('hub: the migration Job is waited for after helm and before the seed Job, the seed Job before the replicas are looked at, every wait bounded', async () => {
  const r = await runScene('hub-order', {});
  assert.equal(r.code, 0, r.all);
  const at = (needle) => r.calls.indexOf(needle);
  const helm = at('helm upgrade --install lanekeeper-hub ');
  const migrate = at('wait --for=condition=complete job/lanekeeper-hub-migrate ');
  const seed = at('wait --for=condition=complete job/lanekeeper-hub-seed-4f2a9c1b7e ');
  const logs = at('logs lanekeeper-hub-0');
  assert.ok(helm > 0 && migrate > helm && seed > migrate && logs > seed, r.calls);
  for (const line of r.calls.split('\n').filter((l) => l.includes('wait --for=condition=complete'))) assert.match(line, / --timeout=\d+s$/, line);
  assert.match(r.calls, /helm upgrade --install lanekeeper-hub .* --timeout \d+s/, 'helm waits for the pre-install hook (the migration Job), and is bounded');
  assert.match(r.all, /the migration Job \(lanekeeper-hub-migrate\) completed/);
  assert.match(r.all, /the seed Job \(lanekeeper-hub-seed-4f2a9c1b7e\) completed/);
});

test('hub: a replica that stopped at the CA, or at the start timeout, is a Google refusal too', async () => {
  const r = await runScene('hub-ca', {}, { logs: [withError(CA_AGENT_SECRET, CA_REPLICA_TIMEOUT), googleLog(WEB_TIMEOUT), googleLog()] });
  assert.equal(r.code, 0, r.all);
  assert.match(r.all, /lanekeeper-hub-0: google/);
  assert.match(r.all, /\| hub \| PASS \|/);
});

test('hub: logs that cannot be fetched at first, and then can, still count', async () => {
  const r = await runScene('hub-retry', { SHIM_LOGS_1: 'retry:3' });
  assert.equal(r.code, 0, r.all);
  assert.match(r.all, /lanekeeper-hub-1: google/);
});

test('hub: a setting error (exit 2, a line "hub: ...") fails the stage, names the replica and quotes the line', async () => {
  const r = await runScene('hub-setting', {}, { pods: [crashed(1), crashed(1, 2), crashed(1)], logs: [googleLog(), SETTING_LOG, googleLog()] });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': .*setting.*lanekeeper-hub-1.*hub: LK_WEB_CERT_REF is required/);
  assert.match(r.all, /\| hub \| FAIL \|/);
  assert.doesNotMatch(r.all, /\| hub \| PASS \|/);
  assert.match(r.all, NOT_MEASURED);
});

test('hub: exit code 2 is a setting error even when the log cannot be had', async () => {
  const r = await runScene('hub-setting-nolog', { SHIM_LOGS_0: 'never' }, { pods: [crashed(1, 2), crashed(1), crashed(1)] });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': .*setting.*lanekeeper-hub-0.*exit code 2/);
});

test('hub: a database error fails the stage, also when the Google error is in the same log', async () => {
  for (const [name, error] of [['hub-db', DB_ERROR], ['hub-db-timeout', DB_TIMEOUT]]) {
    const r = await runScene(name, {}, { logs: [googleLog(), googleLog(), withError(error)] });
    assert.equal(r.code, 1, r.all);
    assert.match(r.all, new RegExp(`FAILED in the stage 'hub': .*database.*lanekeeper-hub-2.*${error.replace(/[().]/g, '\\$&')}`), name);
    assert.doesNotMatch(r.all, /\| hub \| PASS \|/);
  }
});

test('hub: a schema error fails the stage: the migration did not do what the hub expects', async () => {
  const r = await runScene('hub-schema', {}, { logs: [withError(SCHEMA_ERROR), googleLog(), googleLog()] });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': .*schema check.*lanekeeper-hub-0.*although the migration Job completed.*migration 0001 \(init\) has not been applied/);
});

test('hub: a replica that is Ready is an unexpected state and fails the stage; the start-up times are printed for whoever looks', async () => {
  const r = await runScene('hub-ready', {}, { pods: [readyPod(4), readyPod(6), readyPod(12)] });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': UNEXPECTED: .*Ready/);
  assert.match(r.all, /lanekeeper-hub-2: 12 s from the start of the container to Ready, OVER the budget of 10 s \(P11\)/);
  assert.doesNotMatch(r.all, NOT_MEASURED, 'P11 was measured here, and /readyz answered: nothing is said to be not measured');
  assert.doesNotMatch(r.all, /\| hub \| PASS \|/);
});

test('hub: one replica Ready among replicas that stopped at Google is also unexpected', async () => {
  const r = await runScene('hub-ready-one', {}, { pods: [crashed(1), readyPod(5), crashed(1)] });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': UNEXPECTED: lanekeeper-hub-1 is Ready/);
});

test('hub: a log that no known error explains fails the stage and prints the log', async () => {
  const odd = jsonLine('ERROR', 'the hub could not start', { error: 'the web app cannot be served from LK_WEB_DIST: no index.html' });
  const r = await runScene('hub-unknown', {}, { logs: [googleLog(), odd, googleLog()] });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': .*lanekeeper-hub-1.*no error that this job knows/);
  assert.match(r.all, /the web app cannot be served from LK_WEB_DIST: no index\.html/, 'the log is printed');
  assert.doesNotMatch(r.all, /\| hub \| PASS \|/);
});

test('hub: a replica without a log fails the stage: nothing is asserted without a log', async () => {
  const r = await runScene('hub-nolog', { SHIM_LOGS_1: 'never' });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': no log could be retrieved for lanekeeper-hub-1 in 2 s/);
  assert.match(r.all, /unable to retrieve container logs|no log could be retrieved/);
  assert.doesNotMatch(r.all, /\| hub \| PASS \|/);
});

test('hub: a Google refusal with an exit code other than 1 fails the stage', async () => {
  const r = await runScene('hub-exit137', {}, { pods: [crashed(2, 137), crashed(1), crashed(1)] });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': .*lanekeeper-hub-0.*exit code 137.*expected 1/);
});

test('hub: replicas that never end or become Ready are judged after LK_KIND_HUB_TIMEOUT, not waited for', async () => {
  const started = Date.now();
  const r = await runScene('hub-stuck', { SHIM_LOGS_0: 'never', SHIM_LOGS_1: 'never', SHIM_LOGS_2: 'never' }, { pods: [imagePull(), imagePull(), imagePull()] });
  assert.equal(r.code, 1, r.all);
  assert.ok(Date.now() - started < 60000, 'the wait is bounded');
  assert.match(r.all, /FAILED in the stage 'hub': no log could be retrieved for lanekeeper-hub-0/);
  assert.match(r.all, /ImagePullBackOff/, 'the state of the replica is printed');
});

test('hub: a hub chart that does not install fails the stage with the migration Job\'s state and log, and the replicas are not looked at', async () => {
  const r = await runScene('hub-helm', { SHIM_HELM_HUB_EXIT: '1' });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': helm could not install the hub chart/);
  assert.match(r.all, /BackoffLimitExceeded/);
  assert.match(r.all, /the Job lanekeeper-hub-migrate/);
  assert.doesNotMatch(r.calls, /logs lanekeeper-hub-0/);
});

test('hub: a migration Job that does not complete fails the stage before the seed Job and the replicas are looked at', async () => {
  const r = await runScene('hub-migrate', { SHIM_JOB_MIGRATE: 'timeout' });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': the migration Job \(lanekeeper-hub-migrate\) did not complete in 30 s/);
  assert.doesNotMatch(r.calls, /job\/lanekeeper-hub-seed|logs lanekeeper-hub-0/);
});

test('hub: a seed Job that does not complete, is missing, or has not succeeded fails the stage', async () => {
  let r = await runScene('hub-seed', { SHIM_JOB_SEED: 'timeout' });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': the seed Job \(lanekeeper-hub-seed-4f2a9c1b7e\) did not complete in 30 s/);
  assert.doesNotMatch(r.calls, /logs lanekeeper-hub-0/);
  r = await runScene('hub-seed-missing', { SHIM_SEED_JOBS: '' });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': .*seed Job.*found none/);
  r = await runScene('hub-seed-zero', { SHIM_JOB_SUCCEEDED: '0' });
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 'hub': .*is complete but .status.succeeded is '0', not 1/);
});

test('s6: a connection with sslmode=disable that succeeds is a violation, and the stage fails', async () => {
  const s = await scene('s6-connect', { SHIM_S6_DISABLE: 'connect' });
  const r = await s.runSh();
  s.server.close();
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 's6': S6 VIOLATED: psql with sslmode=disable connected to the database/);
  assert.doesNotMatch(r.all, /^kind-e2e: == hub$/m, 'the hub stage is not reached');
});

test('s6: a refusal for another reason than pg_hba (a wrong password) does not pass the check', async () => {
  const s = await scene('s6-wrongpw', { SHIM_S6_DISABLE: 'wrongpw' });
  const r = await s.runSh();
  s.server.close();
  assert.equal(r.code, 1, r.all);
  assert.match(r.all, /FAILED in the stage 's6': sslmode=disable failed, but not because pg_hba rejects an unencrypted connection \(S6\)/);
});

/** Runs hub_replica_class with the arguments in the environment (a log has quotes, braces and newlines). */
async function classOf(log, terminated, exitCode, ready) {
  const r = await lib('hub_replica_class "$LOG" "$TERMINATED" "$EXIT" "$READY"', { env: { LOG: log, TERMINATED: String(terminated), EXIT: String(exitCode), READY: String(ready) } });
  assert.equal(r.code, 0, r.all);
  return r.out.trim();
}

test('hub_replica_class: a Google refusal with exit 1 is google, and each other state has a class of its own', async () => {
  assert.equal(await classOf(googleLog(), true, 1, false), 'google');
  assert.equal(await classOf(googleLog(WEB_TIMEOUT), true, 1, false), 'google', 'the start timeout of the web certificate is Google-side');
  assert.equal(await classOf(withError(CA_AGENT_SECRET, CA_REPLICA_TIMEOUT), true, 1, false), 'google', 'a CA that cannot be read or set up is Google-side');
  assert.equal(await classOf(googleLog(), true, 1, true), 'ready', 'a Ready replica is not a refusal, whatever its log says');
  assert.equal(await classOf(SETTING_LOG, true, 2, false), 'setting');
  assert.equal(await classOf('', true, 2, false), 'setting', 'exit code 2 is a refused setting even when no log could be had');
  assert.equal(await classOf('hub: the async runtime could not be created', true, 1, false), 'setting', 'every line "hub: ..." is the process stopping before its start');
  assert.equal(await classOf(withError(DB_ERROR), true, 1, false), 'database', 'a database error beside a Google error is still a database error');
  assert.equal(await classOf(withError(DB_TIMEOUT), true, 1, false), 'database');
  assert.equal(await classOf(withError(SCHEMA_ERROR), true, 1, false), 'schema');
  assert.equal(await classOf(`${withError(DB_ERROR)}\n${withError(SCHEMA_ERROR)}`, true, 1, false), 'schema', 'schema before database, as the more specific');
  assert.equal(await classOf(`${SETTING_LOG}\n${googleLog()}`, true, 1, false), 'setting', 'a setting error beside anything is a setting error');
  assert.equal(await classOf(googleLog(), true, 137, false), 'wrong-exit');
  assert.equal(await classOf(googleLog(), true, 0, false), 'wrong-exit');
  assert.equal(await classOf(googleLog(), false, '', false), 'wrong-exit', 'a replica that has not ended has no exit code to judge');
  assert.equal(await classOf(jsonLine('ERROR', 'the hub could not start', { error: 'the web app cannot be served from LK_WEB_DIST: no index.html' }), true, 1, false), 'unknown');
  assert.equal(await classOf(jsonLine('INFO', 'starting'), true, 1, false), 'unknown', 'a log without any error that explains the exit is not explained');
  assert.equal(await classOf('unable to retrieve container logs for containerd://f0ccf06c64bd', true, 1, false), 'unlogged', 'a sentence in place of a log is no log');
  assert.equal(await classOf('', true, 1, false), 'unlogged');
  assert.equal(await classOf('  \n ', true, 1, false), 'unlogged');
});

test('hub_replica_class: the markers are fixed strings, not patterns', async () => {
  // "starting took too long: web certificate" with regex dots would match; here the text must be exact
  assert.equal(await classOf(jsonLine('ERROR', 'the hub could not start', { error: 'starting took too long: webXcertificate' }), true, 1, false), 'unknown');
  assert.equal(await classOf(jsonLine('ERROR', 'the hub could not start', { error: 'starting took too long: MCP server' }), true, 1, false), 'unknown', 'a timeout of a later step is not Google-side');
});

test('hub_verdict: only three replicas that all stopped at Google pass; otherwise the most serious class decides', async () => {
  const v = async (...classes) => {
    const r = await lib(`hub_verdict ${classes.join(' ')}`);
    return `${r.code} ${r.out.trim()}`;
  };
  assert.equal(await v('google', 'google', 'google'), '0 google');
  assert.equal(await v('google', 'setting', 'google'), '1 setting');
  assert.equal(await v('database', 'setting', 'google'), '1 setting', 'a refused setting is named before a database error');
  assert.equal(await v('google', 'schema', 'database'), '1 schema');
  assert.equal(await v('google', 'database', 'ready'), '1 database');
  assert.equal(await v('google', 'ready', 'google'), '1 ready');
  assert.equal(await v('google', 'wrong-exit', 'unlogged'), '1 wrong-exit');
  assert.equal(await v('google', 'unknown', 'unlogged'), '1 unknown');
  assert.equal(await v('google', 'google', 'unlogged'), '1 unlogged');
  assert.equal(await v('google', 'google'), '1 unknown', 'two replicas are not three');
  assert.equal(await v('google', 'google', 'google', 'google'), '1 unknown', 'nor are four');
  assert.equal(await v('google', 'google', 'refused'), '1 unknown', 'a class that does not exist is not a pass');
});

test('hub_first_line quotes the first line that holds a marker, cut to 600 characters', async () => {
  const r = await lib('hub_first_line "$LOG" "the database is not usable" "starting took too long: database"', { env: { LOG: `first\n${withError(DB_ERROR)}\nlast` } });
  assert.equal(r.code, 0, r.all);
  assert.match(r.out, /the database is not usable: could not connect to the database/);
  assert.equal((await lib('hub_first_line "$LOG" "no such marker"', { env: { LOG: 'a\nb' } })).code, 1);
  const long = await lib('hub_first_line "$LOG" "x"; echo', { env: { LOG: `x${'y'.repeat(900)}` } });
  assert.equal(long.out.trim().length, 600);
});
