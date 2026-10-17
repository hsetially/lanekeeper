import { partialMatchKey, type QueryClient } from "@tanstack/react-query";
import { compareKeys, isImmutableKey, matchers, type QueryKey } from "./keys";

// What a live event or a write can name in the cache, as DATA. A `Target` says which cache entries a change touches, built from
// the key factories of keys.ts and nothing else; `matches` says whether one entry is named by one; `invalidateTargets` makes
// the one pass over the QueryClient. The live events (src/live/invalidation.ts) and the writes (invalidate.ts) both speak in
// targets, so "sit1 changed" finds the same entries whichever of them says it.
//
// Entries nobody is showing are only marked stale, so they refetch when a screen mounts them; the ones on screen refetch now
// (`refetchType: "active"`). An immutable entry (content and blobs, functions of a hash) is never named, never marked, never
// refetched. And there is no call anywhere that invalidates without a filter: this is the only place that invalidates in bulk,
// and it always passes a predicate.

export type Target =
  /** Every entry whose key starts with `key`. */
  | { readonly kind: "key"; readonly key: QueryKey }
  /** The head of one file, unless the cached head already has this hash (our own write's echo). */
  | { readonly kind: "file-head"; readonly swimlane: string; readonly path: string; readonly hash: string }
  /** The tree pages of a swimlane that list a folder on the way to any of these paths. */
  | { readonly kind: "trees"; readonly swimlane: string; readonly paths: readonly string[] }
  /** The file comparisons that have the swimlane on either side. */
  | { readonly kind: "compares"; readonly swimlane: string }
  /** The tree comparisons that have the swimlane on either side. */
  | { readonly kind: "compare-trees"; readonly swimlane: string }
  /** The grids that have the swimlane among their columns. */
  | { readonly kind: "grids"; readonly swimlane: string }
  /** The git reads of a repository at a branch. */
  | { readonly kind: "content-refs"; readonly repo: string; readonly branch: string };

/** A stable name for a target, to count and deduplicate them. Keys are JSON-safe by construction. */
export function targetId(target: Target): string {
  return JSON.stringify(target);
}

/**
 * Whether the cache entry with this key (and this data) is named by the target. Immutable entries are never named. `data` is
 * what the entry holds, and only the file-head rule reads it.
 */
export function matches(target: Target, entryKey: QueryKey, data: unknown): boolean {
  if (isImmutableKey(entryKey)) return false;
  switch (target.kind) {
    case "key":
      return partialMatchKey(entryKey, target.key);
    case "file-head": {
      const [root, swimlane, path] = entryKey;
      if (root !== "file-head" || swimlane !== target.swimlane || path !== target.path) return false;
      const cached = typeof data === "object" && data !== null ? (data as { hash?: unknown }).hash : undefined;
      return cached !== target.hash;
    }
    case "trees":
      return target.paths.some((path) => matchers.treeCovers(entryKey, target.swimlane, path));
    case "compares":
      return entryKey[0] === compareKeys.diffs()[0] && matchers.comparesSwimlane(entryKey, target.swimlane);
    case "compare-trees":
      return entryKey[0] === compareKeys.trees()[0] && matchers.comparesSwimlane(entryKey, target.swimlane);
    case "grids":
      return matchers.gridHasLane(entryKey, target.swimlane);
    case "content-refs":
      return matchers.contentRefOnBranch(entryKey, target.repo, target.branch);
  }
}

/**
 * Invalidate what the targets name, or every active query that is not immutable (`"everything"`, a resync or a window with too
 * many targets to test one by one). Resolves when the entries on screen have refetched; a refetch that fails is that query's
 * own error state, not an error here.
 */
export function invalidateTargets(client: QueryClient, targets: readonly Target[] | "everything"): Promise<void> {
  if (targets !== "everything" && targets.length === 0) return Promise.resolve();
  return client
    .invalidateQueries({
      predicate: (query) => !isImmutableKey(query.queryKey) && (targets === "everything" || targets.some((target) => matches(target, query.queryKey, query.state.data))),
      refetchType: "active",
    })
    .catch(() => undefined);
}
