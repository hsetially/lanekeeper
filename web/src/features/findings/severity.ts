import type { Finding } from "@/api/generated/model";
import type { SortState } from "@/components/app/virtual/DataTable";
import { SEVERITIES, SEVERITY_LABEL, severityRank } from "@/components/attribution/severity";
import { KINDS } from "./kinds";

// The levels, their words and their order are the shared severity module's (src/components/attribution/severity.ts), the one
// definition every screen uses.
export { SEVERITIES, SEVERITY_LABEL, severityRank };

// How the Findings screen orders what the server found. DISPLAY ONLY: the severity, the kind and the swimlane of a finding are
// the server's (the registry rates a finding by the engine's rules and the admin's, D103), and the browser never rates or
// re-rates one. The server lists newest first by id and cannot be asked for another order, so the order a person chooses applies
// to the findings that are loaded, and the screen says so while more are on their way.

/** The order when no column is chosen: the most severe first. */
export const DEFAULT_SORT: SortState = { column: "severity", direction: "desc" };

const UNKNOWN_LANE = Number.MAX_SAFE_INTEGER;
const laneAt = (order: ReadonlyMap<string, number>, id: string): number => order.get(id) ?? UNKNOWN_LANE;

type Compare = (a: Finding, b: Finding) => number;

function bySeverity(a: Finding, b: Finding): number {
  return severityRank(a.severity) - severityRank(b.severity);
}
function byCheck(a: Finding, b: Finding): number {
  return KINDS.indexOf(a.kind) - KINDS.indexOf(b.kind);
}
/** A finding with no file comes after those that have one. */
function byPath(a: Finding, b: Finding): number {
  return Number(a.path === null) - Number(b.path === null) || (a.path ?? "").localeCompare(b.path ?? "");
}

/** Most severe first, then the swimlane in the swimlane list's order, then the check, then the file, then the newest. */
function byUrgency(lanes: ReadonlyMap<string, number>): Compare {
  return (a, b) => bySeverity(a, b) || laneAt(lanes, a.swimlane) - laneAt(lanes, b.swimlane) || byCheck(a, b) || byPath(a, b) || b.id - a.id;
}

/**
 * The comparison for a chosen column (`null`, or a column this screen does not sort by, is the default order). Rows that are equal
 * in the chosen column keep the default order, so the list never flickers between two refreshes.
 */
export function compareFindings(sort: SortState | null, lanes: ReadonlyMap<string, number>): Compare {
  const fallback = byUrgency(lanes);
  const dir = sort?.direction === "asc" ? 1 : -1;
  switch (sort?.column) {
    case "severity":
      // Ascending is from low up to critical, so "descending" puts the most severe (the lowest rank) first.
      return (a, b) => dir * -bySeverity(a, b) || fallback(a, b);
    case "check":
      return (a, b) => dir * byCheck(a, b) || fallback(a, b);
    case "swimlane":
      return (a, b) => dir * (laneAt(lanes, a.swimlane) - laneAt(lanes, b.swimlane)) || fallback(a, b);
    case "file":
      return (a, b) => dir * byPath(a, b) || fallback(a, b);
    default:
      return fallback;
  }
}

/** A new array in the chosen order. */
export function sortFindings(findings: readonly Finding[], sort: SortState | null, lanes: ReadonlyMap<string, number>): Finding[] {
  return [...findings].sort(compareFindings(sort, lanes));
}
