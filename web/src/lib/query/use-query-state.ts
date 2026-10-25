import { useEffect, useState } from "react";
import { ApiProblem } from "@/api/problem";
import { useLiveStatus } from "@/live/LiveProvider";
import type { CursorPages } from "./cursor-pages";

// What a screen should show for a query, decided in one place so that every screen behaves the same way.
//
// STATE is what replaces the content, and it is one of four:
//   loading  no data yet (the design's skeleton, with the footer "Loading <noun>…");
//   error    the first load failed and there is nothing to show (the design's error panel, with the problem's title and the request ID);
//   empty    data arrived and the caller says there is nothing in it (the design's empty state, which the caller supplies);
//   ready    data to show.
//
// Two OVERLAYS never replace the content, so a screen with data never goes back to a skeleton or an error panel:
//   refreshing  a background refetch is running (shown only after a short delay, so a fast one does not flash);
//   stale       the data may be out of date, and the cause: it is older than its staleTime (`age`), the live stream is down so
//               events may have been missed (`live-paused`), or a refetch failed and the data shown is the last good one
//               (`refetch-failed`). It carries when the data is from.
//
// A query that live events keep fresh sets `staleTime: Infinity` (see src/features/*/queries.ts): it is never stale by age,
// and the stream going down is what makes it stale. A query that no event covers (audit, admin lists) has a finite staleTime
// and says so when it gets old.

export type QueryStateKind = "loading" | "error" | "empty" | "ready";
export type StaleCause = "age" | "live-paused" | "refetch-failed";

/** What the hook needs from a query. A TanStack `UseQueryResult` has all of it; so does `cursorPagesSource`. */
export type QuerySource<T> = {
  readonly data: T | undefined;
  readonly error: ApiProblem | null;
  readonly status: "pending" | "error" | "success";
  readonly isFetching: boolean;
  /** TanStack's staleness: older than the staleTime, or invalidated. Absent: never stale by age. */
  readonly isStale?: boolean;
  /** When the data was fetched, in milliseconds since the epoch. */
  readonly dataUpdatedAt?: number;
  readonly refetch: () => unknown;
};

export type QueryState<T> = {
  readonly state: QueryStateKind;
  readonly data: T | undefined;
  /** The failure of the latest attempt: the screen's error when `state` is `error`, the reason of a failed refresh otherwise. */
  readonly error: ApiProblem | null;
  /** Whether trying again can change the answer (a network error, a timeout, a 5xx, a 429). Not for a 403 or a 404. */
  readonly retryable: boolean;
  readonly refreshing: boolean;
  readonly stale: { readonly causes: readonly StaleCause[]; readonly asOf: number | null } | null;
  readonly refetch: () => void;
};

export type QueryStateOptions<T> = {
  /** Whether the data has nothing in it. Without it, data is never `empty`. */
  isEmpty?: (data: T) => boolean;
  /** How long a background refetch runs before `refreshing` shows. Default 400 ms. */
  refreshDelayMs?: number;
};

export const REFRESH_DELAY_MS = 400;

/** An answer that a second try can change: nothing reached the server, it is busy, or it failed. Not a refusal. */
export function isRetryableProblem(problem: ApiProblem): boolean {
  if (problem.status === 0) return problem.kind === "network" || problem.kind === "timeout";
  return problem.status === 429 || problem.status === 500 || problem.status === 502 || problem.status === 503 || problem.status === 504;
}

/** True once `flag` has been true for `ms` without a break; false at once when it is not. */
function useDelayedFlag(flag: boolean, ms: number): boolean {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    if (!flag) {
      setShown(false);
      return undefined;
    }
    const timer = setTimeout(() => {
      setShown(true);
    }, ms);
    return () => {
      clearTimeout(timer);
    };
  }, [flag, ms]);
  return flag && shown;
}

export function useQueryState<T>(source: QuerySource<T>, options: QueryStateOptions<T> = {}): QueryState<T> {
  const { data, error, status, isFetching, isStale, dataUpdatedAt, refetch } = source;
  const live = useLiveStatus();
  const hasData = data !== undefined;
  const refreshing = useDelayedFlag(hasData && isFetching, options.refreshDelayMs ?? REFRESH_DELAY_MS);

  let state: QueryStateKind;
  if (!hasData) state = status === "error" ? "error" : "loading";
  else state = options.isEmpty?.(data) === true ? "empty" : "ready";

  const causes: StaleCause[] = [];
  if (hasData) {
    if (error !== null) causes.push("refetch-failed");
    if (live.status === "offline" || live.status === "stopped") causes.push("live-paused");
    // A refetch in flight is `refreshing`, not a reason to warn; and a failed one already says the data is old.
    if (isStale === true && !isFetching && error === null) causes.push("age");
  }

  return {
    state,
    data,
    error,
    retryable: error !== null && isRetryableProblem(error),
    refreshing,
    stale: causes.length > 0 ? { causes, asOf: dataUpdatedAt !== undefined && dataUpdatedAt > 0 ? dataUpdatedAt : null } : null,
    refetch: () => {
      void refetch();
    },
  };
}

/** The pages of a cursor-paged list as a source: its items are the data once a page has arrived. */
export function cursorPagesSource<T>(pages: CursorPages<T>): QuerySource<T[]> {
  return {
    data: pages.status === "success" ? pages.items : undefined,
    error: pages.status === "error" ? pages.error : pages.loadMoreError,
    status: pages.status,
    isFetching: pages.isFetching,
    isStale: pages.isStale,
    dataUpdatedAt: pages.dataUpdatedAt,
    refetch: pages.refetch,
  };
}
