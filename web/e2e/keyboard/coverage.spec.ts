// The keymap has to be tested or explained (H-11). The rows of the handoff's Engineering page ("Keyboard") are read from the
// extracted data, plus the reviewed screen-local rows; each must be named by a test in this folder (`test(row("id") + ...)`: the
// source of every spec here is read, so a title that names a row is a claim this test checks) or be on the list below of rows
// that cannot be tested, with the reason. A row the handoff gains fails here until it has a test or a reason, and a test that
// names a row the keymap no longer has fails too. Nothing in the folder may be skipped, marked fixme or made exclusive.
//
// This does not run the tests it counts: they are the specs next to it, in the same run. It reads their source because a
// Playwright worker cannot see what another file registered.
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { KEYMAP } from "../../src/handoff/keymap.generated.ts";
import { KEYMAP_DATA } from "../../src/keyboard/keymap.ts";
import { LOCAL_ROWS } from "../../src/keyboard/rows.ts";
import { expect, test } from "../fixtures/test";

const DIR = fileURLToPath(new URL(".", import.meta.url));
const specs = readdirSync(DIR)
  .filter((name) => name.endsWith(".spec.ts"))
  .sort();
const source = (name: string): string => readFileSync(`${DIR}${name}`, "utf8");

/** Rows that have no test, and why. Each is reviewed: add one here only for a row that has no key to press. */
const NOT_APPLICABLE: Readonly<Record<string, string>> = {
  "search.toggle-buttons-no-shortcut-assigned": "Match case and Regex are toggle buttons with no shortcut assigned yet (the handoff suggests Alt+C and Alt+R and leaves it undecided); there is no key to press.",
};

/** Every id a test of this folder names as its row, with the file. */
function marked(): Array<{ id: string; file: string }> {
  const found: Array<{ id: string; file: string }> = [];
  for (const file of specs.filter((name) => name !== "coverage.spec.ts")) {
    for (const match of source(file).matchAll(/\btest\(\s*row\(\s*"([^"]+)"\s*\)/g)) found.push({ id: match[1] ?? "", file });
  }
  return found;
}

const handoffRows = KEYMAP.rows.map((r) => r.id);
const localRows = LOCAL_ROWS.map((r) => r.id);

test("every row of the handoff's keymap, and every screen-local row, has a test or is on the reviewed list of rows that have no key", () => {
  const named = new Set(marked().map((m) => m.id));
  const all = [...handoffRows, ...localRows];
  expect(all.length, "the keymap was read").toBeGreaterThan(30);
  const missing = all.filter((id) => !named.has(id) && NOT_APPLICABLE[id] === undefined);
  expect(missing, "rows with neither a test nor a reason").toEqual([]);
});

test("a test names only rows the keymap has, and a row on the reviewed list is a row that is really without a key and without a test", () => {
  const all = new Set<string>([...handoffRows, ...localRows]);
  expect(
    marked()
      .filter((m) => !all.has(m.id))
      .map((m) => `${m.file}: ${m.id}`),
    "tests for rows the keymap no longer has",
  ).toEqual([]);
  const named = new Set(marked().map((m) => m.id));
  for (const [id, reason] of Object.entries(NOT_APPLICABLE)) {
    const row = KEYMAP.rows.find((r) => r.id === id);
    expect(row, `${id} is a row of the handoff's keymap`).toBeDefined();
    expect(reason.length, `${id} has a reason`).toBeGreaterThan(20);
    expect(row?.bindings, `${id} has no key to press`).toEqual([]);
    expect(named.has(id), `${id} is on the list of rows without a test, and also has one`).toBe(false);
  }
});

test("the reviewed copy of the keymap that the app reads is the extracted one, row for row", () => {
  expect(KEYMAP_DATA.map((r) => r.id)).toEqual(handoffRows);
  expect(JSON.parse(JSON.stringify(KEYMAP_DATA))).toEqual(KEYMAP.rows);
});

test("no test in the folder is switched off, skipped or made exclusive, so that a row claimed by a test is a row that is tested", () => {
  const offenders: string[] = [];
  for (const file of specs) {
    for (const bad of source(file).matchAll(/\b(?:test|it)\.(?:skip|fixme|only|fail)\b|\b(?:test\.)?describe\.(?:skip|fixme|only)\b/g)) offenders.push(`${file}: ${bad[0]}`);
  }
  expect(offenders, "tests that are switched off or alone").toEqual([]);
  expect(specs, "the specs of the folder").toEqual(expect.arrayContaining(["coverage.spec.ts", "file.spec.ts", "focus.spec.ts", "keymap.spec.ts", "lists.spec.ts", "screens.spec.ts"]));
});
