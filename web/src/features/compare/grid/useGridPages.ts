import { hashKey, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef } from "react";
import type { GridRow, GridRowPage } from "@/api/generated/model";
import { IDLE_WAIT_MS, useCursorPages, whenIdle, type CursorPages } from "@/lib/query/cursor-pages";
import type { QueryKey } from "@/lib/query/keys";
import { pagesToKeep } from "../model";
import { GRID_PAGE_SIZES, gridQuery } from "../queries";

// The pages of the grid, and what a live event does to them.
//
// The rows are one infinite query (src/lib/query/cursor-pages.ts), one cache entry under the grid key. A grid of 5,000 rows is
// eleven pages, and the live layer invalidates the entry when a swimlane in its columns changes (file_observed, drift_changed).
// An invalidated infinite query on screen refetches EVERY page it holds, one after the other, because each page's cursor
// comes from the page before: eleven requests, a 2.7 MB page each at 40 swimlanes, for one event. That is the cost of keeping
// the screen right if nothing is done, and a scan fires hundreds of events.
//
// So the grid keeps its observer DISABLED. The live layer refetches only the queries on screen (`refetchType: "active"`), and a
// query whose only observer is disabled is not "active": an event marks it invalidated and does nothing else. The grid then
// does the refetch itself, with the policy of the plan's T3 note ("the real grid route should use a first-page and reset
// refetch policy"): read again the first page, and the pages that reach the last row the person can see, and drop the rest,
// which are read again if the person scrolls to them. The data the person is looking at stays on screen until the new pages
// replace it in one step (TanStack replaces the pages when the whole refetch has succeeded), so the screen never goes back
// to a skeleton. A grid that was off screen when the event came is refreshed the same way when it comes back.
//
// `infiniteQuery({ pages })` is the one call that reads a given number of pages from the start and replaces the cache
// entry with them. (`setQueryData` could cut the list but is not allowed in the app: lint rule optimistic-update.)

export type GridSpec = {
  readonly file: string;
  readonly lanes: readonly string[];
  readonly onlyDiff: boolean;
  readonly channel: string | null;
};

/** The person is this close to the end of the rows loaded (in rows) when the next page is asked for. */
export const PREFETCH_ROWS = 100;

export type GridPages = CursorPages<GridRow> & {
  /**
   * Tell the grid where the person is looking: the index of the last row in view. When that is near the end of what is
   * loaded the next page is asked for, and a refresh after a live event reads as far as this row.
   */
  lookAt: (lastVisibleRow: number) => void;
};

export function useGridPages(spec: GridSpec): GridPages {
  const client = useQueryClient();
  // A new array with the same ids is the same grid, so the lanes are rebuilt from their text and the options stay the same object.
  const lanesKey = spec.lanes.join(",");
  const lanes = useMemo(() => (lanesKey === "" ? [] : lanesKey.split(",")), [lanesKey]);
  const options = useMemo(() => gridQuery(spec.file, lanes, spec.onlyDiff, spec.channel, GRID_PAGE_SIZES), [spec.file, lanes, spec.onlyDiff, spec.channel]);
  const key: QueryKey = options.queryKey;
  const hash = hashKey(key);

  const pages = useCursorPages({ ...options, enabled: false });
  const lastRow = useRef(0);
  const latest = useRef({ pages, options, key });
  useEffect(() => {
    latest.current = { pages, options, key };
  });

  // Read the rows again (as many pages as reach the last row in view), replacing what is cached in one step.
  const refresh = useCallback(() => {
    const { options: current, key: currentKey } = latest.current;
    const run = (): void => {
      // Counted when the read starts, so a page that arrived while this waited is in the count.
      const loaded = client.getQueryData<{ pages: unknown[] }>(currentKey)?.pages.length ?? 0;
      const keep = pagesToKeep(lastRow.current, loaded, GRID_PAGE_SIZES);
      void client
        .infiniteQuery({
          queryKey: currentKey,
          queryFn: ({ pageParam, signal }: { pageParam: string | undefined; signal: AbortSignal }) => current.fetchPage(pageParam, signal),
          initialPageParam: undefined as string | undefined,
          getNextPageParam: (last: GridRowPage) => current.select(last).next ?? undefined,
          pages: keep,
          // Always read: the entry was invalidated, and "fresh" is for the entry's own observers.
          staleTime: 0,
        })
        // A failed refresh leaves the rows on screen and the error on the entry, which the screen shows (`loadMoreError`).
        .catch(() => undefined);
    };
    const state = client.getQueryState(currentKey);
    if (state?.fetchStatus !== "fetching") run();
    // A page that the person asked for is out: it is not cancelled (a "Load more" that an event cancels is a click that does
    // nothing). It is read, and the refresh follows it, so no row on screen is older than the event.
    else if (state.fetchMeta?.fetchMore !== undefined) void whenIdle(client, currentKey, IDLE_WAIT_MS).then(run);
    // An event that comes while a refresh is on its way means that what is on its way may already be old.
    else void client.cancelQueries({ queryKey: currentKey }).then(run);
  }, [client]);

  // On screen: read the first page if there is nothing, or refresh if the rows are known to be old.
  useEffect(() => {
    const state = client.getQueryState(latest.current.key);
    if (state?.fetchStatus === "fetching") return;
    if (state?.data === undefined) void latest.current.pages.refetch();
    else if (state.isInvalidated) refresh();
  }, [client, hash, refresh]);

  // While on screen: an event invalidates the entry, and the entry is read again by the policy, not by the live layer.
  useEffect(() => {
    return client.getQueryCache().subscribe((event) => {
      if (event.type !== "updated" || event.action.type !== "invalidate") return;
      if (event.query.queryHash !== hash) return;
      refresh();
    });
  }, [client, hash, refresh]);

  const lookAt = useCallback((row: number) => {
    lastRow.current = row;
    const { pages: current } = latest.current;
    if (row >= current.items.length - PREFETCH_ROWS && current.hasMore && !current.isFetching && current.loadMoreError === null) void current.loadMore();
  }, []);
  // The button under the rows. The rows the person asks for with it are kept by a refresh until they scroll and say where they
  // are looking again (`lookAt`), so an event that comes while the page is out does not take the page away.
  const loadMore = useCallback((): Promise<void> => {
    const { pages: current } = latest.current;
    lastRow.current = Math.max(lastRow.current, current.items.length + GRID_PAGE_SIZES.next - 1);
    return current.loadMore();
  }, []);
  // Another grid (another file, lanes or filter) starts at the top.
  useEffect(() => {
    lastRow.current = 0;
  }, [hash]);

  return { ...pages, lookAt, loadMore };
}
