// Tests for github-hook-ranges.mjs. Run: node --test deploy/tools/*.test.mjs
// The /meta document in testdata is synthetic (documentation addresses); nothing here touches the network.
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

import { fetchMeta, parseCommitted, parseMeta, render, run } from './github-hook-ranges.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const metaText = readFileSync(join(here, 'testdata/meta.json'), 'utf8');
const meta = (hooks) => JSON.stringify({ hooks });

/** A directory with an output file; the callback gets its path. */
async function withOutput(contents, fn) {
  const dir = mkdtempSync(join(tmpdir(), 'hooks-'));
  const out = join(dir, 'github-hook-ranges.yaml');
  if (contents !== undefined) writeFileSync(out, contents);
  try {
    return await fn(out, dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const capture = () => {
  const lines = { out: [], err: [] };
  return { lines, io: { log: (m) => lines.out.push(m), error: (m) => lines.err.push(m) } };
};

test('only the hooks list is used, sorted (as text) and without duplicates', () => {
  assert.deepEqual(parseMeta(metaText), [
    '198.51.100.0/24',
    '2001:db8:1::/48',
    '2001:db8::/32',
    '203.0.113.0/25',
    '203.0.113.128/25',
  ]);
});

for (const [name, text, fragment] of [
  ['not JSON', 'nope', 'JSON'],
  ['no hooks', JSON.stringify({ web: ['192.0.2.0/24'] }), 'hooks'],
  ['hooks that is not a list', JSON.stringify({ hooks: '203.0.113.0/24' }), 'hooks'],
  ['an empty list', meta([]), 'empty'],
  ['an entry that is not a string', meta([5]), 'string'],
  ['an address that is not an address', meta(['not-an-ip/24']), 'not-an-ip/24'],
  ['an IPv4 octet over 255', meta(['999.1.1.1/24']), '999.1.1.1/24'],
  ['an IPv4 prefix over 32', meta(['203.0.113.0/33']), '203.0.113.0/33'],
  ['an IPv6 prefix over 128', meta(['2001:db8::/129']), '2001:db8::/129'],
  ['a range without a prefix', meta(['203.0.113.1']), '203.0.113.1'],
  ['the whole IPv4 internet', meta(['203.0.113.0/24', '0.0.0.0/0']), '0.0.0.0/0'],
  ['the whole IPv6 internet', meta(['203.0.113.0/24', '::/0']), '::/0'],
  ['a range that is nearly the internet', meta(['10.0.0.0/8']), 'too wide'],
  ['an IPv6 range that is nearly the internet', meta(['2000::/16']), 'too wide'],
  ['more ranges than any real list', meta(Array.from({ length: 65 }, (_, i) => `203.0.${i}.0/24`)), 'more than'],
  ['a quote that would end the Cloud Armor expression', meta(["203.0.113.0/24' || true || '"]), 'not a CIDR'],
]) {
  test(`parseMeta refuses ${name}`, () => {
    assert.throws(() => parseMeta(text), (error) => error.message.includes(fragment), `expected a message with '${fragment}'`);
  });
}

test('what is written is read back', () => {
  const ranges = parseMeta(metaText);
  assert.deepEqual(parseCommitted(render(ranges)), ranges);
});

test('the unresolved file reads as empty', () => {
  assert.deepEqual(parseCommitted('# comment\ncloudArmor:\n  githubHookRanges: []\n'), []);
});

test('--write writes the file and --check then passes', async () => {
  await withOutput(undefined, async (out) => {
    const first = capture();
    assert.equal(await run(['--write', '--out', out], { fetchText: async () => metaText, ...first.io }), 0);
    const second = capture();
    assert.equal(await run(['--check', '--out', out], { fetchText: async () => metaText, ...second.io }), 0);
    assert.match(second.lines.out.join('\n'), /5 ranges/);
    assert.match(readFileSync(out, 'utf8'), /^# GENERATED/m);
  });
});

test('--check fails when GitHub has a range the file lacks, and names it', async () => {
  await withOutput(undefined, async (out) => {
    await run(['--write', '--out', out], { fetchText: async () => metaText, ...capture().io });
    const changed = JSON.parse(metaText);
    changed.hooks.push('192.0.2.64/26');
    const c = capture();
    assert.equal(await run(['--check', '--out', out], { fetchText: async () => JSON.stringify(changed), ...c.io }), 1);
    assert.match(c.lines.err.join('\n'), /192\.0\.2\.64\/26/);
  });
});

test('--check fails when the file has a range GitHub no longer lists', async () => {
  await withOutput(undefined, async (out) => {
    await run(['--write', '--out', out], { fetchText: async () => metaText, ...capture().io });
    const changed = JSON.parse(metaText);
    changed.hooks = changed.hooks.filter((r) => r !== '198.51.100.0/24');
    const c = capture();
    assert.equal(await run(['--check', '--out', out], { fetchText: async () => JSON.stringify(changed), ...c.io }), 1);
    assert.match(c.lines.err.join('\n'), /198\.51\.100\.0\/24/);
  });
});

test('--check fails on an unresolved (empty) file', async () => {
  await withOutput('cloudArmor:\n  githubHookRanges: []\n', async (out) => {
    const c = capture();
    assert.equal(await run(['--check', '--out', out], { fetchText: async () => metaText, ...c.io }), 1);
    assert.match(c.lines.err.join('\n'), /empty|unresolved/i);
  });
});

test('--check fails when the file is missing', async () => {
  await withOutput(undefined, async (out) => {
    const c = capture();
    assert.equal(await run(['--check', '--out', out], { fetchText: async () => metaText, ...c.io }), 1);
  });
});

test('--meta reads a recorded document instead of the network', async () => {
  await withOutput(undefined, async (out, dir) => {
    const file = join(dir, 'meta.json');
    writeFileSync(file, metaText);
    const c = capture();
    assert.equal(
      await run(['--write', '--out', out, '--meta', file], {
        fetchText: async () => {
          throw new Error('the network must not be used');
        },
        ...c.io,
      }),
      0,
    );
  });
});

test('--write does not overwrite the file when /meta cannot be read', async () => {
  await withOutput('# precious\ncloudArmor:\n  githubHookRanges: [203.0.113.0/24]\n', async (out) => {
    const c = capture();
    const status = await run(['--write', '--out', out], {
      fetchText: async () => {
        throw new Error('HTTP 403');
      },
      ...c.io,
    });
    assert.equal(status, 1);
    assert.match(readFileSync(out, 'utf8'), /precious/);
    assert.match(c.lines.err.join('\n'), /HTTP 403/);
  });
});

test('--write does not overwrite the file with a list that fails validation', async () => {
  await withOutput('# precious\ncloudArmor:\n  githubHookRanges: [203.0.113.0/24]\n', async (out) => {
    const c = capture();
    assert.equal(await run(['--write', '--out', out], { fetchText: async () => meta(['0.0.0.0/0']), ...c.io }), 1);
    assert.match(readFileSync(out, 'utf8'), /precious/);
  });
});

test('an unknown argument is refused', async () => {
  const c = capture();
  assert.equal(await run(['--frobnicate'], { fetchText: async () => metaText, ...c.io }), 2);
});

test('fetchMeta retries a failing request, a bounded number of times', async () => {
  let calls = 0;
  const flaky = async () => {
    calls += 1;
    if (calls < 3) throw new Error('reset');
    return metaText;
  };
  assert.equal(await fetchMeta(flaky, { attempts: 3, delayMs: 0 }), metaText);
  assert.equal(calls, 3);
  calls = 0;
  await assert.rejects(fetchMeta(async () => { calls += 1; throw new Error('reset'); }, { attempts: 3, delayMs: 0 }), /reset/);
  assert.equal(calls, 3);
});

test('the committed file is generated output or the unresolved marker, never hand-typed', () => {
  const text = readFileSync(join(here, '../values/hub/github-hook-ranges.yaml'), 'utf8');
  const ranges = parseCommitted(text);
  if (ranges.length > 0) assert.match(text, /^# GENERATED by deploy\/tools\/github-hook-ranges\.mjs/m);
  else assert.match(text, /UNRESOLVED/);
});
