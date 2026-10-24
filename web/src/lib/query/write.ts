import type { QueryClient, UseMutationOptions } from "@tanstack/react-query";
import type { ApiProblem } from "@/api/problem";
import { invalidateAfterWrite, shouldRefetchAfterError, type MutatingOperationId, type WriteContext } from "./invalidate";

// The options of a plain write: the call, and what it changed invalidated afterwards (src/lib/query/invalidate.ts). The
// colocated mutations of the features (src/features/<domain>/mutations.ts) are built with it, and so are the writes of any
// screen that has none of its own.
//
// A plain write shows the SERVER'S answer: no optimistic value (the one exception is src/lib/query/optimistic.ts, for a
// closed list of operations), no `setQueryData`. Most writes carry an expected hash (a 409 says someone else got there
// first), may be sent for approval (a 202 is not "done"), or are secret-bearing, and an optimistic value would lie about
// all three.
//
//  - after a success the mutation stays pending until the entries on screen have refetched, so the screen is right when the
//    write returns (and a second save does not use the hash it just replaced);
//  - after a failure the cache is refetched only when it is probably out of date (409, 412, 423, 5xx, no answer);
//  - a write is never retried by this layer (the fetch layer retries a network error with the same Idempotency-Key, once);
//  - `gcTime: 0`: a write holds file content in its variables, so nothing of it outlives the component that made it.

export type WriteSpec<K extends MutatingOperationId, V, R> = {
  operation: K;
  /** The call: a generated function. */
  run: (variables: V) => Promise<R>;
  /** What the write changed. `result` is undefined after a failure, when whether it was applied is not known. */
  context: (variables: V, result: R | undefined) => WriteContext<K>;
};

export function writeMutation<K extends MutatingOperationId, V, R>(client: QueryClient, spec: WriteSpec<K, V, R>): UseMutationOptions<R, ApiProblem, V> {
  return {
    mutationKey: ["write", spec.operation],
    mutationFn: spec.run,
    retry: false,
    gcTime: 0,
    onSuccess: (result, variables) => invalidateAfterWrite(client, spec.operation, spec.context(variables, result)),
    onError: (error, variables) => (shouldRefetchAfterError(error) ? invalidateAfterWrite(client, spec.operation, spec.context(variables, undefined)) : undefined),
  };
}

/**
 * Whether a write was applied or only sent for approval, read from its answer (`WriteResult` carries `outcome`). An answer
 * with no `outcome` (a notify count, a pull-request job) was applied. The invalidation table needs it: a write that was only
 * sent for approval changed nothing yet.
 */
export function outcomeOf(result: unknown): { readonly outcome?: "applied" | "proposal_created" } {
  const outcome = typeof result === "object" && result !== null && "outcome" in result ? (result).outcome : undefined;
  return outcome === "proposal_created" || outcome === "applied" ? { outcome } : {};
}
