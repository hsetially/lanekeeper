import type { QueryClient, UseMutationOptions } from "@tanstack/react-query";
import type { AutoNotify } from "@/api/generated/model";
import type { ApiProblem } from "@/api/problem";
import { adminKeys, type QueryKey } from "./keys";
import { writeTargets, type MutatingOperationId, type WriteContext } from "./invalidate";
import { invalidateTargets, type Target } from "./targets";

// The one place that may touch the cache while a write is in flight (a named, one-file override of the lint rule that bans
// `onMutate` and `setQueryData` everywhere else; web/lint/lanekeeper-rules.mjs, id `optimistic-update`).
//
// Why the ban, and why this exception. Most writes of the app are hash-guarded (a 409 says someone else changed the thing),
// approval-gated (a 202 is "sent for approval", not "done") or secret-bearing. An optimistic value would claim a result for
// each of those that the server may refuse or has not decided, and a secret must never be written into the cache. So a write
// shows the server's answer (src/lib/query/write.ts), except the operations in OPTIMISTIC below.
//
// What the helper does, in order: it cancels what is loading for the affected keys (a late answer must not overwrite the
// expected one), remembers what they held, shows the expected answer, puts the old one back if the write fails, and ALWAYS
// refetches the affected keys when the write settles, success or failure, so the screen ends on the server's answer.
//
// Who may be on the list is decided by the contract, not by taste, and optimistic.test.ts checks every entry against
// api/openapi.yaml: a PUT (idempotent, the whole value), no 202, 409, 412 or 423 among its answers, no expected hash or
// If-Match, not secret-bearing, not a reveal. The list is a typed registry; adding to it is a review item.

type Patch = { readonly key: QueryKey; readonly patch: (cached: never) => unknown };

type Entry<V> = {
  /** Why this operation is safe to show before it answers. A reviewer reads this. */
  readonly why: string;
  /** The cache entries the write changes, and what each should hold if the write is accepted. */
  readonly entries: (variables: V) => readonly Patch[];
};

export const OPTIMISTIC = {
  adminPutAutoNotify: {
    why:
      "The auto-notify switch of one swimlane: a PUT of the whole value ({enabled}), so the answer is exactly what was sent. The contract gives it no 202 (it is not approval-gated), no 409, 412 or 423 and no expected hash, it carries no secret, and the setting changes nothing by itself until a later write notifies services. Admin only.",
    entries: (variables: { readonly swimlane: string; readonly body: AutoNotify }): readonly Patch[] => [
      { key: adminKeys.list("auto-notify", { id: variables.swimlane }), patch: (cached: AutoNotify) => ({ ...cached, enabled: variables.body.enabled }) },
    ],
  },
} as const satisfies Record<string, Entry<never>>;

export type OptimisticOperation = Extract<keyof typeof OPTIMISTIC, MutatingOperationId>;
export type OptimisticVariables<K extends OptimisticOperation> = Parameters<(typeof OPTIMISTIC)[K]["entries"]>[0];

export function isOptimisticOperation(operation: string): operation is OptimisticOperation {
  return Object.hasOwn(OPTIMISTIC, operation);
}

type Snapshot = { readonly key: QueryKey; readonly previous: unknown };
type Rollback = { readonly snapshots: readonly Snapshot[] };

export type OptimisticSpec<K extends OptimisticOperation, V extends OptimisticVariables<K>, R> = {
  operation: K;
  run: (variables: V) => Promise<R>;
  /** What the write changed, for the invalidation table (src/lib/query/invalidate.ts). */
  context: (variables: V) => WriteContext<K>;
};

export function optimisticWriteMutation<K extends OptimisticOperation, V extends OptimisticVariables<K>, R>(client: QueryClient, spec: OptimisticSpec<K, V, R>): UseMutationOptions<R, ApiProblem, V, Rollback> {
  const patches = (variables: V): readonly Patch[] => (OPTIMISTIC[spec.operation].entries as (variables: V) => readonly Patch[])(variables);
  return {
    mutationKey: ["write", spec.operation],
    mutationFn: spec.run,
    retry: false,
    gcTime: 0,
    onMutate: async (variables) => {
      const snapshots: Snapshot[] = [];
      for (const { key, patch } of patches(variables)) {
        await client.cancelQueries({ queryKey: key, exact: true });
        const previous = client.getQueryData(key);
        // Nothing on screen to change: a value is never made up for an entry that holds none.
        if (previous === undefined) continue;
        snapshots.push({ key, previous });
        client.setQueryData(key, (patch as (cached: unknown) => unknown)(previous));
      }
      return { snapshots };
    },
    onError: (_error, _variables, rollback) => {
      for (const { key, previous } of rollback?.snapshots ?? []) client.setQueryData(key, previous);
    },
    // Always: whatever the outcome, the entries this write touched are asked for again.
    onSettled: async (_result, _error, variables) => {
      const own: Target[] = patches(variables).map(({ key }) => ({ kind: "key", key }));
      await invalidateTargets(client, [...own, ...writeTargets(spec.operation, spec.context(variables))]);
    },
  };
}
