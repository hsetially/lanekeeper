// Tests for tools-graph.mjs (09/T8, review round 2; S7, S19, S20). Run: node --test deploy/sentinel/tests/*.test.mjs
// The step names below are the ones BuildKit printed for `docker buildx build --target rpm` of deploy/sentinel/Dockerfile.build before the
// fix (reproduced on 2026-10-26): the plan of the `rpm` target held all seven steps of `binary`, cargo build among them.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import { checkGraph, stepNames } from './tools-graph.mjs';

/** The rawjson progress for these step names: one line per vertex, as `--progress=rawjson` prints them, plus an update that repeats one. */
function progress(names) {
  const vertex = (name) => ({ digest: `sha256:${createHash('sha256').update(name).digest('hex')}`, name });
  const lines = names.map((name) => JSON.stringify({ vertexes: [vertex(name)] }));
  if (names.length > 0) lines.push(JSON.stringify({ vertexes: [{ ...vertex(names[0]), started: '2026-10-26T11:42:36Z' }] }));
  return lines.join('\n');
}

const internal = [
  '[internal] load build definition from Dockerfile.build',
  '[internal] load metadata for docker.io/library/ubuntu:noble-20260930.1@sha256:68ee22004a9109b44973313b02ff7b1de250f1de0f5ecb628d73f579aaff33ac',
];
const binary = [
  '[internal] load metadata for docker.io/library/rust:1.88.0-trixie@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db',
  '[binary 1/7] FROM docker.io/library/rust:1.88.0-trixie@sha256:f2a17efbe58b00470be6e73dbea79705a789ee20ee8de2dcdaf73c0c6091f1db',
  '[binary 2/7] RUN cc --version',
  '[binary 5/7] COPY . .',
  '[binary 6/7] RUN --mount=type=secret,id=build_ca,target=/etc/ssl/certs/ca-certificates.crt,required=false     cargo build --release --locked --target x86_64-unknown-linux-musl -p sentinel',
  '[binary 7/7] RUN set -eu;     bin=/src/target/x86_64-unknown-linux-musl/release/sentinel; readelf -hW "$bin"',
];

test('the plan of the rpm target before the fix holds the binary stage, and is refused', () => {
  const rpm = [
    ...internal,
    ...binary,
    '[rpm 1/7] FROM docker.io/library/ubuntu:noble-20260930.1@sha256:68ee22004a9109b44973313b02ff7b1de250f1de0f5ecb628d73f579aaff33ac',
    '[rpm 2/7] RUN --mount=type=secret,id=build_ca,mode=0444 apt-get update; apt-get install --yes rpm gpg gpg-agent',
    '[rpm 3/7] WORKDIR /build',
    '[rpm 4/7] COPY --from=binary /src/target/x86_64-unknown-linux-musl/release/sentinel /build/SOURCES/lanekeeper-sentinel',
  ];
  const r = checkGraph(progress(rpm), 'rpm');
  assert.equal(r.inconclusive, false);
  assert.equal(r.others.length, 5);
  assert.ok(r.problems.some((p) => p.includes('building rpm also builds: [binary 6/7] RUN')), r.problems.join('\n'));
  assert.ok(r.problems.some((p) => p.startsWith('the graph runs a Rust build tool')), r.problems.join('\n'));
});

test('the plan of a target that holds only its own steps is accepted', () => {
  const tools = [
    ...internal,
    '[rpm-tools 1/3] FROM docker.io/library/ubuntu:noble-20260930.1@sha256:68ee22004a9109b44973313b02ff7b1de250f1de0f5ecb628d73f579aaff33ac',
    "[rpm-tools 2/3] RUN apt-get update; apt-get install --yes --no-install-recommends rpm=4.18.2+dfsg-2.1build2 'gpg=2.4.4-2ubuntu17*' 'gpg-agent=2.4.4-2ubuntu17*'",
    '[rpm-tools 3/3] WORKDIR /build',
  ];
  const r = checkGraph(progress(tools), 'rpm-tools');
  assert.deepEqual(r.problems, []);
  assert.equal(r.inconclusive, false);
  assert.equal(r.own, 3);
});

test('the word cargo in a step of the target itself is refused, as the policy refuses it', () => {
  const r = checkGraph(progress(['[rpm-tools 1/2] FROM x', '[rpm-tools 2/2] RUN cargo install --locked something']), 'rpm-tools');
  assert.ok(r.problems.some((p) => p.startsWith('the graph runs a Rust build tool')));
});

test('a message that names cargo is not a step that runs it', () => {
  const r = checkGraph(progress(['[rpm-tools 1/2] FROM x', '[rpm-tools 2/2] RUN echo this stage never runs cargo']), 'rpm-tools');
  assert.deepEqual(r.problems, []);
});

test('a message that names cargo build, and a flag before the command, are read as the shell reads them', () => {
  const clean = checkGraph(progress(['[rpm-tools 1/2] FROM x', '[rpm-tools 2/2] RUN echo this stage never runs cargo build']), 'rpm-tools');
  assert.deepEqual(clean.problems, []);
  const flagged = checkGraph(progress(['[rpm-tools 1/2] FROM x', '[rpm-tools 2/2] RUN --mount=type=cache,target=/c apt-get update && cargo build']), 'rpm-tools');
  assert.ok(flagged.problems.some((p) => p.startsWith('the graph runs a Rust build tool')));
});

test('a build that stopped before it planned (no step of the target) proves nothing and is not a pass', () => {
  const r = checkGraph(progress(internal), 'rpm-tools');
  assert.equal(r.inconclusive, true);
  assert.equal(r.own, 0);
});

test('an empty file, and lines that are not JSON, are inconclusive', () => {
  assert.equal(checkGraph('', 'rpm-tools').inconclusive, true);
  assert.equal(checkGraph('ERROR: target stage "rpm-tools" could not be found\n{not json', 'rpm-tools').inconclusive, true);
});

test('a step is counted once however often BuildKit repeats its vertex', () => {
  const names = ['[rpm-tools 1/2] FROM x', '[rpm-tools 2/2] RUN true'];
  assert.deepEqual(stepNames(`${progress(names)}\n${progress(names)}`).sort(), names.sort());
});
