import type { QueryState, StaleCause } from "@/lib/query/use-query-state";

/**
 * One state for a screen that needs two answers. The first failure wins (and `refetch` asks for what failed), loading lasts
 * while either is loading, and the data is both answers once both are there. What is stale is the union of the causes, and as
 * old as the older answer.
 */
export function combine<A, B>(a: QueryState<A>, b: QueryState<B>): QueryState<[A, B]> {
  const failed = a.state === "error" ? a : b.state === "error" ? b : null;
  const state = failed ? "error" : a.state === "loading" || b.state === "loading" ? "loading" : a.state === "empty" || b.state === "empty" ? "empty" : "ready";
  const causes: StaleCause[] = [...new Set([...(a.stale?.causes ?? []), ...(b.stale?.causes ?? [])])].sort();
  const times = [a.stale?.asOf, b.stale?.asOf].filter((t): t is number => typeof t === "number");
  return {
    state,
    data: a.data !== undefined && b.data !== undefined ? [a.data, b.data] : undefined,
    error: failed?.error ?? a.error ?? b.error,
    retryable: failed?.retryable ?? false,
    refreshing: a.refreshing || b.refreshing,
    stale: causes.length === 0 ? null : { causes, asOf: times.length === 0 ? null : Math.min(...times) },
    refetch: () => {
      if (failed) failed.refetch();
      else {
        a.refetch();
        b.refetch();
      }
    },
  };
}
