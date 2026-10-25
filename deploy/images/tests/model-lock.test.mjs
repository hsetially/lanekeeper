// Tests for deploy/images/model-lock.mjs: a resolved lock passes, and each way a lock could let an unpinned or unchecked
// file into the hub image is refused. Run with `node --test deploy/images/tests/`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { checkLock, urlOf } from '../model-lock.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const sha = 'a'.repeat(64);
const rev = '0123456789abcdef0123456789abcdef01234567';

const good = [
  '# a comment',
  '',
  'model bge-small-en-v1.5',
  'repo BAAI/bge-small-en-v1.5',
  `revision ${rev}`,
  'date 2026-01-01T00:00:00Z',
  `file ${sha} 133093490 onnx/model.onnx`,
  `file ${'b'.repeat(64)} 711396 tokenizer.json`,
].join('\n');

const errorsOf = (text, now = new Date('2026-10-25T00:00:00Z')) => checkLock(text, { now }).errors;

test('a resolved lock passes', () => {
  const { errors, lock } = checkLock(good, { now: new Date('2026-10-25T00:00:00Z') });
  assert.deepEqual(errors, []);
  assert.equal(lock.files.length, 2);
  assert.equal(
    urlOf(lock, lock.files[0]),
    `https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/${rev}/onnx/model.onnx`,
  );
});

test('the committed lock is checked by the same code', () => {
  // Until update-model-lock.sh has filled it in, the committed lock must be refused, never accepted half-resolved.
  const text = readFileSync(join(here, '..', 'hub-model.lock'), 'utf8');
  const { errors } = checkLock(text, { now: new Date() });
  if (/^revision UNRESOLVED$/m.test(text)) {
    assert.ok(errors.some((e) => e.includes('UNRESOLVED')), errors.join('\n'));
  } else {
    assert.deepEqual(errors, []);
  }
});

const planted = {
  'unresolved revision': [good.replace(`revision ${rev}`, 'revision UNRESOLVED'), 'UNRESOLVED'],
  'a branch for a revision': [good.replace(`revision ${rev}`, 'revision main'), 'revision'],
  'a short revision': [good.replace(`revision ${rev}`, `revision ${rev.slice(0, 12)}`), 'revision'],
  'no revision': [good.replace(`revision ${rev}\n`, ''), 'revision'],
  'two revisions': [good.replace(`revision ${rev}`, `revision ${rev}\nrevision ${rev}`), 'revision'],
  'a short sha256': [good.replace(sha, sha.slice(1)), 'sha256'],
  'an upper-case sha256': [good.replace(sha, 'A'.repeat(64)), 'sha256'],
  'a zero size': [good.replace('133093490', '0'), 'size'],
  'a size that is not a number': [good.replace('133093490', '12MB'), 'size'],
  'an absolute path': [good.replace('onnx/model.onnx', '/etc/passwd'), 'path'],
  'a parent segment': [good.replace('onnx/model.onnx', 'onnx/../../x'), 'path'],
  'a hidden segment': [good.replace('onnx/model.onnx', '.git/config'), 'path'],
  'a query in the path': [good.replace('onnx/model.onnx', 'model.onnx?download=1'), 'path'],
  'a duplicate path': [good.replace('tokenizer.json', 'onnx/model.onnx'), 'twice'],
  'no files': [good.split('\n').filter((l) => !l.startsWith('file ')).join('\n'), 'no file'],
  'no onnx file': [good.replace('onnx/model.onnx', 'onnx/model.bin'), '.onnx'],
  'no tokenizer': [good.replace(' tokenizer.json', ' vocab.txt'), 'tokenizer.json'],
  'a repo with a host': [good.replace('repo BAAI/bge-small-en-v1.5', 'repo evil.example/BAAI/bge'), 'repo'],
  'a model dir with a slash': [good.replace('model bge-small-en-v1.5', 'model ../bin'), 'model'],
  'an unknown line': [`${good}\nurl https://example.com/model.onnx`, 'unknown'],
  'a file line with extra fields': [good.replace('tokenizer.json', 'tokenizer.json https://example.com/t.json'), 'file'],
  'a revision younger than 14 days': [good.replace('2026-01-01T00:00:00Z', '2026-10-20T00:00:00Z'), '14 days'],
  'no date': [good.replace('date 2026-01-01T00:00:00Z\n', ''), 'date'],
};

for (const [name, [text, fragment]] of Object.entries(planted)) {
  test(`refuses ${name}`, () => {
    const errors = errorsOf(text);
    assert.ok(
      errors.some((e) => e.includes(fragment)),
      `expected an error mentioning "${fragment}", got:\n${errors.join('\n') || '(none)'}`,
    );
  });
}
