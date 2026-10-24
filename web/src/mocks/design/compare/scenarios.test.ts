import { describe, expect, it } from "vitest";
import { hunksAround, settingSummary } from "../../../features/compare/grid/drawer";
import { rowFacts } from "../../../features/compare/model";
import { GRID_ROWS } from "../../perf/grid";
import { resolveBody, type MockContext, type MockResponse } from "../../scenario";
import { scenarios } from "./index";
import { CORE_FILE, designCompare, designGrid, designRows, designTree } from "./data";

// The Compare scenarios serve the design's data the way the hub serves it. These tests read the data back through the same
// code the screen uses, so that a scenario cannot drift from what the screen needs (a hunk the drawer cannot find, a cursor
// that does not end).

const q = (text: string): URLSearchParams => new URLSearchParams(text);
const SIX = "sit,sitb,sitc,presita,presitb,presitc";

function bodyOf(id: string, path: string, query: string): unknown {
  const scenario = scenarios.find((s) => s.id === id);
  const response = scenario?.responses.find((r: MockResponse) => r.path === path);
  if (response === undefined) throw new Error(`${id} has no ${path}`);
  const context: MockContext = { params: {}, query: q(query), headers: new Headers(), body: undefined };
  return resolveBody(response, context);
}

describe("the design's data", () => {
  it("is 5,000 settings: 440 shared and 1,520 in each of three channels, the same every time", () => {
    const rows = designRows();
    expect(rows).toHaveLength(5000);
    expect(rows.filter((r) => r.ch === null)).toHaveLength(440);
    for (const ch of ["remote-itm-teller", "atm-iso", "atm"]) expect(rows.filter((r) => r.ch === ch)).toHaveLength(1520);
    expect(designRows()).toBe(rows);
  });

  it("has about three settings in ten differing somewhere, and none between the base swimlanes", () => {
    const rows = designRows();
    const differing = rows.filter((r) => r.differ).length;
    expect(differing).toBeGreaterThan(1000);
    expect(differing).toBeLessThan(2000);
    for (const row of rows) expect(row.cells[2]).toEqual(row.cells[5]?.k === "a" ? { k: "a" } : row.cells[5]);
  });

  it("serves the rows for the swimlanes asked for, in the order asked, with a cell for each", () => {
    const grid = designGrid(q(`file=${CORE_FILE}&swimlanes=sitb,sit&limit=5`));
    expect(grid.swimlanes).toEqual(["sitb", "sit"]);
    expect(grid.rows.items).toHaveLength(5);
    for (const row of grid.rows.items) expect(row.cells).toHaveLength(2);
    expect(grid.rows.next).toBe("o5");
  });

  it("pages with a keyset cursor to the end, and the differing rows are the ones the page's own script marks", () => {
    let cursor: string | null = null;
    let total = 0;
    do {
      const page = designGrid(q(`file=${CORE_FILE}&swimlanes=${SIX}&only_differing=true&limit=500${cursor === null ? "" : `&cursor=${cursor}`}`));
      total += page.rows.items.length;
      cursor = page.rows.next;
    } while (cursor !== null);
    expect(total).toBe(designRows().filter((r) => r.differ).length);
  });

  it("filters to a channel and keeps the shared settings", () => {
    const grid = designGrid(q(`file=${CORE_FILE}&swimlanes=${SIX}&channel=atm&limit=500&cursor=o430`));
    const paths = grid.rows.items.map((r) => r.path);
    expect(paths.some((p) => p.startsWith("channels.atm."))).toBe(true);
    expect(paths.some((p) => p.startsWith("channels.atm-iso."))).toBe(false);
    expect(paths.some((p) => p.startsWith("core."))).toBe(true);
  });
});

describe("the comparison behind the drawer", () => {
  it("has a hunk for every setting whose two cells differ, found by the lines of the cells", () => {
    const grid = designGrid(q(`file=${CORE_FILE}&swimlanes=sit,sitb&only_differing=true&limit=500`));
    const comparison = designCompare(q(`left=effective:sit&right=effective:sitb&file=${CORE_FILE}`));
    const row = grid.rows.items.find((r) => rowFacts(r.cells).differing.size > 0 && r.cells[0]?.value !== null && r.cells[1]?.value !== null && r.cells[0]?.value?.text !== r.cells[1]?.value?.text);
    expect(row).toBeDefined();
    if (row === undefined) return;
    const near = hunksAround(comparison.hunks, row.cells[0]?.location?.lines ?? null, row.cells[1]?.location?.lines ?? null);
    expect(near.length).toBeGreaterThan(0);
    expect(near.length).toBeLessThanOrEqual(2);
    expect(settingSummary(row.path, comparison, "sit", "sitb")).toEqual({ text: "Value differs", same: false });
    const changed = near.flatMap((h) => h.lines).filter((l) => l.kind !== "context");
    expect(changed.map((l) => l.text).join("\n")).toContain(row.cells[0]?.value?.text ?? "");
  });

  it("does not overlap two hunks: a hunk ends before the next begins, in both files", () => {
    const comparison = designCompare(q(`left=effective:sitb&right=effective:sitc&file=${CORE_FILE}`));
    expect(comparison.hunks.length).toBeGreaterThan(50);
    for (let i = 1; i < comparison.hunks.length; i++) {
      const before = comparison.hunks[i - 1];
      const hunk = comparison.hunks[i];
      expect(hunk && before && hunk.old_start >= before.old_start + before.old_lines).toBe(true);
      expect(hunk && before && hunk.new_start >= before.new_start + before.new_lines).toBe(true);
    }
  });
});

describe("the whole-swimlane comparison", () => {
  it("lists the thirteen files for two swimlanes, fewer for a branch or a tag, and counts the directories it skipped", () => {
    expect(designTree(q("left=effective:sitb&right=effective:sitc&limit=500")).entries.items).toHaveLength(13);
    expect(designTree(q("left=nfs:sitb&right=git:tenant@sit7&limit=500")).entries.items).toHaveLength(4);
    expect(designTree(q("left=nfs:sitb&right=git:base@v2.3.1&limit=500")).entries.items).toHaveLength(6);
    expect(designTree(q("left=effective:sitb&right=effective:sitc")).skipped_identical_subtrees).toBe(38);
  });

  it("has a binary file, a file only on the left and a file only on the right", () => {
    expect(designCompare(q("left=effective:sitb&right=effective:sitc&file=resources/receipt-logo.jpeg")).class).toBe("binary");
    expect(designCompare(q("left=effective:sitb&right=effective:sitc&file=tx-infinity-api/tx-infinity-limits.yml")).hunks[0]?.new_lines).toBe(0);
    expect(designCompare(q("left=effective:sitb&right=effective:sitc&file=resources/receipt-footer.txt")).hunks[0]?.old_lines).toBe(0);
  });

  it("is identical when the scenario says so", () => {
    expect(designTree(q("left=effective:sitb&right=effective:sitc"), true).entries.items).toEqual([]);
    expect((bodyOf("design-compare-identical", "/api/v1/compare/tree", "left=effective:sitb&right=effective:sitc") as { skipped_identical_subtrees: number }).skipped_identical_subtrees).toBe(43);
  });
});

describe("the scenarios", () => {
  it("are named for the slice and the state", () => {
    expect(scenarios.map((s) => s.id).sort()).toEqual(["design-compare-default", "design-compare-empty", "design-compare-error", "design-compare-identical", "design-compare-large", "design-compare-offline", "design-compare-toomany"]);
  });

  it("has an offline agent only in the offline scenario, 14 minutes before the clock", () => {
    const lanes = (id: string) => (scenarios.find((s) => s.id === id)?.responses.find((r) => r.path === "/api/v1/swimlanes")?.body as { items: Array<{ id: string; agent: string; last_scan_at: number }> }).items;
    expect(lanes("design-compare-default").every((l) => l.agent === "connected")).toBe(true);
    const presitb = lanes("design-compare-offline").find((l) => l.id === "presitb");
    expect(presitb?.agent).toBe("disconnected");
    expect(1_760_000_000_000 - (presitb?.last_scan_at ?? 0)).toBe(14 * 60_000);
  });

  it("lists 43 swimlanes in the too-many scenario", () => {
    expect(lanes43()).toBe(43);
  });

  it("serves the large grid in the pages the real screen asks for: 50 rows, then 500, eleven pages for 5,000 rows of 40 swimlanes", () => {
    let cursor: string | null = null;
    const sizes: number[] = [];
    do {
      const limit = cursor === null ? 50 : 500;
      const page = bodyOf("design-compare-large", "/api/v1/grid", `file=a.yml&swimlanes=x&limit=${limit}${cursor === null ? "" : `&cursor=${cursor}`}`) as { swimlanes: string[]; rows: { items: Array<{ cells: unknown[] }>; next: string | null } };
      expect(page.swimlanes).toHaveLength(40);
      expect(page.rows.items[0]?.cells).toHaveLength(40);
      sizes.push(page.rows.items.length);
      cursor = page.rows.next;
    } while (cursor !== null);
    expect(sizes.reduce((a, b) => a + b, 0)).toBe(GRID_ROWS);
    expect(sizes).toHaveLength(11);
    expect(sizes[0]).toBe(50);
  });

  it("answers an error everywhere in the error scenario, with a request id", () => {
    const grid = scenarios.find((s) => s.id === "design-compare-error")?.responses.find((r) => r.path === "/api/v1/grid");
    expect(grid?.status).toBe(500);
    expect((grid?.body as { request_id: string }).request_id).toMatch(/^req-/);
  });
});

function lanes43(): number {
  const response = scenarios.find((s) => s.id === "design-compare-toomany")?.responses.find((r) => r.path === "/api/v1/swimlanes");
  return (response?.body as { items: unknown[] }).items.length;
}
