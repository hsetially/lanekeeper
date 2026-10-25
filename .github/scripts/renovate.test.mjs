// Tests for renovate.json (09/T2; S19): the settings that keep the supply chain safe must stay in the file. Renovate itself is not
// run here; these tests read the configuration the way a reviewer would and fail when a setting is removed or weakened.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const config = JSON.parse(readFileSync(join(root, 'renovate.json'), 'utf8'));
const rules = config.packageRules ?? [];

/** "14 days", "2 weeks", "336 hours" in days. */
function days(text) {
  const m = /^\s*(\d+)\s*(day|days|week|weeks|hour|hours|d|w|h)\s*$/.exec(String(text));
  assert.ok(m, `cannot read the duration '${text}'`);
  const n = Number(m[1]);
  return m[2].startsWith('w') ? n * 7 : m[2].startsWith('h') ? n / 24 : n;
}

/** A glob with ** and * as Renovate's matchFileNames reads it, as a RegExp. */
function glob(pattern) {
  const re = pattern
    .replace(/[.+^${}()|[\]\\]/g, '\\$&')
    .replace(/\*\*\//g, '\u0001')
    .replace(/\*\*/g, '\u0002')
    .replace(/\*/g, '[^/]*')
    .replace(/\u0001/g, '(?:.*/)?')
    .replace(/\u0002/g, '.*')
    .replace(/\?/g, '[^/]');
  return new RegExp(`^${re}$`);
}
const covers = (patterns, file) => patterns.some((p) => glob(p).test(file));

test('a release must be at least 14 days old before Renovate proposes it', () => {
  assert.ok(days(config.minimumReleaseAge) >= 14, `minimumReleaseAge is ${config.minimumReleaseAge}`);
  assert.equal(config.internalChecksFilter, 'strict', 'a release that is too young must not even show as a pending check');
});

test('GitHub Actions and Docker images are pinned by digest', () => {
  assert.ok(config.extends.includes('helpers:pinGitHubActionDigests'));
  assert.ok(config.extends.includes('docker:pinDigests'));
});

test('nothing merges by itself', () => {
  assert.equal(config.automerge, false, 'the top-level automerge must be false');
  for (const r of rules) assert.notEqual(r.automerge, true, `a rule turns automerge on: ${JSON.stringify(r)}`);
});

test('updates are grouped: cargo, npm, actions, docker digests, helm', () => {
  const group = (match) => rules.find((r) => match(r) && r.groupName)?.groupName;
  assert.ok(group((r) => r.matchManagers?.includes('cargo')), 'cargo');
  assert.ok(group((r) => r.matchManagers?.includes('npm')), 'npm');
  assert.ok(group((r) => r.matchManagers?.includes('github-actions')), 'github-actions');
  assert.ok(group((r) => r.matchDatasources?.includes('docker')), 'docker');
  assert.ok(group((r) => r.matchManagers?.some((m) => m.startsWith('helm'))), 'helm');
});

// AGENTS.md, "Ownership": the contract paths, as files that a change could touch.
const contractFiles = [
  'crates/domain/src/lib.rs',
  'crates/ports/src/lib.rs',
  'crates/proto/Cargo.toml',
  'proto/agent.proto',
  'api/openapi.yaml',
  'db/migrations/0001_init.sql',
  'docs/mcp-tools.md',
  'perf/budgets.toml',
  'design/handoff/screens/x.dc.html',
];
for (const file of contractFiles) {
  test(`a change to ${file} is labelled contract-change and never merges by itself`, () => {
    const hit = rules.filter((r) => r.matchFileNames && covers(r.matchFileNames, file));
    assert.ok(hit.length > 0, `no packageRule matches ${file}`);
    assert.ok(hit.some((r) => r.automerge === false), `no matching rule sets automerge: false for ${file}`);
    assert.ok(hit.some((r) => (r.labels ?? []).includes('contract-change')), `no matching rule labels ${file} contract-change`);
  });
}

test('the planted fixtures of the pin check are not proposed for update', () => {
  assert.ok(covers(config.ignorePaths ?? [], '.github/scripts/fixtures/pins/bad-tag.yml'));
  assert.ok(covers(config.ignorePaths ?? [], '.github/scripts/fixtures/workflows/bad-write-all.yml'));
});

test('every image of deploy/images/tools.env is seen by a custom manager', () => {
  const manager = (config.customManagers ?? []).find((m) => m.customType === 'regex' && m.datasourceTemplate === 'docker');
  assert.ok(manager, 'no regex custom manager for docker images');
  assert.ok(manager.managerFilePatterns.some((p) => new RegExp(p.replace(/^\/|\/$/g, '')).test('deploy/images/tools.env')));
  const text = readFileSync(join(root, 'deploy/images/tools.env'), 'utf8');
  const lines = text.split('\n').filter((l) => /^LK_[A-Z_]+_IMAGE=/.test(l));
  assert.ok(lines.length >= 5, `only ${lines.length} image lines`);
  const re = new RegExp(manager.matchStrings[0]);
  for (const line of lines) {
    const m = re.exec(line);
    assert.ok(m?.groups?.depName && m.groups.currentValue && m.groups.currentDigest, `not matched: ${line}`);
  }
});
