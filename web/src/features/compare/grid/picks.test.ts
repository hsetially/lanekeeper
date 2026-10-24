import { describe, expect, it } from "vitest";
import type { GridCell, GridRow } from "@/api/generated/model";
import { defaultPick, pickCell, type Pick } from "./picks";

const cell = (text: string | null): GridCell => (text === null ? { value: null, location: null } : { value: { text, ty: "string", flag: null }, location: null });
const row = (path: string, ...texts: Array<string | null>): GridRow => ({ path, cells: texts.map(cell) });
const R1 = row("a.b", "1", "1", "2", "1");
const R2 = row("a.c", "1", "1", "1", "1");

describe("picking cells with the pointer", () => {
  it("picks the first cell as A and does not open the diff", () => {
    expect(pickCell(null, R1, 2)).toEqual({ pick: { row: R1, a: 2, b: null }, open: false });
  });

  it("picks a second cell of the same row as B and opens the diff", () => {
    const first: Pick = { row: R1, a: 0, b: null };
    expect(pickCell(first, R1, 2)).toEqual({ pick: { row: R1, a: 0, b: 2 }, open: true });
  });

  it("starts again when the second cell is in another row", () => {
    const first: Pick = { row: R1, a: 0, b: null };
    expect(pickCell(first, R2, 1)).toEqual({ pick: { row: R2, a: 1, b: null }, open: false });
  });

  it("unpicks a cell that is clicked again, keeping the other as A", () => {
    expect(pickCell({ row: R1, a: 0, b: 2 }, R1, 0)).toEqual({ pick: { row: R1, a: 2, b: null }, open: false });
    expect(pickCell({ row: R1, a: 0, b: 2 }, R1, 2)).toEqual({ pick: { row: R1, a: 0, b: null }, open: false });
  });

  it("starts again from a third cell of the row when both are taken", () => {
    expect(pickCell({ row: R1, a: 0, b: 2 }, R1, 3)).toEqual({ pick: { row: R1, a: 3, b: null }, open: false });
  });

  it("fills A when only B is set (A was cleared in the list)", () => {
    expect(pickCell({ row: R1, a: null, b: 2 }, R1, 1)).toEqual({ pick: { row: R1, a: 1, b: 2 }, open: true });
  });
});

describe("the pair a row opens with", () => {
  it("is a swimlane that has the value most of the row has, and the first one that stands apart", () => {
    expect(defaultPick(R1, 4)).toEqual({ row: R1, a: 0, b: 2 });
  });

  it("is the first two swimlanes when they all agree", () => {
    expect(defaultPick(R2, 4)).toEqual({ row: R2, a: 0, b: 1 });
  });

  it("does not take a hidden value for one that agrees", () => {
    const hidden = row("s", "[REDACTED: 4 chars, k]", "x", "y", "x");
    expect(defaultPick(hidden, 4)).toEqual({ row: hidden, a: 1, b: 2 });
  });

  it("is only A when there is one swimlane, and nothing when there is none", () => {
    expect(defaultPick(row("a", "1"), 1)).toEqual({ row: row("a", "1"), a: 0, b: null });
    expect(defaultPick(row("a"), 0)).toEqual({ row: row("a"), a: null, b: null });
  });
});
