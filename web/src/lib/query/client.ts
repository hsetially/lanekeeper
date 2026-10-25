import { QueryClient } from "@tanstack/react-query";
import { retryDelayMs, shouldRetry } from "@/api/retry";

// Defaults for every query and mutation (S2, S10). There is deliberately no persister: nothing the API returns is written
// to storage (src/lib/query/no-persist.test.ts), so a shared machine keeps nothing after the tab closes.

/** Unused data is dropped after five minutes. */
export const QUERY_GC_MS = 5 * 60 * 1000;
/** Data younger than this is not refetched on mount; live events invalidate precisely when it changes sooner. */
export const QUERY_STALE_MS = 15 * 1000;

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        gcTime: QUERY_GC_MS,
        staleTime: QUERY_STALE_MS,
        refetchOnWindowFocus: false,
        retry: shouldRetry,
        retryDelay: (failureCount: number) => retryDelayMs(failureCount),
      },
      mutations: { retry: false },
    },
  });
}
