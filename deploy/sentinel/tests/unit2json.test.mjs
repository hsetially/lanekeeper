// Tests for unit2json.mjs. Run: node --test deploy/sentinel/tests/*.test.mjs
// The converter is the parser in front of policy/systemd.rego, so it is held to what systemd does with a unit file: repeated keys
// accumulate, an empty assignment is kept (it resets a list, and `CapabilityBoundingSet=` empty means "no capability"), and anything
// that systemd would not read as `Key=Value` is refused instead of skipped.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { parseUnit, UnitError } from './unit2json.mjs';

test('sections, keys and values are read in order', () => {
  const unit = parseUnit('[Unit]\nDescription=A thing\n\n[Service]\nType=simple\nExecStart=/usr/bin/x --flag\n');
  assert.deepEqual(unit, { Unit: { Description: ['A thing'] }, Service: { Type: ['simple'], ExecStart: ['/usr/bin/x --flag'] } });
});

test('a repeated key keeps every assignment, in order', () => {
  const unit = parseUnit('[Service]\nSystemCallFilter=@system-service\nSystemCallFilter=~@privileged @resources\n');
  assert.deepEqual(unit.Service.SystemCallFilter, ['@system-service', '~@privileged @resources']);
});

test('an empty assignment is kept as an empty string', () => {
  const unit = parseUnit('[Service]\nCapabilityBoundingSet=\nAmbientCapabilities=\n');
  assert.deepEqual(unit.Service.CapabilityBoundingSet, ['']);
  assert.deepEqual(unit.Service.AmbientCapabilities, ['']);
});

test('an empty assignment after a value stays in its place, so a later reset can be seen', () => {
  const unit = parseUnit('[Service]\nReadWritePaths=/a\nReadWritePaths=\nReadWritePaths=/b\n');
  assert.deepEqual(unit.Service.ReadWritePaths, ['/a', '', '/b']);
});

test('comments and blank lines are ignored, whether they start with # or ;', () => {
  const unit = parseUnit('# a comment\n; another\n\n[Service]\n   # indented comment\nUser=x\n');
  assert.deepEqual(unit, { Service: { User: ['x'] } });
});

test('white space around the equals sign and at the ends is dropped', () => {
  const unit = parseUnit('[Service]\n  User  =  lanekeeper-sentinel  \n');
  assert.deepEqual(unit.Service.User, ['lanekeeper-sentinel']);
});

test('a # inside a value is part of the value (systemd has no inline comments)', () => {
  const unit = parseUnit('[Service]\nDescription=a # not a comment\n');
  assert.deepEqual(unit.Service.Description, ['a # not a comment']);
});

test('a line that ends in a backslash continues on the next line', () => {
  const unit = parseUnit('[Service]\nExecStart=/usr/bin/x \\\n  --a \\\n  --b\nUser=u\n');
  assert.deepEqual(unit.Service.ExecStart, ['/usr/bin/x --a --b']);
  assert.deepEqual(unit.Service.User, ['u']);
});

test('a section that appears twice is one section', () => {
  const unit = parseUnit('[Service]\nA=1\n[Unit]\nB=2\n[Service]\nC=3\n');
  assert.deepEqual(unit.Service, { A: ['1'], C: ['3'] });
});

test('CRLF line ends are read as LF', () => {
  const unit = parseUnit('[Service]\r\nUser=u\r\n');
  assert.deepEqual(unit.Service.User, ['u']);
});

test('a key outside any section is refused, with its line number', () => {
  assert.throws(() => parseUnit('User=x\n[Service]\n'), (e) => e instanceof UnitError && /line 1/.test(e.message) && /outside/.test(e.message));
});

test('a line without an equals sign is refused', () => {
  assert.throws(() => parseUnit('[Service]\nUser\n'), (e) => e instanceof UnitError && /line 2/.test(e.message) && /Key=Value/.test(e.message));
});

test('a section header that is not closed is refused', () => {
  assert.throws(() => parseUnit('[Service\nUser=x\n'), (e) => e instanceof UnitError && /line 1/.test(e.message) && /section/.test(e.message));
});

test('an empty section name is refused', () => {
  assert.throws(() => parseUnit('[]\nUser=x\n'), (e) => e instanceof UnitError && /section/.test(e.message));
});

test('an empty key is refused', () => {
  assert.throws(() => parseUnit('[Service]\n=x\n'), (e) => e instanceof UnitError && /line 2/.test(e.message) && /key/.test(e.message));
});

test('a key with characters that are not a directive name is refused', () => {
  assert.throws(() => parseUnit('[Service]\nUser name=x\n'), (e) => e instanceof UnitError && /key/.test(e.message));
});

test('a continuation that runs off the end of the file is refused', () => {
  assert.throws(() => parseUnit('[Service]\nExecStart=/x \\'), (e) => e instanceof UnitError && /continu/.test(e.message));
});

test('a file with no section at all is refused', () => {
  assert.throws(() => parseUnit('# only a comment\n'), (e) => e instanceof UnitError && /no section/.test(e.message));
});

test('a unit larger than the bound is refused before it is parsed', () => {
  assert.throws(() => parseUnit(`[Service]\n${'A=1\n'.repeat(100000)}`), (e) => e instanceof UnitError && /larger than/.test(e.message));
});

test('a NUL byte is refused', () => {
  assert.throws(() => parseUnit('[Service]\nUser=a\u0000b\n'), (e) => e instanceof UnitError && /NUL/.test(e.message));
});
