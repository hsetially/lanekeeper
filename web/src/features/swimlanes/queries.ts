import { getSwimlane, listPendingRestarts, listSwimlanes } from "@/api/generated/endpoints/swimlanes/swimlanes";
import { EVENT_FED } from "@/lib/query/freshness";
import { swimlaneKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";

// Swimlanes: the list, one swimlane, and the pickup state of its services. Live events keep all three fresh (file_observed,
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

/** The services of a swimlane and whether each has picked up its config (Live, Live by HH:MM, Needs notify or restart, Needs config-server restart). */
export const swimlanePickupQuery = (id: string) =>
  pagedQuery({
    queryKey: swimlaneKeys.pickup(id),
    fetch: (cursor, signal) => listPendingRestarts(id, cursor === undefined ? {} : { cursor }, { signal }),
    staleTime: EVENT_FED,
  });
