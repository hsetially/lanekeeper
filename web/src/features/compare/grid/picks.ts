import type { GridRow } from "@/api/generated/model";
import { cellKind, rowFacts } from "../model";

// Which two swimlanes are being compared at one setting. A pick is a row and up to two columns (A and B). The pointer picks
// by clicking cells; the keyboard picks by pressing Enter on a row, which opens it with a pair worth looking at.

export type Pick = { row: GridRow; a: number | null; b: number | null };

/** The pick after a click on the cell of `lane` in `row`, and whether the diff opens (the second cell of a row does that). */
export function pickCell(current: Pick | null, row: GridRow, lane: number): { pick: Pick; open: boolean } {
  if (current === null || current.row.path !== row.path) return { pick: { row, a: lane, b: null }, open: false };
  const { a, b } = current;
  if (lane === a) return { pick: { row, a: b, b: null }, open: false };
  if (lane === b) return { pick: { row, a, b: null }, open: false };
  if (a === null) return { pick: { row, a: lane, b }, open: b !== null };
  if (b === null) return { pick: { row, a, b: lane }, open: true };
  return { pick: { row, a: lane, b: null }, open: false };
}

/**
 * The pair a row opens with when the person presses Enter on it: A is the first swimlane with the value most of the row has,
 * B the first that stands apart. A row whose swimlanes agree opens with the first two. A hidden value is not one that agrees
 * (the page cannot tell), so it is never A.
 */
export function defaultPick(row: GridRow, lanes: number): Pick {
  const count = Math.min(lanes, row.cells.length);
  if (count === 0) return { row, a: null, b: null };
  const { differing } = rowFacts(row.cells);
  let b: number | null = null;
  for (const index of [...differing].sort((x, y) => x - y)) {
    if (index < count) {
      b = index;
      break;
    }
  }
  if (b !== null) {
    const a = row.cells.findIndex((cell, index) => index < count && !differing.has(index) && cellKind(cell) !== "hidden");
    return { row, a: a < 0 ? 0 : a, b };
  }
  return { row, a: 0, b: count > 1 ? 1 : null };
}
