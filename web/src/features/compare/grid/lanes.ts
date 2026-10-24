import type { SwimlaneSummary } from "@/api/generated/model";
import { visibleText } from "@/components/app/safe-text";
import { agentView, offlineSince, type AgentView } from "@/features/swimlanes/agent";
import { formatClock } from "@/lib/time";

// A swimlane as a column of the grid: what the header says about it. The contract's summary has the tier, the tenants, the
// base version and the agent; it does not say which file the swimlane reads for the chosen logical file (a tenant file beside
// the base file, or the base file), so the header names the tenants and the base version and never claims that.

export type LaneColumn = {
  readonly id: string;
  /** The list has this swimlane. A swimlane the person cannot see, or that does not exist, is a column by id only. */
  readonly known: boolean;
  readonly displayName: string;
  readonly tier: string | null;
  readonly tenants: readonly string[];
  readonly baseTag: string | null;
  /** Null when the list is not known: no dot is drawn, because a dot must never say "online" on a guess. */
  readonly agent: AgentView | null;
  /** When an agent that is offline was last heard from (the time of its last scan). */
  readonly asOf: number | null;
};

export function laneColumn(id: string, summary: SwimlaneSummary | undefined): LaneColumn {
  if (summary === undefined) return { id, known: false, displayName: id, tier: null, tenants: [], baseTag: null, agent: null, asOf: null };
  return {
    id,
    known: true,
    displayName: summary.display_name,
    tier: summary.tier ?? null,
    tenants: summary.tenants,
    baseTag: summary.base_version?.tag ?? null,
    agent: agentView(summary.agent),
    asOf: offlineSince(summary.agent, summary.last_scan_at),
  };
}

export function laneColumns(ids: readonly string[], summaries: readonly SwimlaneSummary[]): LaneColumn[] {
  const byId = new Map(summaries.map((s) => [s.id, s]));
  return ids.map((id) => laneColumn(id, byId.get(id)));
}

/** Whether the agent has stopped reporting, so that the column shows the last scan and not the swimlane now. */
export const isOffline = (lane: LaneColumn): boolean => lane.agent !== null && !lane.agent.connected;

/** The tooltip of a column header: the tenants, the base version and, for an offline agent, how old the data is. */
export function laneTitle(lane: LaneColumn): string {
  if (!lane.known) return `${visibleText(lane.id)}: not in your swimlane list`;
  const parts = [lane.displayName !== lane.id ? `${visibleText(lane.id)} (${visibleText(lane.displayName)})` : visibleText(lane.id)];
  parts.push(lane.tenants.length === 0 ? "no tenants" : `tenants ${lane.tenants.map(visibleText).join(", ")}`);
  if (lane.baseTag !== null) parts.push(`base ${visibleText(lane.baseTag)}`);
  if (isOffline(lane)) parts.push(lane.asOf === null ? "agent offline, no scan on record" : `agent offline, files as of ${formatClock(lane.asOf)}`);
  return parts.join(" · ");
}
