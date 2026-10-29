#!/usr/bin/env node
// Reads a systemd unit file and writes it as JSON, for conftest (09/T8; S25).
//
//   node deploy/sentinel/tests/unit2json.mjs UNIT > unit.json
//   conftest test --policy deploy/policy --namespace systemd unit.json
//
// The output is {"Section": {"Key": ["value", ...]}}: every assignment of a key is kept, in order, and an empty assignment is kept as an
// empty string, because in systemd an empty assignment resets a list (ReadWritePaths=) and `CapabilityBoundingSet=` empty means no
// capability at all. Rego then decides what the last word was. Nothing is skipped: a line that systemd would not read as `Key=Value`
// stops the conversion, so a policy never passes on a line that the parser dropped.
//
// What it follows from systemd.syntax(7): a line starting with # or ; is a comment (after white space); a line ending in a backslash
// continues on the next one; `[Name]` opens a section, and a section that appears again is the same section; white space around the
// `=` and at both ends is dropped; there are no inline comments. Bounded input (rule 5): at most 256 KiB.
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const MAX_BYTES = 256 * 1024;
const KEY = /^[A-Za-z][A-Za-z0-9_-]*$/;

export class UnitError extends Error {}

/**
 * @param {string} text
 * @returns {Record<string, Record<string, string[]>>}
 */
export function parseUnit(text) {
  if (Buffer.byteLength(text) > MAX_BYTES) throw new UnitError(`the unit is larger than ${MAX_BYTES} bytes`);
  if (text.includes('\u0000')) throw new UnitError('the unit has a NUL byte');
  const physical = text.split(/\r?\n/);
  // Join continuation lines first, remembering the line where each logical line starts.
  const logical = [];
  for (let i = 0; i < physical.length; i += 1) {
    let line = physical[i];
    const start = i + 1;
    while (line.endsWith('\\')) {
      if (i + 1 >= physical.length) throw new UnitError(`line ${start}: a continuation runs off the end of the file`);
      i += 1;
      line = `${line.slice(0, -1).trimEnd()} ${physical[i].trim()}`;
    }
    logical.push({ no: start, text: line.trim() });
  }
  const unit = {};
  let section = null;
  for (const { no, text: line } of logical) {
    if (line === '' || line.startsWith('#') || line.startsWith(';')) continue;
    if (line.startsWith('[')) {
      const m = /^\[([A-Za-z][A-Za-z0-9._-]*)\]$/.exec(line);
      if (!m) throw new UnitError(`line ${no}: a section header must be [Name]`);
      section = m[1];
      unit[section] ??= {};
      continue;
    }
    if (section === null) throw new UnitError(`line ${no}: an assignment outside any section`);
    const eq = line.indexOf('=');
    if (eq < 0) throw new UnitError(`line ${no}: expected Key=Value`);
    const key = line.slice(0, eq).trim();
    if (key === '') throw new UnitError(`line ${no}: the key is empty`);
    if (!KEY.test(key)) throw new UnitError(`line ${no}: '${key}' is not a directive name (key)`);
    (unit[section][key] ??= []).push(line.slice(eq + 1).trim());
  }
  if (Object.keys(unit).length === 0) throw new UnitError('the unit has no section');
  return unit;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv.length !== 3) {
    console.error('usage: unit2json.mjs UNIT');
    process.exit(2);
  }
  try {
    process.stdout.write(`${JSON.stringify(parseUnit(readFileSync(process.argv[2], 'utf8')), null, 2)}\n`);
  } catch (e) {
    if (!(e instanceof UnitError)) throw e;
    console.error(`unit2json: ${process.argv[2]}: ${e.message}`);
    process.exit(1);
  }
}
