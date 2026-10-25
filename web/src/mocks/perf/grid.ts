// The comparison grid of the P10 gate: 40 swimlane columns and 5,000 rows, served the way the hub serves a page of the grid
// (a keyset cursor, at most 500 rows a page). Deterministic. Loaded by Node as well as by Vite: relative imports with the
// `.ts` extension and `import type` for types.

import type { Grid, GridCell, GridRow } from "../../api/generated/model/index.ts";

export const GRID_ROWS = 5000;
export const GRID_LANES: readonly string[] = Array.from({ length: 40 }, (_, i) => `swimlane-${String(i + 1).padStart(2, "0")}`);
const MAX_PAGE = 500;
const GROUPS = ["datasource", "cache", "server", "logging", "retry", "security", "feature", "metrics"];

function cell(row: number, lane: number): GridCell {
  // Most lanes agree. Every seventh row differs in a few lanes, and some settings are missing from some lanes.
  const differs = row % 7 === 0 && lane % 5 === 0;
  const missing = row % 11 === 0 && lane % 13 === 0;
  if (missing) return { value: null, location: null };
  const base = row % 3 === 0 ? String(10 + (row % 50)) : row % 3 === 1 ? "true" : `value-${row % 97}`;
  const text = differs ? `${base}-${lane}` : base;
  const ty = row % 3 === 0 ? "int" : row % 3 === 1 ? "bool" : "string";
  return { value: { text, ty, flag: null }, location: { path: pathOf(row), lines: { start: 3 + (row % 400), end: 3 + (row % 400) } } };
}

function pathOf(row: number): string {
  return `app.${GROUPS[row % GROUPS.length]}.group${Math.floor(row / GROUPS.length) % 50}.setting-${row}`;
}

function row(index: number): GridRow {
  return { path: pathOf(index), cells: GRID_LANES.map((_, lane) => cell(index, lane)) };
}

const cursorFor = (offset: number): string => `o${offset}`;
function offsetOf(cursor: string | null): number {
  const match = cursor === null ? null : /^o(\d{1,9})$/.exec(cursor);
  const offset = match?.[1] === undefined ? 0 : Number(match[1]);
  return offset < GRID_ROWS ? offset : 0;
}

/** One page of the grid, from the query of `GET /api/v1/grid` (`limit` and `cursor`; the other parameters are not used). */
export function gridBody(query: URLSearchParams): Grid {
  const asked = Number(query.get("limit") ?? MAX_PAGE);
  const limit = Number.isInteger(asked) && asked >= 1 ? Math.min(asked, MAX_PAGE) : asked >= 1 ? MAX_PAGE : 1;
  const from = offsetOf(query.get("cursor"));
  const to = Math.min(GRID_ROWS, from + limit);
  const items = Array.from({ length: to - from }, (_, i) => row(from + i));
  return { swimlanes: [...GRID_LANES], rows: { items, next: to < GRID_ROWS ? cursorFor(to) : null } };
}
