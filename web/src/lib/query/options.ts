import { queryOptions } from "@tanstack/react-query";
import type { ApiProblem } from "@/api/problem";
import type { QueryKey } from "./keys";

// `queryOptions` with the error type of this app: every failed request is an ApiProblem (src/api/client.ts), so a query built
// with this has `error: ApiProblem | null`, which is what useQueryState and the error panel take. The key is whatever a key
// factory made; `staleTime` is required, because "how long is this fresh" is a decision every query has to make
// (src/lib/query/freshness.ts has the three answers).

export function apiQuery<TData>(options: {
  queryKey: QueryKey;
  queryFn: (context: { signal: AbortSignal }) => Promise<TData>;
  staleTime: number;
  gcTime?: number;
}) {
  const { queryKey, queryFn, staleTime, gcTime } = options;
  return queryOptions<TData, ApiProblem, TData>({ queryKey, queryFn, staleTime, ...(gcTime === undefined ? {} : { gcTime }) });
}
