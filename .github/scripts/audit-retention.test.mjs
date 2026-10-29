// The retention of the audit checkpoint bucket (S9; plan 09 A19). A human decided on 2026-10-26 that prod keeps the hourly checkpoints
// for three months (90 days = 7,776,000 seconds) under a LOCKED policy, which cannot be undone, so the value is pinned here: a change
// to it is a decision, not an edit. The chart keeps refusing to render without a period (it has no default), and no file may still call
// the earlier one-year number an example or a pending decision.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const read = (path) => readFileSync(join(root, path), 'utf8');

/** The text of `auditCheckpoint:` in a gcp values file: from its key to the next key of the same indent. */
function auditBlock(path) {
  const text = read(path);
  const m = /^( *)auditCheckpoint:\n((?:\1 +[^\n]*\n|\n)+)/m.exec(text);
  assert.ok(m, `${path} has no auditCheckpoint block`);
  return m[2];
}

test('prod keeps the audit checkpoints for 90 days (7,776,000 s) under a locked policy, and says a human decided it', () => {
  assert.equal(90 * 24 * 60 * 60, 7776000);
  const block = auditBlock('deploy/values/gcp/prod.yaml');
  assert.match(block, /^ +retentionPeriodSeconds: 7776000$/m);
  assert.match(block, /^ +isLocked: true$/m, 'the policy requires the lock in prod (storage.rego)');
  assert.match(
    block,
    /# [^\n]*decided by a human on 2026-10-26: three months \(90 days = 7,776,000 s\); the lock cannot be undone/,
    'the comment above the period must say who decided it and that the lock is permanent',
  );
  assert.doesNotMatch(block, /EXAMPLE/, 'the period is decided, not an example');
});

test('staging is never locked', () => {
  const block = auditBlock('deploy/values/gcp/staging.yaml');
  assert.match(block, /^ +isLocked: false$/m);
  assert.doesNotMatch(block, /retentionPeriodSeconds: 7776000/, 'staging has its own, shorter period');
});

test('the chart still has no default period and refuses to render without one', () => {
  assert.match(auditBlock('deploy/charts/gcp/values.yaml'), /^ +retentionPeriodSeconds: 0$/m, 'the chart default must stay unset (0)');
  const template = read('deploy/charts/gcp/templates/buckets.yaml');
  assert.match(template, /\{\{- if le \$period 0 \}\}\n\{\{- fail "buckets\.auditCheckpoint\.retentionPeriodSeconds is required and has no default/);
});

/** Every file under `dir` (relative paths), but not the vendored schemas. */
function walk(dir) {
  const out = [];
  for (const n of readdirSync(join(root, dir))) {
    const p = join(dir, n);
    if (p === join('deploy', 'schemas')) continue;
    if (statSync(join(root, p)).isDirectory()) out.push(...walk(p));
    else out.push(p);
  }
  return out;
}

test('no file under deploy/ or in the threat model still carries the one-year number or calls the period an example', () => {
  const files = [...walk('deploy'), 'docs/threat-model.md'].filter((f) => /\.(ya?ml|md|rego|sh|mjs|tpl|json)$/.test(f));
  assert.ok(files.length > 100, `only ${files.length} files were read: the check is not walking the tree`);
  const stale = [/31536000/, /one[- ]year/i, /EXAMPLE period \(plan 09 A19\): one year/, /period is an? (EXAMPLE|example)/, /an example that a human must confirm/];
  for (const f of files) {
    const lines = read(f).split('\n');
    lines.forEach((line, i) => {
      for (const re of stale) assert.doesNotMatch(line, re, `${relative('.', f)}:${i + 1} still carries a stale statement about the retention period: ${line.trim().slice(0, 120)}`);
    });
  }
});
