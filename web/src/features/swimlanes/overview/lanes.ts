import { AgentStatus, BaselineState, type SwimlaneSummary } from "@/api/generated/model";
import type { SortState } from "@/components/app/virtual/DataTable";
import { agentView } from "../agent";
import { driftChips } from "../format";

// What the swimlane list does with the swimlanes it was given: filter them, sort them and say how many there are. The server
// pages the list but has no filter and no sort, and the whole list is loaded for a screen that shows it (at most a few
// hundred swimlanes), so this is done here, on the loaded list, and never on a part of it that the screen did not say it had.
// All of it is plain text and numbers: a filter is never a pattern (S11), and nothing here decides a fact about a swimlane
// (the drift and the findings it counts come from the server).

export type AgentFilter = "online" | "offline";

export type LaneFilter = {
  /** Plain text, matched anywhere in the id, the name, the project, the tier and the tenants. */
  readonly q: string;
  /** A tier as it is written in the address (`sit`, `presit`), or null for all. */
  readonly tier: string | null;
  /** Only the swimlanes with files in a state that needs a look. */
  readonly hasDrift: boolean;
  readonly agent: AgentFilter | null;
};

const matchesText = (lane: SwimlaneSummary, needle: string): boolean => {
  if (needle === "") return true;
  const haystack = [lane.id, lane.display_name, lane.project ?? "", lane.tier ?? "", ...lane.tenants];
  return haystack.some((field) => field.toLowerCase().includes(needle));
};

export function filterLanes(lanes: readonly SwimlaneSummary[], filter: LaneFilter): SwimlaneSummary[] {
  const needle = filter.q.trim().toLowerCase();
  const tier = filter.tier?.toLowerCase() ?? null;
  return lanes.filter((lane) => {
    if (!matchesText(lane, needle)) return false;
    if (tier !== null && lane.tier?.toLowerCase() !== tier) return false;
    if (filter.hasDrift && driftChips(lane.drift_counts).length === 0) return false;
    if (filter.agent !== null && agentView(lane.agent).connected !== (filter.agent === "online")) return false;
    return true;
  });
}

const driftTotal = (lane: SwimlaneSummary): number => driftChips(lane.drift_counts).reduce((sum, chip) => sum + chip.count, 0);
const byName = (a: SwimlaneSummary, b: SwimlaneSummary): number => a.display_name.localeCompare(b.display_name) || a.id.localeCompare(b.id);

/** A new array, in the order of the column. Ties are broken by name. A column it does not know leaves the order as given. */
export function sortLanes(lanes: readonly SwimlaneSummary[], sort: SortState): SwimlaneSummary[] {
  const sign = sort.direction === "asc" ? 1 : -1;
  const out = [...lanes];
  switch (sort.column) {
    case "swimlane":
      return out.sort((a, b) => sign * byName(a, b));
    case "findings":
      return out.sort((a, b) => sign * (a.finding_count - b.finding_count) || byName(a, b));
    case "drift":
      return out.sort((a, b) => sign * (driftTotal(a) - driftTotal(b)) || byName(a, b));
    case "last":
      // A swimlane that was never scanned has nothing to compare: it goes last in either direction.
      return out.sort((a, b) => {
        if (a.last_scan_at === null && b.last_scan_at === null) return byName(a, b);
        if (a.last_scan_at === null) return 1;
        if (b.last_scan_at === null) return -1;
        return sign * (a.last_scan_at - b.last_scan_at) || byName(a, b);
      });
    default:
      return out;
  }
}

/** The tiers that exist, once each, in upper case, in the order they first appear. */
export function tiersOf(lanes: readonly SwimlaneSummary[]): string[] {
  const seen = new Set<string>();
  for (const lane of lanes) {
    const tier = lane.tier?.trim();
    if (tier) seen.add(tier.toUpperCase());
  }
  return [...seen];
}

/** "6 swimlanes · 4 of 6 agents online · 1 needs adoption": the line under the title. */
export function facts(lanes: readonly SwimlaneSummary[]): string {
  const online = lanes.filter((lane) => lane.agent === AgentStatus.connected).length;
  const open = lanes.filter((lane) => lane.baseline?.state === BaselineState.unconfirmed || lane.baseline?.state === BaselineState.in_progress).length;
  return `${lanes.length} ${lanes.length === 1 ? "swimlane" : "swimlanes"} · ${online} of ${lanes.length} agents online · ${open} ${open === 1 ? "needs" : "need"} adoption`;
}
