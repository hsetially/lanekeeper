import { describe, expect, it } from "vitest";
import { CELL, MAX_CLUSTERS, NAME_LIMIT, clusterCells, describeElement, formatReport, trimName } from "../e2e/visual/diff-report.mjs";

// A tier-1 comparison that goes over its limit used to say only "8 of 1134 pixels differ". The first CI run of the baseline
// bootstrap failed two of them by a hair (0.705% and 0.558% against 0.5%) on a Chromium that is not the one the margins were
// measured on, and nothing in the log said WHERE. e2e/visual/diff-report.mjs turns the picture of the difference into a short text:
// the pixels that differ are grouped into clusters (cells of 16 px merged by adjacency), the largest first, each with its box, its
// share, the colour difference, what the two renderings look like there and the element under it. These are the parts of it
// that need no browser; the part that reads the pictures is e2e/visual/diff-report.spec.ts.

/** One cell of the grid with `count` differing pixels in the box (x0,y0)-(x1,y1). */
const cell = (cx, cy, count, box = {}, sum = count * 10) => ({ cx, cy, count, sum, x0: cx * CELL, y0: cy * CELL, x1: cx * CELL + CELL - 1, y1: cy * CELL + CELL - 1, ...box });

describe("clusterCells", () => {
  it("finds nothing where nothing differs", () => {
    expect(clusterCells([])).toEqual({ clusters: [], rest: { clusters: 0, pixels: 0 } });
  });

  it("names the box and the count of a block of 6 by 4 pixels", () => {
    // The block at x 40..45, y 20..23 lies in the cell (2, 1) of 16 px: 24 differing pixels.
    const { clusters, rest } = clusterCells([cell(2, 1, 24, { x0: 40, y0: 20, x1: 45, y1: 23 }, 24 * 90)]);
    expect(clusters).toEqual([{ x0: 40, y0: 20, x1: 45, y1: 23, count: 24, sum: 24 * 90, cells: 1 }]);
    expect(rest).toEqual({ clusters: 0, pixels: 0 });
  });

  it("merges cells that touch, along a side or by a corner, and keeps the exact box of the pixels", () => {
    // A block that crosses the line between two cells (x 46..51), and a cell below and to the right of it, touching by a corner.
    const { clusters } = clusterCells([
      cell(2, 1, 12, { x0: 46, y0: 20, x1: 47, y1: 23 }),
      cell(3, 1, 12, { x0: 48, y0: 20, x1: 51, y1: 23 }),
      cell(4, 2, 3, { x0: 64, y0: 32, x1: 65, y1: 32 }),
    ]);
    expect(clusters).toHaveLength(1);
    expect(clusters[0]).toMatchObject({ x0: 46, y0: 20, x1: 65, y1: 32, count: 27, cells: 3 });
  });

  it("keeps cells that are a whole cell apart as clusters of their own", () => {
    const { clusters } = clusterCells([cell(0, 0, 5), cell(2, 0, 7)]);
    expect(clusters.map((c) => c.count)).toEqual([7, 5]);
  });

  it("puts the largest first, breaks a tie by position, and keeps eight with the rest counted", () => {
    const cells = Array.from({ length: 12 }, (_, i) => cell(i * 2, 0, i === 3 || i === 4 ? 6 : i + 1));
    const { clusters, rest } = clusterCells(cells);
    expect(MAX_CLUSTERS).toBe(8);
    expect(clusters).toHaveLength(8);
    const counts = clusters.map((c) => c.count);
    expect(counts).toEqual([...counts].sort((a, b) => b - a));
    // The two clusters of 6 are in the list in the order of the page: the left one first.
    expect(clusters.filter((c) => c.count === 6).map((c) => c.x0)).toEqual([6 * CELL, 8 * CELL]);
    // What was left out is counted, not lost: 12 clusters, 8 shown.
    expect(rest.clusters).toBe(4);
    expect(rest.pixels).toBe(cells.reduce((n, c) => n + c.count, 0) - counts.reduce((n, c) => n + c, 0));
  });

  it("does not trust a limit that is not a positive whole number", () => {
    expect(clusterCells([cell(0, 0, 1), cell(5, 5, 1)], 0).clusters).toHaveLength(0);
    expect(clusterCells([cell(0, 0, 1), cell(5, 5, 1)], 1).clusters).toHaveLength(1);
    expect(clusterCells([cell(0, 0, 1), cell(5, 5, 1)], 1).rest.clusters).toBe(1);
  });
});

describe("trimName", () => {
  it("collapses white space, drops control characters and keeps at most 60 characters", () => {
    expect(NAME_LIMIT).toBe(60);
    expect(trimName("  Binary \n\t file\u0000 ")).toBe("Binary file");
    const long = trimName("x".repeat(500));
    expect(long.length).toBeLessThanOrEqual(60);
    expect(long.endsWith("...")).toBe(true);
  });

  it("returns an empty string for nothing", () => {
    expect(trimName("")).toBe("");
    expect(trimName(undefined)).toBe("");
  });
});

describe("describeElement", () => {
  it("says the tag, the role, the name trimmed, the slot and the box", () => {
    const text = describeElement({ tag: "span", role: "meter", name: "Attribution confidence: High", slot: "attribution-meter", cls: "", x: 12, y: 3.5, w: 18, h: 6 });
    expect(text).toBe('<span> role=meter "Attribution confidence: High" slot=attribution-meter 18x6@(12,3.5)');
  });

  it("falls back on the first classes when there is no slot (the design has none), and never prints more than 60 characters of a name", () => {
    const text = describeElement({ tag: "div", role: "", name: "n".repeat(300), slot: "", cls: "bd-pr bd-x bd-y", x: 0, y: 0, w: 10, h: 10 });
    expect(text).toContain(".bd-pr.bd-x");
    expect(text).not.toContain(".bd-y");
    expect(text).not.toContain("n".repeat(61));
  });

  it("says that there is nothing under the point", () => {
    expect(describeElement(null)).toBe("nothing at that point");
  });
});

describe("formatReport", () => {
  const base = {
    label: "frame 1m (light)",
    width: 1440,
    height: 900,
    compared: 255104,
    differing: 24,
    ratio: 24 / 255104,
    limit: 0.005,
    found: 24,
    rest: { clusters: 0, pixels: 0 },
    notes: [],
  };
  const block = {
    x0: 40,
    y0: 20,
    x1: 45,
    y1: 23,
    count: 24,
    sum: 24 * 90,
    cells: 1,
    measure: { ref: { w: 6, h: 4, x: 0, y: 0 }, app: null, shift: { dx: 1, dy: 0, before: 24, after: 0 }, worst: [{ x: 40, y: 20, ref: "#cc0000", app: "#ffffff" }] },
    app: { tag: "div", role: "", name: "", slot: "", cls: "", x: 41, y: 20, w: 6, h: 4 },
    design: null,
  };

  it("names the position, the size, the count and the share of a block, and the colour difference", () => {
    const text = formatReport({ ...base, clusters: [block] });
    expect(text).toContain("x 40..45 y 20..23 (6x4)");
    expect(text).toContain("24 px");
    expect(text).toContain("100.0%");
    expect(text).toMatch(/mean abs diff 90\/255/);
    expect(text).toContain("frame 1m (light)");
  });

  it("says whether the two renderings are the same size, and the shift that would make them agree", () => {
    const same = formatReport({ ...base, clusters: [{ ...block, measure: { ...block.measure, ref: { w: 6, h: 4, x: 0, y: 0 }, app: { w: 6, h: 4, x: 1, y: 0 } } }] });
    expect(same).toContain("ink 6x4 | 6x4: same size");
    expect(same).toContain("(+1,0)");
    expect(same).toContain("24 -> 0");
    const bigger = formatReport({ ...base, clusters: [{ ...block, measure: { ...block.measure, ref: { w: 6, h: 4, x: 0, y: 0 }, app: { w: 7, h: 4, x: 0, y: 0 } } }] });
    expect(bigger).toContain("ink 6x4 | 7x4: sizes differ");
  });

  it("says that there is no difference when none of the pixels differ", () => {
    const text = formatReport({ ...base, differing: 0, found: 0, ratio: 0, clusters: [] });
    expect(text).toContain("no differing pixels");
    expect(text).not.toContain("#1");
  });

  it("says what the design side is when it could not be asked, once", () => {
    const text = formatReport({ ...base, clusters: [block], notes: ["design page: not queried (the page is closed once its pieces are photographed)"] });
    expect(text.match(/not queried/g)).toHaveLength(1);
  });

  it("compares the boxes of the two elements when both were asked", () => {
    const design = { tag: "div", role: "", name: "", slot: "", cls: "meter", x: 41, y: 20, w: 6, h: 4 };
    const same = formatReport({ ...base, clusters: [{ ...block, design }] });
    expect(same).toContain("design <div> .meter 6x4@(41,20)");
    expect(same).toMatch(/box is the same size as the app's/);
    const other = formatReport({ ...base, clusters: [{ ...block, design: { ...design, w: 7 } }] });
    expect(other).toMatch(/box differs from the app's \(7x4 against 6x4\)/);
  });

  it("counts the differences that the clusters leave out, and the ones the picture does not hold (the aligned parts)", () => {
    const text = formatReport({ ...base, differing: 40, found: 30, clusters: [block], rest: { clusters: 3, pixels: 6 } });
    expect(text).toContain("3 more clusters, 6 px");
    expect(text).toContain("10 px more are in the aligned parts");
  });

  it("stays under 40 lines with eight clusters, each with an element on both sides, and does not print a long name", () => {
    const cluster = (i) => ({
      ...block,
      x0: i * 100,
      x1: i * 100 + 5,
      count: 50 - i,
      measure: { ref: { w: 6, h: 4, x: 0, y: 0 }, app: { w: 6, h: 4, x: 1, y: 0 }, shift: { dx: 1, dy: 0, before: 24, after: 2 }, worst: [{ x: 1, y: 1, ref: "#000000", app: "#ffffff" }, { x: 2, y: 2, ref: "#000000", app: "#ffffff" }] },
      app: { ...block.app, name: "secret ".repeat(40) },
      design: { ...block.app, name: "secret ".repeat(40) },
    });
    const text = formatReport({ ...base, found: 400, differing: 400, clusters: Array.from({ length: 8 }, (_, i) => cluster(i)), rest: { clusters: 5, pixels: 20 } });
    expect(text.split("\n").length).toBeLessThanOrEqual(40);
    expect(text).not.toContain("secret ".repeat(10));
  });
});
