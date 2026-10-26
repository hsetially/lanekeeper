// The osv-scanner configuration (S19; plan 09 blockers, B17). `just osv-scan` passes osv-scanner.toml with --config, so every
// lockfile of the repository is judged by the one file. Each ignore in it was decided by a human, and an ignore without an expiry
// date or without a reason would stay in force for ever, unexplained, so the file is parsed here and every entry is checked.
//
// Node has no TOML parser and this repository adds no dependency for a test, so the parser below accepts only the small subset the
// file uses (`[[Table]]` headers, `key = "string"`, `key = true|false`, `key = YYYY-MM-DD`, comments) and THROWS on anything else.
// A file that leaves the subset fails the test instead of being half read.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const read = (path) => readFileSync(join(root, path), 'utf8');

// The tables and keys of osv-scanner v2's configuration that this repository uses (the documented form, and the pinned 2.6.0 binary
// accepts --config). Any other table or key is refused, so a typo (`ignoreUntill`) cannot silently turn an expiry into nothing.
const TABLES = {
  IgnoredVulns: ['id', 'ignoreUntil', 'reason'],
  PackageOverrides: ['name', 'version', 'ecosystem', 'ignore', 'reason', 'effectiveUntil'],
};

/** The tables of `text` as { IgnoredVulns: [{...}], PackageOverrides: [{...}] }. Throws on any line outside the subset. */
function parse(text) {
  const out = { IgnoredVulns: [], PackageOverrides: [] };
  let current = null;
  text.split('\n').forEach((raw, i) => {
    const at = `line ${i + 1}`;
    const line = stripComment(raw, at).trim();
    if (line === '') return;
    const header = /^\[\[([A-Za-z]+)\]\]$/.exec(line);
    if (header) {
      assert.ok(header[1] in TABLES, `${at}: unknown table [[${header[1]}]]`);
      current = {};
      out[header[1]].push(current);
      current[Symbol.for('table')] = header[1];
      return;
    }
    const pair = /^([A-Za-z]+) = (.+)$/.exec(line);
    assert.ok(pair, `${at}: not a key = value line, a [[table]] header or a comment: ${line}`);
    assert.ok(current, `${at}: a key before any [[table]]`);
    const [, key, value] = pair;
    assert.ok(TABLES[current[Symbol.for('table')]].includes(key), `${at}: unknown key ${key} in [[${current[Symbol.for('table')]}]]`);
    assert.ok(!(key in current), `${at}: ${key} is set twice`);
    current[key] = scalar(value, at);
  });
  return out;
}

/** Drops a trailing `# comment` unless the # is inside a double-quoted string. */
function stripComment(line, at) {
  let quoted = false;
  for (let i = 0; i < line.length; i += 1) {
    const c = line[i];
    if (c === '\\' && quoted) i += 1;
    else if (c === '"') quoted = !quoted;
    else if (c === '#' && !quoted) return line.slice(0, i);
  }
  assert.ok(!quoted, `${at}: a string is not closed on its line (multi-line strings are outside the subset)`);
  return line;
}

function scalar(value, at) {
  if (value === 'true') return true;
  if (value === 'false') return false;
  const date = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (date) {
    const d = new Date(`${value}T00:00:00Z`);
    assert.ok(!Number.isNaN(d.getTime()) && d.toISOString().startsWith(value), `${at}: ${value} is not a calendar date`);
    return { date: value };
  }
  const str = /^"((?:[^"\\]|\\["\\])*)"$/.exec(value);
  assert.ok(str, `${at}: a value that is not a string, a boolean or a YYYY-MM-DD date: ${value}`);
  return str[1].replace(/\\(["\\])/g, '$1');
}

const config = parse(read('osv-scanner.toml'));

test('the parser refuses what is outside the subset the file may use', () => {
  const bad = [
    ['[[Nope]]\nid = "x"', 'unknown table'],
    ['[[IgnoredVulns]]\nignoreUntill = 2026-11-30', 'unknown key'],
    ['[[IgnoredVulns]]\nid = "a"\nid = "b"', 'set twice'],
    ['id = "a"', 'before any'],
    ['[[IgnoredVulns]]\nignoreUntil = 2026-02-30', 'not a calendar date'],
    ['[[IgnoredVulns]]\nignoreUntil = "2026-11-30"', undefined], // a quoted date is a string: fine for the parser, refused by the rules below
    ['[[IgnoredVulns]]\nreason = """long"""', 'not a string'],
    ['[[IgnoredVulns]]\nid = unquoted', 'not a string'],
    ['[IgnoredVulns]\nid = "x"', 'not a key = value'],
  ];
  for (const [text, why] of bad) {
    if (why === undefined) assert.doesNotThrow(() => parse(text));
    else assert.throws(() => parse(text), new RegExp(why), text);
  }
  const ok = parse('# a comment\n[[IgnoredVulns]]\nid = "RUSTSEC-2026-0001" # why # not a comment inside a string\nreason = "has a # in it"\n');
  assert.equal(ok.IgnoredVulns[0].reason, 'has a # in it');
});

test('osv-scanner.toml has ignores, and each has an id, an expiry date and a reason', () => {
  assert.ok(config.IgnoredVulns.length >= 15, `only ${config.IgnoredVulns.length} ignores were found: the check is not reading the file`);
  const seen = new Set();
  for (const e of config.IgnoredVulns) {
    assert.match(String(e.id), /^RUSTSEC-\d{4}-\d{4}$/, 'every ignore names a RustSec advisory: the npm findings are waited for, not ignored (B19)');
    assert.ok(!seen.has(e.id), `${e.id} is ignored twice`);
    seen.add(e.id);
    assert.ok(e.ignoreUntil && typeof e.ignoreUntil === 'object' && 'date' in e.ignoreUntil, `${e.id} has no ignoreUntil date (an ignore without an expiry never ends)`);
    assert.equal(typeof e.reason, 'string', `${e.id} has no reason`);
    assert.ok(e.reason.length >= 80, `${e.id}: the reason is too short to say why it is safe`);
    assert.match(e.reason, /Decided by a human on 2026-\d{2}-\d{2}|approved by a human/i, `${e.id}: the reason does not say that a human decided it`);
  }
});

test('the fourteen zstd advisories share one time-boxed ignore until 2026-11-30, with one wording', () => {
  const zstd = ['0391', '0395', '0396', '0397', '0398', '0399', '0400', '0401', '0402', '0403', '0404', '0405', '0406', '0410'].map((n) => `RUSTSEC-2026-${n}`);
  const entries = zstd.map((id) => {
    const e = config.IgnoredVulns.find((x) => x.id === id);
    assert.ok(e, `${id} is not ignored`);
    return e;
  });
  for (const e of entries) assert.equal(e.ignoreUntil.date, '2026-11-30', `${e.id} expires on another day`);
  assert.equal(new Set(entries.map((e) => e.reason)).size, 1, 'the zstd entries must carry one shared wording');
  const reason = entries[0].reason;
  for (const part of ['tonic 0.14.6', 'zstd::stream::read::{Encoder,Decoder}::new', 'RUSTSEC-2026-0410 is wasm32 only', 'Revisit by 2026-11-30']) {
    assert.ok(reason.includes(part), `the shared reason lost "${part}"`);
  }
});

test('the rsa advisory is ignored until 2027-04-30, for the reason .cargo/audit.toml gives', () => {
  const rsa = config.IgnoredVulns.filter((e) => e.id === 'RUSTSEC-2023-0071');
  assert.equal(rsa.length, 1);
  assert.equal(rsa[0].ignoreUntil.date, '2027-04-30');
  assert.match(rsa[0].reason, /sqlx-mysql/);
  assert.match(rsa[0].reason, /no fixed release/i);
  assert.match(read('.cargo/audit.toml'), /ignore = \["RUSTSEC-2023-0071"\]/, 'cargo-audit and osv-scanner must ignore the same advisory for the same reason');
});

test('the one package override is our own path crate `domain`, which shares a name with a DNS crate of another project', () => {
  assert.equal(config.PackageOverrides.length, 1, 'a package override ignores a whole package: each needs a human and this test');
  const [p] = config.PackageOverrides;
  assert.equal(p.name, 'domain');
  assert.equal(p.version, '0.1.0', 'the override is for our version only, so another `domain` is still reported');
  assert.equal(p.ecosystem, 'crates.io');
  assert.equal(p.ignore, true);
  assert.match(p.reason, /RUSTSEC-2026-0310/);
  assert.match(p.reason, /path crate/);
  // The claim in the reason is checkable: Cargo.lock lists `domain` 0.1.0 with no `source`, which a registry crate always has.
  const lock = read('Cargo.lock');
  const m = /\[\[package\]\]\nname = "domain"\nversion = "([^"]+)"\n(source = [^\n]+\n)?/.exec(lock);
  assert.ok(m, 'Cargo.lock has no package named domain');
  assert.equal(m[1], '0.1.0');
  assert.equal(m[2], undefined, 'Cargo.lock lists `domain` with a source: it is a registry crate now, and the override would hide its advisories');
});

test('the osv-scan recipe passes the configuration to every scan', () => {
  const justfile = read('Justfile');
  const recipe = /^osv-scan:[^\n]*\n((?: {4}[^\n]*\n|\n)+)/m.exec(justfile);
  assert.ok(recipe, 'the Justfile has no osv-scan recipe');
  const scans = [...recipe[1].matchAll(/^ {4}osv-scanner scan source[^\n]*$/gm)].map((m) => m[0]);
  assert.equal(scans.length, 1, 'the recipe is expected to run one osv-scanner scan');
  assert.match(scans[0], /--config osv-scanner\.toml(\s|$)/, 'osv-scan does not pass --config osv-scanner.toml');
});
