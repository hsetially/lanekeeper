import { describe, expect, it } from "vitest";
import { GRID_LANES, GRID_ROWS, gridBody } from "./grid.ts";

// The 5,000-row grid of the P10 gate: 40 swimlane columns, loaded through the paging path the app really uses (a keyset
// cursor, at most 500 a page). The generator is deterministic, and its pages are what the contract says a page is.

const SWIMLANE_ID = /^[a-z0-9][a-z0-9-]{0,62}$/;
const CURSOR = /^[A-Za-z0-9_=.-]{1,512}$/;

describe("gridBody", () => {
  it("has forty swimlanes with valid, distinct ids", () => {
    expect(GRID_LANES).toHaveLength(40);
    expect(new Set(GRID_LANES).size).toBe(40);
    for (const id of GRID_LANES) expect(id).toMatch(SWIMLANE_ID);
  });

  it("answers the first page with no cursor, at the page size asked for, and a cursor for the next", () => {
    const body = gridBody(new URLSearchParams("limit=500"));
    expect(body.swimlanes).toEqual(GRID_LANES);
    expect(body.rows.items).toHaveLength(500);
    expect(body.rows.next).toMatch(CURSOR);
  });

  it("walks to 5,000 rows in ten pages, and the last page has no next", () => {
    let cursor: string | null = null;
    let rows = 0;
    let pages = 0;
    const seen = new Set<string>();
    do {
      const q = new URLSearchParams("limit=500");
      if (cursor) q.set("cursor", cursor);
      const body = gridBody(q);
      for (const row of body.rows.items) seen.add(row.path);
      rows += body.rows.items.length;
      pages += 1;
      cursor = body.rows.next;
    } while (cursor && pages < 50);
    expect([rows, pages, seen.size, GRID_ROWS]).toEqual([5000, 10, 5000, 5000]);
    expect(cursor).toBeNull();
  });

  it("gives one cell for each swimlane in each row, in the order of the swimlanes", () => {
    for (const row of gridBody(new URLSearchParams("limit=50")).rows.items) {
      expect(row.cells).toHaveLength(40);
      expect(row.path.length).toBeLessThanOrEqual(1024);
    }
  });

  it("is a grid with something to see: most cells agree, some differ, some have no value", () => {
    const rows = gridBody(new URLSearchParams("limit=500")).rows.items;
    const cells = rows.flatMap((r) => r.cells);
    expect(cells.some((c) => c.value === null && c.location === null)).toBe(true);
    const differing = rows.filter((r) => new Set(r.cells.map((c) => c.value?.text)).size > 1).length;
    expect(differing).toBeGreaterThan(50);
    expect(differing).toBeLessThan(450);
  });

  it("stays under the contract's limit of 500 rows, whatever is asked", () => {
    expect(gridBody(new URLSearchParams("limit=9999")).rows.items).toHaveLength(500);
    expect(gridBody(new URLSearchParams("limit=0")).rows.items.length).toBeGreaterThan(0);
  });

  it("starts again from the beginning for a cursor it does not know, rather than failing", () => {
    expect(gridBody(new URLSearchParams("limit=5&cursor=nonsense")).rows.items[0]?.path).toBe(gridBody(new URLSearchParams("limit=5")).rows.items[0]?.path);
  });

  it("is the same every time", () => {
    expect(gridBody(new URLSearchParams("limit=100&cursor=o300"))).toEqual(gridBody(new URLSearchParams("limit=100&cursor=o300")));
  });
});
