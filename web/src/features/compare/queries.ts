import { compareFile, compareTree, getGrid } from "@/api/generated/endpoints/compare/compare";
import { listGitRefs } from "@/api/generated/endpoints/git/git";
import type { GitRefKind, RepoKind, SwimlaneId } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { compareKeys, gitKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";

// Comparisons. A side is a ref (`nfs:<swimlane>`, `baseline:<swimlane>`, `effective:<swimlane>`, `git:base@<ref>`), and a ref
// moves, so none of these is immutable and none is keyed by a hash: the key holds the refs, and file_observed, drift_changed
// and git_head_moved invalidate by the swimlane or branch a key names (src/lib/query/keys.ts, `matchers`). The fetch layer
// revalidates `compareFile` by ETag. All of them are EVENT_FED.

/** One file between two sides, with the hunks the engine computed (the browser never diffs). */
export const compareFileQuery = (left: string, right: string, file: string, channel: string | null) =>
  apiQuery({
    queryKey: compareKeys.diff(left, right, file, channel),
    queryFn: ({ signal }) => compareFile({ left, right, file, ...(channel === null ? {} : { channel }) }, { signal }),
    staleTime: EVENT_FED,
  });

/** The files that differ between two sides under a folder. */
export const compareTreeQuery = (left: string, right: string, prefix: string) =>
  pagedQuery({
    queryKey: compareKeys.tree(left, right, prefix),
    fetch: (cursor, signal) => compareTree({ left, right, ...(prefix === "" ? {} : { prefix }), ...(cursor === undefined ? {} : { cursor }) }, { signal }).then((tree) => tree.entries),
    staleTime: EVENT_FED,
  });

/**
 * One logical file across swimlanes: a row per setting, a column per swimlane (the caller's `lanes`, in that order). The rows
 * are paged by cursor inside one cache entry; the first page should be small (50) and the page size is the caller's.
 */
export const gridQuery = (file: string, lanes: readonly SwimlaneId[], onlyDiff: boolean, channel: string | null) =>
  pagedQuery({
    queryKey: compareKeys.grid(file, lanes, onlyDiff, channel),
    fetch: (cursor, signal) =>
      getGrid({ file, swimlanes: [...lanes], ...(onlyDiff ? { only_differing: true } : {}), ...(channel === null ? {} : { channel }), ...(cursor === undefined ? {} : { cursor }) }, { signal }).then((grid) => grid.rows),
    staleTime: EVENT_FED,
  });

/** The branches and tags of one of the two repositories, filtered by a prefix. */
export const gitRefsQuery = (repo: RepoKind, kind: GitRefKind | null, q: string) =>
  pagedQuery({
    queryKey: gitKeys.refs(repo, kind ?? "", q),
    fetch: (cursor, signal) => listGitRefs({ repo, ...(kind === null ? {} : { kind }), ...(q === "" ? {} : { q }), ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });
