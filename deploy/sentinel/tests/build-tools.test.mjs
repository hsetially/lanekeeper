// Tests for `deploy/sentinel/build.sh --tools` (09/T8, review round 2; S19, S20, S7). Run: node --test deploy/sentinel/tests/*.test.mjs
//
// The job that signs a release (`sign-release` in .github/workflows/sentinel.yml) holds the Workload Identity credential and receives the
// release key, and it runs `build.sh --tools` to get the image with rpm and gpg. That build must not compile the sentinel: if it asked
// BuildKit for a stage that depends on `binary`, cargo would run the build scripts and proc-macros of every crate in the sentinel's tree in
// that job. These tests run the real script with a `docker` that only records its arguments, and require the target to be `rpm-tools`
// (the stage with no dependency on another stage, which dockerfile_artifact.rego enforces) and nothing that belongs to the sentinel's build.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const script = join(root, 'deploy', 'sentinel', 'build.sh');

/** Runs build.sh with a fake docker that records its arguments, one per line, in `calls`; returns { status, stdout, stderr, calls }. */
function run(args, env = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'build-tools-test-'));
  try {
    const log = join(dir, 'docker.log');
    writeFileSync(join(dir, 'docker'), `#!/bin/sh\n{ echo "-- docker"; for a in "$@"; do printf '%s\\n' "$a"; done; } >> '${log}'\n`);
    chmodSync(join(dir, 'docker'), 0o755);
    const r = spawnSync('bash', [script, ...args], {
      cwd: root,
      encoding: 'utf8',
      env: { PATH: `${dir}:${process.env.PATH}`, HOME: process.env.HOME ?? dir, ...env },
    });
    let calls = [];
    try {
      calls = readFileSync(log, 'utf8')
        .split('-- docker\n')
        .filter((c) => c !== '')
        .map((c) => c.split('\n').filter((l) => l !== ''));
    } catch {
      // docker was never called
    }
    return { status: r.status, stdout: r.stdout, stderr: r.stderr, calls };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const targetsOf = (call) => call.flatMap((a, i) => (a === '--target' ? [call[i + 1]] : []));

test('--tools builds the rpm-tools target, which no stage of the sentinel build is part of', () => {
  const r = run(['--tools', '--worktree', '--tag', 'tools-test']);
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.calls.length, 1, 'one docker call');
  assert.deepEqual(targetsOf(r.calls[0]), ['rpm-tools']);
  assert.equal(r.stdout.trim(), 'tools-test');
});

test('--tools passes no build argument of the sentinel build (the version and the source date belong to the rpm stage)', () => {
  const r = run(['--tools', '--worktree']);
  assert.equal(r.status, 0, r.stderr);
  const buildArgs = r.calls[0].filter((a) => /^(SOURCE_DATE_EPOCH|LK_VERSION)=/.test(a));
  assert.deepEqual(buildArgs, []);
});

test('--tools loads the image under the tag and exports nothing', () => {
  const r = run(['--tools', '--worktree', '--tag', 'tools-test']);
  assert.equal(r.status, 0, r.stderr);
  const call = r.calls[0];
  assert.ok(call.includes('--load'));
  assert.equal(call[call.indexOf('--tag') + 1], 'tools-test');
  assert.ok(!call.includes('--output'));
});

test('--tools still forwards the apt mirror, which the tools stage uses', () => {
  const r = run(['--tools', '--worktree'], { LK_APT_MIRROR: 'https://mirror.example/ubuntu' });
  assert.equal(r.status, 0, r.stderr);
  assert.ok(r.calls[0].includes('LK_APT_MIRROR=https://mirror.example/ubuntu'));
});

test('the build of the files targets out and keeps the version and the source date', () => {
  // The fake docker exports nothing, so the script stops at its check of the output; what matters is the one call it made first.
  const out = mkdtempSync(join(tmpdir(), 'build-tools-test-out-'));
  const r = run(['--worktree', '--out', out]);
  rmSync(out, { recursive: true, force: true });
  assert.notEqual(r.status, 0);
  assert.equal(r.calls.length, 1);
  assert.deepEqual(targetsOf(r.calls[0]), ['out']);
  assert.ok(r.calls[0].some((a) => /^SOURCE_DATE_EPOCH=\d+$/.test(a)));
  assert.ok(r.calls[0].some((a) => /^LK_VERSION=\d+\.\d+\.\d+$/.test(a)));
});
