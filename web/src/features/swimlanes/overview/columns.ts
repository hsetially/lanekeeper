// The columns of the swimlane list that a person can hide, in the order the design draws them. The Swimlane column is always on.
// The ids are what `lib/prefs` keeps (layout only, never data), so they are lower case words with no spaces.

export const COLUMN_IDS = ["swimlane", "project", "branch", "base", "agent", "drift", "pickup", "findings", "baseline", "last"] as const;
export type ColumnId = (typeof COLUMN_IDS)[number];

export const COLUMN_LABEL: Record<ColumnId, string> = {
  swimlane: "Swimlane",
  project: "Project",
  branch: "Branch",
  base: "Base version",
  agent: "Agent",
  drift: "Drift",
  pickup: "Pickup",
  findings: "Findings",
  baseline: "Baseline",
  last: "Last scan",
};

const KNOWN: ReadonlySet<string> = new Set(COLUMN_IDS);

/** The columns to draw, from what was remembered: the known ones, in the design's order, with the Swimlane column always there. */
export function visibleColumns(remembered: readonly string[] | undefined): ColumnId[] {
  if (remembered === undefined) return [...COLUMN_IDS];
  const wanted = new Set(remembered.filter((id) => KNOWN.has(id)));
  wanted.add("swimlane");
  return COLUMN_IDS.filter((id) => wanted.has(id));
}
