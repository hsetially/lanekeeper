import { useMemo } from "react";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, type QuerySource } from "@/lib/query/use-query-state";
import type { SwimlaneSummary } from "@/api/generated/model";

// The swimlanes a search can be narrowed to, from the list the sidebar and the switcher already use (one cache entry, kept fresh
// by the live events). A swimlane that is not adopted yet has no tenants.

export type LaneInfo = {
  id: string;
  name: string;
  tier: string | null;
  tenants: readonly string[];
  /** The agent's connection, for the screens that dim what an offline agent last reported. */
  agent: "connected" | "disconnected" | "never_seen";
  lastScanAt: number | null;
};

export function useLanes(): { lanes: LaneInfo[]; ready: boolean; failed: boolean; source: QuerySource<SwimlaneSummary[]> } {
  const pages = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const items = pages.items;
  const lanes = useMemo<LaneInfo[]>(
    () => items.map((lane) => ({ id: lane.id, name: lane.display_name, tier: lane.tier ?? null, tenants: lane.tenants, agent: lane.agent, lastScanAt: lane.last_scan_at })),
    [items],
  );
  return { lanes, ready: pages.status === "success", failed: pages.status === "error", source: cursorPagesSource(pages) };
}
