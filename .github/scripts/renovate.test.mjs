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
  const lines = text.split('\n').filter((l) => /^LK_[A-Z0-9_]+_IMAGE=/.test(l));
  assert.ok(lines.length >= 5, `only ${lines.length} image lines`);
  // The names may hold digits (LK_ROCKY_8_9_IMAGE): a pattern that stopped at the first digit would leave those images unseen.
  assert.ok(lines.some((l) => l.startsWith('LK_ROCKY_8_9_IMAGE=')), 'the filter does not see the Rocky image lines');
  const re = new RegExp(manager.matchStrings[0]);
  for (const line of lines) {
    const m = re.exec(line);
    assert.ok(m?.groups?.depName && m.groups.currentValue && m.groups.currentDigest, `not matched: ${line}`);
  }
});

test('the Rocky Linux images of the sentinel test are not moved to another release by an update', () => {
  const rule = (config.packageRules ?? []).find((r) => (r.matchPackageNames ?? []).includes('docker.io/rockylinux/rockylinux'));
  assert.ok(rule && rule.enabled === false, 'no rule that turns updates of docker.io/rockylinux/rockylinux off');
});

// The pins of the kind end-to-end test (deploy/tests/kind/pins.env; review round 2, non-blocking 3). Renovate sees what it can express: the two
// images (tag and digest move together), and the version and the manifest URL of CloudNativePG. It cannot compute the sha256 of the release's
// manifest, and the vendored CRDs name the version too, so the CloudNativePG update waits for a person on the dependency dashboard.
const pins = readFileSync(join(root, 'deploy/tests/kind/pins.env'), 'utf8');
const pinLine = (name) => {
  const m = new RegExp(`^${name}=.*$`, 'm').exec(pins);
  assert.ok(m, `pins.env has no ${name}`);
  return m[0];
};
const managerRe = (m) => new RegExp(m.matchStrings[0]);
const managers = config.customManagers ?? [];
const patternCovers = (m, file) => m.managerFilePatterns.some((p) => new RegExp(p.replace(/^\/|\/$/g, '')).test(file));

test('the node image and the CloudNativePG operator image of pins.env are seen by the docker custom manager', () => {
  const manager = managers.find((m) => m.customType === 'regex' && m.datasourceTemplate === 'docker');
  assert.ok(manager && patternCovers(manager, 'deploy/tests/kind/pins.env'), 'the docker custom manager does not cover deploy/tests/kind/pins.env');
  assert.ok(patternCovers(manager, 'deploy/images/tools.env'), 'the docker custom manager stopped covering deploy/images/tools.env');
  for (const [name, dep] of [['LK_KIND_NODE_IMAGE', 'kindest/node'], ['LK_CNPG_OPERATOR_IMAGE', 'ghcr.io/cloudnative-pg/cloudnative-pg']]) {
    const m = managerRe(manager).exec(pinLine(name));
    assert.ok(m?.groups, `not matched: ${name}`);
    assert.equal(m.groups.depName, dep);
    assert.match(m.groups.currentValue, /^v?\d+\.\d+\.\d+$/);
    assert.match(m.groups.currentDigest, /^sha256:[0-9a-f]{64}$/);
  }
});

test('the version of CloudNativePG and the manifest URL are seen from the GitHub releases, and the URL is rebuilt whole', () => {
  const version = /^LK_CNPG_VERSION=(\S+)$/m.exec(pins)?.[1];
  const byLine = (name) => managers.find((m) => m.customType === 'regex' && m.datasourceTemplate === 'github-releases' && m.matchStrings[0].includes(name));
  for (const name of ['LK_CNPG_VERSION', 'LK_CNPG_MANIFEST_URL']) {
    const m = byLine(name);
    assert.ok(m, `no github-releases custom manager for ${name}`);
    assert.ok(patternCovers(m, 'deploy/tests/kind/pins.env'));
    assert.equal(m.depNameTemplate, 'cloudnative-pg/cloudnative-pg');
    assert.equal(m.extractVersionTemplate, '^v(?<version>.+)$', 'the release tag is v1.2.3 and the pin is 1.2.3');
    const hit = managerRe(m).exec(pinLine(name));
    assert.equal(hit?.groups?.currentValue, version, `${name}: the manager reads another version than LK_CNPG_VERSION`);
  }
  // The URL names the version twice (the tag and the file), so Renovate rebuilds the line: with today's version, the line is today's line.
  const url = byLine('LK_CNPG_MANIFEST_URL');
  assert.ok(url.autoReplaceStringTemplate, 'a URL that names the version twice needs autoReplaceStringTemplate');
  assert.equal(url.autoReplaceStringTemplate.replaceAll('{{{newValue}}}', version), pinLine('LK_CNPG_MANIFEST_URL'));
  // With another version, both places move.
  assert.match(url.autoReplaceStringTemplate.replaceAll('{{{newValue}}}', '9.8.7'), /\/v9\.8\.7\/cnpg-9\.8\.7\.yaml$/);
});

test('a CloudNativePG update is one group that waits for a person: the sha256 of the manifest and the vendored CRDs are not Renovate\'s', () => {
  const rule = (config.packageRules ?? []).find((r) => (r.matchPackageNames ?? []).includes('ghcr.io/cloudnative-pg/cloudnative-pg'));
  assert.ok(rule, 'no packageRule for the CloudNativePG operator image');
  assert.ok((rule.matchPackageNames ?? []).includes('cloudnative-pg/cloudnative-pg'), 'the rule leaves the release version out of the group');
  assert.ok(rule.groupName, 'the image, the version and the URL must move in one pull request');
  assert.equal(rule.dependencyDashboardApproval, true, 'no pull request until a person approves it on the dashboard');
  assert.equal(rule.automerge, false);
  const notes = (rule.prBodyNotes ?? []).join('\n');
  assert.match(notes, /LK_CNPG_MANIFEST_SHA256/);
  assert.match(notes, /deploy\/schemas/);
  // The rule must come after the docker group, which would otherwise win the group name for the image.
  const rules = config.packageRules ?? [];
  assert.ok(rules.indexOf(rule) > rules.findIndex((r) => (r.matchDatasources ?? []).includes('docker') && r.groupName));
});

test('the README of the kind test says which pins Renovate moves and which a person does', () => {
  const readme = readFileSync(join(root, 'deploy/tests/kind/README.md'), 'utf8');
  for (const name of ['LK_KIND_NODE_IMAGE', 'LK_CNPG_OPERATOR_IMAGE', 'LK_CNPG_VERSION', 'LK_CNPG_MANIFEST_URL', 'LK_CNPG_MANIFEST_SHA256']) {
    assert.ok(readme.includes(name), `the README does not mention ${name}`);
  }
  assert.match(readme, /cannot compute/i);
  assert.match(readme, /Justfile/);
});
