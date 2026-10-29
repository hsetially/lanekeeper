// The build profiles of the workspace (plan 09 blockers, B20). The shipped images are built with `--release`, whose `codegen-units = 1`
// and `lto = "thin"` are part of what makes a build reproducible and fast at run time, so they must not move. The benchmark profile
// inherits them, and one dev-dependency, jsonschema, took 28 of the 33 minutes of a cold CI compile on one core. A human relaxed
// codegen-units for that one package in the bench profile only. This test holds both halves: the release profile is untouched, and the
// override is as narrow as it was decided.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const cargo = readFileSync(join(root, 'Cargo.toml'), 'utf8');

/** The `key = value` lines of the table `[name]` (comments dropped), as { key: value }. Undefined when there is no such table. */
function table(name) {
  const lines = cargo.split('\n');
  const at = lines.findIndex((l) => l.trim() === `[${name}]`);
  if (at < 0) return undefined;
  const out = {};
  for (const line of lines.slice(at + 1)) {
    if (/^\s*\[/.test(line)) break;
    const m = /^([A-Za-z-]+) = ([^#\n]+?)\s*(?:#.*)?$/.exec(line);
    if (m) out[m[1]] = m[2];
  }
  return out;
}

test('the release profile, which builds the images, keeps codegen-units = 1 and lto = "thin"', () => {
  assert.deepEqual(table('profile.release'), { lto: '"thin"', 'codegen-units': '1' });
});

test('the bench profile relaxes codegen-units for jsonschema and for nothing else', () => {
  assert.deepEqual(table('profile.bench.package.jsonschema'), { 'codegen-units': '16' });
  const overrides = [...cargo.matchAll(/^\[(profile\.(?!release\])[^\]]*)\]/gm)].map((m) => m[1]);
  assert.deepEqual(overrides, ['profile.bench.package.jsonschema'], 'another profile or override appeared: a person decides that (B20)');
  assert.match(cargo, /B20/, 'the override must cite B20, where the decision and the unmeasured saving are recorded');
});

test('jsonschema is a dev-dependency that no benchmark uses, so the override cannot change a measured number', () => {
  const crates = join(root, 'crates');
  const users = [];
  for (const c of readdirSync(crates)) {
    const manifest = join(crates, c, 'Cargo.toml');
    let text;
    try {
      text = readFileSync(manifest, 'utf8');
    } catch {
      continue;
    }
    const at = text.search(/^jsonschema(\.workspace| =)/m);
    if (at < 0) continue;
    const before = text.slice(0, at);
    const section = [...before.matchAll(/^\[([^\]]+)\]/gm)].pop()?.[1];
    assert.equal(section, 'dev-dependencies', `${c} has jsonschema in [${section}]: it would be compiled into a shipped binary`);
    users.push(c);
    const benches = join(crates, c, 'benches');
    if (!exists(benches)) continue;
    for (const f of walk(benches)) {
      assert.ok(!/jsonschema/.test(readFileSync(f, 'utf8')), `${f} uses jsonschema: a benchmark would measure code compiled with the relaxed setting`);
    }
  }
  assert.ok(users.includes('hub-mcp'), 'the check found no crate that uses jsonschema: it is not reading the manifests');
});

function exists(p) {
  try {
    return statSync(p).isDirectory();
  } catch {
    return false;
  }
}

function walk(dir) {
  const files = [];
  for (const n of readdirSync(dir)) {
    const p = join(dir, n);
    if (statSync(p).isDirectory()) files.push(...walk(p));
    else if (n.endsWith('.rs')) files.push(p);
  }
  return files;
}
