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
