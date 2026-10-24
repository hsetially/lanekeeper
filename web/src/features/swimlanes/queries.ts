import { getSwimlane, listDrift, listSwimlanes } from "@/api/generated/endpoints/swimlanes/swimlanes";
import type { DriftState } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { swimlaneKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";

// Swimlanes: the list, one swimlane, and the drift list. Live events keep all of them fresh (file_observed,
// drift_changed, findings_changed, pickup_changed, pending_restart_changed, agent_status_changed, swimlane_changed and the sync
// windows each name the swimlane), so they are EVENT_FED: never stale by age, refetched when an event invalidates them, and
// marked stale by the banner while the stream is down.

/** Every swimlane the user may see. A list that is paged by cursor (`useCursorPages`). `summary.tenants` may be empty for an unadopted swimlane. */
export const swimlaneListQuery = () =>
  pagedQuery({
    queryKey: swimlaneKeys.all(),
    fetch: (cursor, signal) => listSwimlanes(cursor === undefined ? {} : { cursor }, { signal }),
    staleTime: EVENT_FED,
  });

export const swimlaneQuery = (id: string) =>
  apiQuery({
    queryKey: swimlaneKeys.detail(id),
    queryFn: ({ signal }) => getSwimlane(id, { signal }),
    staleTime: EVENT_FED,
  });

/**
 * The files of a swimlane that are not in sync (the Drift tab), for one state or for all of them. A flat list across the
 * swimlane, which is what the tree cannot give (it lists one folder at a time). It sits under the swimlane's own key, so every
 * event that refreshes the swimlane (drift_changed, file_observed, swimlane_changed, the sync windows) refreshes it too.
 */
export const swimlaneDriftQuery = (id: string, state: DriftState | null) =>
  pagedQuery({
    queryKey: swimlaneKeys.drift(id, state),
    fetch: (cursor, signal) => listDrift(id, { ...(state === null ? {} : { state }), ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });
