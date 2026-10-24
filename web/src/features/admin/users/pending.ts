import { adminQueries } from "@/features/admin/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";

/**
 * The access requests that wait for an Admin. The Admin area's navigation counts them and the Users section lists them, from ONE
 * cache entry: `access_request_created` (src/live/invalidation.ts) refreshes it, so the count and the list change together. The
 * sidebar's Admin item counts them too (`enabled` only for an Admin, and `autoLoad` off: a count needs the first page, and "N+"
 * says there are more); the list asks for all of them, into the same entry.
 */
export function usePendingRequests({ enabled = true, autoLoad = true }: { enabled?: boolean; autoLoad?: boolean } = {}) {
  const pages = useCursorPages({ ...adminQueries.accessRequests({ status: "pending" }), autoLoad, enabled });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  return { pages, state };
}
