import { getServedFile } from "@/api/generated/endpoints/swimlanes/swimlanes";
import { POLLED_MS } from "@/lib/query/freshness";
import { swimlaneKeys } from "@/lib/query/keys";
import { apiQuery } from "@/lib/query/options";
import { ASKED_BY_HAND } from "@/lib/query/targets";
import type { ServedSelection } from "./model";

// The "As served" view: one file as the config-server answers for one application, tenant and channel (`getServedFile`).
//
// The hub does not cache the answer: it asks the swimlane's agent, which asks the config-server, for every request, because the answer
// is what the config-server says NOW and it can differ from the files for as long as the config-server has not picked a change up
// (D85, D86). Each request is therefore an agent round trip, bounded on the hub (10 s, 16 at a time on a replica, 5 a second at the
// agent), and this query is shaped around that:
//
//  - it has a root of its own (`served`), not a place under the swimlane's key. `file_observed` and `drift_changed` name the swimlane's
//    summary and detail, and a scan in a sync window sends hundreds of them; if they named this query it would be started again, and
//    its round trip cancelled, for every batch. No event names it, so it is fresh for a minute (POLLED_MS) and then says it may be out
//    of date, and the person asks again with a button;
//  - opening the page asks again (`refetchOnMount: "always"`), though an answer kept from a moment ago is shown meanwhile;
//  - it is dropped half a minute after the screen is left. The text is the config-server's answer for one request, not something to
//    keep: going back to the view asks again. (It is also the largest thing a query of this app holds, with the file content.)
//  - the ETag the hub sends is replayed by the fetch layer (src/api/client.ts), so an answer that did not change costs a 304, though
//    the round trip to the agent is made all the same.

/** How long an answer is kept after the last screen that showed it has gone. */
export const SERVED_GC_MS = 30_000;

/**
 * The served file for a selection. With no selection the query is disabled (the address is not a complete request yet), and its
 * function refuses to run, so that even a forced refetch of the idle query sends nothing.
 */
export const servedFileQuery = (swimlane: string, selection: ServedSelection | null) => ({
  ...apiQuery({
    queryKey: swimlaneKeys.served(swimlane, selection?.application ?? "", selection?.tenant ?? "", selection?.channel ?? null, selection?.file ?? ""),
    queryFn: ({ signal }) => {
      if (selection === null) return Promise.reject(new Error("There is no selection to ask the config-server for."));
      return getServedFile(
        swimlane,
        { application: selection.application, tenant: selection.tenant, ...(selection.channel === null ? {} : { channel: selection.channel }), file: selection.file },
        { signal },
      );
    },
    staleTime: POLLED_MS,
    gcTime: SERVED_GC_MS,
  }),
  enabled: selection !== null,
  // Not tried again. The app's default is two more tries after a 503 or a network error (src/api/retry.ts), which is right for a read
  // the hub answers from its own database. This one is a round trip to an agent, limited at the hub, the replica and the agent
  // (above); three of them for one click would spend three of a limited number, and the person would wait through the pauses between
  // them to read an error. The page shows the error at once and has "Ask again" (design/DEVIATIONS.md, "As served: text and states").
  retry: false as const,
  // A resync (the first connection of the event stream, a reconnect, the hub's own) reads every query on screen again; this one is
  // asked for, not pushed: a page opened cold would make two round trips to the agent, and the first would be cancelled while out.
  meta: ASKED_BY_HAND,
  // Opening the page asks again, whatever is kept from a moment ago: the person who comes here after an edit, or to see whether the
  // config-server has caught up, wants what it says now. The answer kept is on screen meanwhile.
  refetchOnMount: "always" as const,
});
