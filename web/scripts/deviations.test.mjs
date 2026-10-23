import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// design/DEVIATIONS.md is the record of every difference between the app and the design (D71), and the design owner approves each
// row. This test keeps it a table that can be read by a machine and by that owner: four cells in every row, none empty, one row to a
// screen, and nothing the visual specs leave out without a row (no skipped test, no threshold above the limit).
const root = new URL("../../", import.meta.url);
const text = readFileSync(new URL("design/DEVIATIONS.md", root), "utf8");
const lines = text.split("\n");
const header = lines.findIndex((l) => l.startsWith("| Screen / component |"));
const rows = lines
  .slice(header + 2)
  .filter((l) => l.startsWith("|"))
  .map((l) => l.replace(/^\|\s*/, "").replace(/\s*\|\s*$/, "").split(/\s+\|\s+/));

describe("design/DEVIATIONS.md", () => {
  it("is a table with a header of four columns and a separator", () => {
    expect(header).toBeGreaterThan(0);
    expect(lines[header]).toBe("| Screen / component | Deviation | Reason (security, performance or accessibility) | Approved by |");
    expect(lines[header + 1]).toBe("|---|---|---|---|");
  });

  it("has a row for each difference, and every row has a screen, a difference, a reason and a status", () => {
    expect(rows.length).toBeGreaterThan(100);
    for (const cells of rows) {
      expect(cells, `a row with ${cells.length} cells: ${cells[0] ?? ""}`).toHaveLength(4);
      for (const [i, cell] of cells.entries()) expect(cell.trim(), `cell ${i + 1} of "${cells[0] ?? ""}" is empty`).not.toBe("");
    }
  });

  it("names a screen once", () => {
    const names = rows.map((cells) => (cells[0] ?? "").trim().toLowerCase());
    const twice = names.filter((name, i) => names.indexOf(name) !== i);
    expect(twice).toEqual([]);
  });

  it("says who approved a row, or that nobody has: `pending`", () => {
    const status = rows.map((cells) => (cells[3] ?? "").trim());
    // A name is the design owner's to write. What an agent writes is `pending`.
    for (const s of status) expect(s === "pending" || /^[A-Z][\w .'-]+$/.test(s), `"${s}" is neither pending nor a name`).toBe(true);
    expect(status.filter((s) => s === "pending").length).toBeGreaterThan(0);
  });
});

function sources(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      if (name === "__baselines__" || name === ".refs" || name === "node_modules") continue;
      sources(path, out);
    } else if (/\.(ts|mjs)$/.test(name) && !/\.test\./.test(name)) out.push(path);
  }
  return out;
}

describe("what the visual specs may leave out", () => {
  const e2e = new URL("web/e2e/", root).pathname;
  const files = sources(e2e);

  it("has no test that is marked to be fixed later", () => {
    const found = files.filter((f) => /\btest\.fixme\(|\btest\.fail\(/.test(readFileSync(f, "utf8")));
    expect(found).toEqual([]);
  });

  it("has no per-screen threshold above the design's limit (there is no thresholds file)", () => {
    expect(existsSync(new URL("web/e2e/visual/thresholds.json", root))).toBe(false);
  });
});

// A frame, a masked region or a piece of the chip that the app draws differently from the design says why in a string, and the reason
// is a row of this file: `design/DEVIATIONS.md: "Row", "Row"`, the rows named by their first column. A reason that names a row nobody
// wrote is a difference the design owner is never asked about, so every such name must resolve, and the reasons of the surfaces that task
// T5 added are not allowed to say "candidate row" (a row not written yet).
describe("the rows that the visual specs name", () => {
  const visual = new URL("web/e2e/visual/", root).pathname;
  const known = rows.map((cells) => (cells[0] ?? "").trim());
  const byLength = [...known].sort((a, b) => b.length - a.length);
  const knownSet = new Set(known);
  const T5_DIRS = ["attribution", "prs", "masking", "admin-t5", "pickup", "notify", "served"];
  const HELPERS = /\b(?:APP|T5ROW)\("((?:[^"\\]|\\.)*)"/g;

  /** The text of a file as the reader sees it: a block of comment lines is one line, and an escaped quote is a quote. */
  function readable(path) {
    const out = [];
    let block = null;
    for (const [i, raw] of readFileSync(path, "utf8").split("\n").entries()) {
      const comment = /^\s*(?:\/\/+|\/\*+|\*+(?!\/))\s?(.*)$/.exec(raw);
      if (comment !== null) {
        if (block === null) {
          block = { line: i + 1, comment: true, text: comment[1] ?? "" };
          out.push(block);
        } else block.text += ` ${comment[1] ?? ""}`;
      } else {
        block = null;
        out.push({ line: i + 1, comment: false, text: raw });
      }
    }
    return out.map((b) => ({ ...b, text: b.text.replace(/\\"/g, '"') }));
  }

  /** The row names that follow `design/DEVIATIONS.md:` at `at`, quoted and separated by commas or "and". A name that is not a row is returned as it was written. */
  function namesAfter(text, at) {
    let rest = text.slice(at).replace(/^design\/DEVIATIONS\.md/, "");
    if (!rest.startsWith(":")) return null;
    rest = rest.slice(1).trimStart();
    const names = [];
    while (rest.startsWith('"')) {
      const row = byLength.find((name) => rest.startsWith(`"${name}"`));
      const name = row ?? /^"([^"]*)"/.exec(rest)?.[1] ?? "";
      names.push(name);
      rest = rest.slice(name.length + 2).replace(/^\s*(?:,|\band\b)\s*/, "");
    }
    return names.length === 0 ? null : names;
  }

  function mentions(path) {
    const found = [];
    for (const block of readable(path)) {
      for (let at = block.text.indexOf("design/DEVIATIONS.md"); at >= 0; at = block.text.indexOf("design/DEVIATIONS.md", at + 1)) {
        found.push({ line: block.line, comment: block.comment, names: namesAfter(block.text, at), text: block.text.slice(Math.max(0, at - 50), at + 70) });
      }
      for (const m of block.text.matchAll(HELPERS)) found.push({ line: block.line, comment: false, names: [m[1] ?? ""], text: m[0] });
    }
    return found;
  }

  const files = sources(visual);
  const here = (path) => path.slice(visual.length);

  it("finds the specs, so that the checks below are not empty", () => {
    expect(files.length).toBeGreaterThan(40);
    const all = files.flatMap((f) => mentions(f).filter((m) => m.names !== null));
    expect(all.length).toBeGreaterThan(50);
  });

  it("names only rows that exist", () => {
    const missing = files.flatMap((f) =>
      // A name written `${row}` is a helper's parameter; the helper's calls are checked by their argument.
      mentions(f).flatMap((m) => (m.names ?? []).filter((n) => !n.includes("${") && !knownSet.has(n)).map((n) => `${here(f)}:${m.line} names "${n}", which is not a row of design/DEVIATIONS.md`)),
    );
    expect(missing).toEqual([]);
  });

  it("names the row in every reason of the T5 surfaces: a string that mentions the file says which row", () => {
    const bare = files
      .filter((f) => T5_DIRS.some((d) => here(f).startsWith(`${d}/`)))
      .flatMap((f) => mentions(f).filter((m) => !m.comment && m.names === null).map((m) => `${here(f)}:${m.line} mentions the file and no row: ...${m.text}...`));
    expect(bare).toEqual([]);
  });

  it("has no \"candidate row\" among the T5 surfaces: a difference is a row before the task ends", () => {
    const candidates = files
      .filter((f) => T5_DIRS.some((d) => here(f).startsWith(`${d}/`)))
      .flatMap((f) => readable(f).filter((b) => /candidate row/i.test(b.text)).map((b) => `${here(f)}:${b.line}: ${b.text.trim().slice(0, 90)}`));
    expect(candidates).toEqual([]);
  });
});
