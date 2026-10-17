import { useInfiniteQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef } from "react";
import type { ApiProblem } from "@/api/problem";
import type { QueryKey } from "./keys";

// One list that the server pages with a keyset cursor (`Limit` at most 500, `Cursor` is the `next` of the page before): the
// grid, the tree, findings, history, search. The pages live in ONE cache entry under ONE key (src/lib/query/keys.ts keeps the
// cursor out of the key), so a live event invalidates the list once and a new page never makes a new entry.
//
// The call to the server is the generated function of the operation (for example `getGrid`), passed in as `fetchPage`; this
// hook knows nothing about any operation. Everything is bounded (rule 5): at most `maxPages` pages are ever asked for, and the
// result says so when it stopped there, so a screen can say "showing the first N" instead of looking complete.

export const DEFAULT_MAX_PAGES = 50;

export type CursorPageOptions<TPage, TItem> = {
  queryKey: QueryKey;
  /** The first page is asked for with no cursor. `signal` aborts the request when the entry is dropped. */
  fetchPage: (cursor: string | undefined, signal: AbortSignal) => Promise<TPage>;
  /** The items of a page and its `next` cursor (null at the end). */
  select: (page: TPage) => { readonly items: readonly TItem[]; readonly next: string | null };
  /** The most pages that are asked for. Default 50 (25,000 rows at the largest page size). */
  maxPages?: number;
  /** Keep asking for the next page until the end (or `maxPages`). For a view that needs all of it, such as a grid. */
  autoLoad?: boolean;
  enabled?: boolean;
  /** How long the list counts as fresh. `Infinity` for a list that live events keep fresh. Default: the client's. */
  staleTime?: number;
  /** How long the list is kept after nobody shows it. Default: the client's. */
  gcTime?: number;
};

export type CursorPages<TItem> = {
  /** Every item loaded so far, in order. The same array until a page arrives or the list is refetched. */
  items: TItem[];
  /** `pending` until the first page arrives, `error` only when it failed, else `success`. */
  status: "pending" | "error" | "success";
  /** The error of the first page. Null once any page is shown. */
  error: ApiProblem | null;
  /** The error of the last attempt after the first page was shown (a later page, or a refetch). Items stay on screen. */
  loadMoreError: ApiProblem | null;
  /** The server has a next page and `maxPages` has not been reached. */
  hasMore: boolean;
  /** `maxPages` was reached with more still on the server: the list is cut off. */
  truncated: boolean;
  isFetching: boolean;
  /** When the list was last fetched (ms since the epoch); 0 before the first page. For "last updated" and the stale banner. */
  dataUpdatedAt: number;
  /** TanStack's staleness: older than the entry's `staleTime`, or invalidated. */
  isStale: boolean;
  /** Ask for the next page. Does nothing when there is none, and joins a request that is already out. */
  loadMore: () => Promise<void>;
  refetch: () => Promise<void>;
};

export function useCursorPages<TPage, TItem>(options: CursorPageOptions<TPage, TItem>): CursorPages<TItem> {
  const { queryKey, fetchPage, select, maxPages = DEFAULT_MAX_PAGES, autoLoad = false, enabled = true, staleTime, gcTime } = options;
  const query = useInfiniteQuery<TPage, ApiProblem, { pages: TPage[] }, QueryKey, string | undefined>({
    queryKey,
    queryFn: ({ pageParam, signal }) => fetchPage(pageParam, signal),
    initialPageParam: undefined,
    getNextPageParam: (lastPage, allPages) => (allPages.length >= maxPages ? undefined : (select(lastPage).next ?? undefined)),
    select: (data) => ({ pages: data.pages }),
    enabled,
    ...(staleTime === undefined ? {} : { staleTime }),
    ...(gcTime === undefined ? {} : { gcTime }),
  });

  const pages = query.data?.pages;
  const items = useMemo(() => (pages ? pages.flatMap((page) => select(page).items) : []), [pages, select]);
  const hasData = pages !== undefined;
  const lastPage = pages?.[pages.length - 1];
  const truncated = lastPage !== undefined && pages !== undefined && pages.length >= maxPages && select(lastPage).next !== null;
  const loadMoreError = hasData && query.error ? query.error : null;

  const inFlight = useRef<Promise<void> | null>(null);
  const { hasNextPage, fetchNextPage, refetch: refetchAll } = query;
  const loadMore = useCallback((): Promise<void> => {
    if (inFlight.current) return inFlight.current;
    if (!hasNextPage) return Promise.resolve();
    const run = fetchNextPage()
      .then(() => undefined)
      .finally(() => {
        inFlight.current = null;
      });
    inFlight.current = run;
    return run;
  }, [hasNextPage, fetchNextPage]);

  useEffect(() => {
    // A failed page is not asked for again by itself: the screen shows the error and the person decides. `pages` is a
    // dependency because React may batch the render in which a request was out away, so "fetching" is never seen to change.
    if (autoLoad && hasNextPage && !query.isFetching && loadMoreError === null) void loadMore();
  }, [autoLoad, hasNextPage, query.isFetching, loadMoreError, loadMore, pages]);

  return {
    items,
    status: hasData ? "success" : query.status,
    error: hasData ? null : query.error,
    loadMoreError,
    hasMore: hasNextPage,
    truncated,
    isFetching: query.isFetching,
    dataUpdatedAt: query.dataUpdatedAt,
    isStale: query.isStale,
    loadMore,
    refetch: () => refetchAll().then(() => undefined),
  };
}
