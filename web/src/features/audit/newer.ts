import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { useLiveStatus } from "@/live/LiveProvider";
import { adminKeys, auditKeys, keyRoot, prKeys, proposalKeys, swimlaneKeys, type QueryKey } from "@/lib/query/keys";

// "n updates": the live events that may have put a row into the audit log since it was read. No event names the audit trail (the
// contract has none), so nothing refreshes it, and nothing polls it. But the events that DO exist are the ones that come with an
// audited action: a file seen (the swimlanes), a proposal decided (the proposals), a pull request changing (the PRs), an access
// request (the admin list). The live layer refetches the lists of those that are on screen; this counts those refetches. It is a
// count of changes to the things the log is about, not of log rows (the hub does not say how many there are), and it is zero
// until a refetch of something that was already loaded finishes after the log was read: a first load is not news. The catch-up
// refetch that follows the stream opening (or opening again) is not news either, so what finishes in the first second of a page
// or of a live stream is not counted.

const ROOTS: ReadonlySet<string> = new Set([swimlaneKeys.all()[0], swimlaneKeys.details()[0], proposalKeys.lists()[0], proposalKeys.detail(0)[0], prKeys.lists()[0]]);
const ACCESS: QueryKey = adminKeys.section("access-requests");

export function isSignal(key: QueryKey): boolean {
  if (ROOTS.has(keyRoot(key))) return true;
  return key[0] === ACCESS[0] && key[1] === ACCESS[1];
}

/** The time after the page opens, and after the stream goes live, in which a refetch is the catch-up and not an event. */
export const QUIET_MS = 1000;

/** How many refetches of the things the audit log is about finished after `readAt` (when the log was read). */
export function useNewerEvents(readAt: number): number {
  const client = useQueryClient();
  const live = useLiveStatus().status;
  const [count, setCount] = useState(0);
  const quietUntil = useRef(performance.now() + QUIET_MS);
  useEffect(() => {
    if (live === "live") quietUntil.current = performance.now() + QUIET_MS;
  }, [live]);
  useEffect(
    () =>
      client.getQueryCache().subscribe((event) => {
        if (event.type !== "updated" || event.action.type !== "success") return;
        // The cache's event types its query loosely (`any`); this is the part of it that is read.
        const { state, queryKey } = event.query as { state: { dataUpdateCount: number; dataUpdatedAt: number }; queryKey: QueryKey };
        // The log was read again: what was counted is in it now.
        if (keyRoot(queryKey) === auditKeys.all()[0]) {
          setCount(0);
          return;
        }
        if (performance.now() < quietUntil.current) return;
        // dataUpdateCount 1 is the first load; later ones are refetches, which is what an event causes.
        if (state.dataUpdateCount > 1 && state.dataUpdatedAt >= readAt && isSignal(queryKey)) setCount((current) => current + 1);
      }),
    [client, readAt],
  );
  return count;
}
