import { compareFile, compareTree, getGrid } from "@/api/generated/endpoints/compare/compare";
import { listGitRefs } from "@/api/generated/endpoints/git/git";
import type { GitRefKind, RepoKind, SwimlaneId } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { compareKeys, gitKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";
import type { PageSizes } from "./model";

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

/** The first page of the grid is small, so that the first screen is drawn when 50 rows have arrived and not when 2.7 MB has. */
export const GRID_FIRST_PAGE = 50;
/** Every other page of the grid is the largest the server gives (500 rows). */
export const GRID_PAGE = 500;
export const GRID_PAGE_SIZES: PageSizes = { first: GRID_FIRST_PAGE, next: GRID_PAGE };
/** A page of the entries of a tree comparison: a path and a state each, so the largest page is small. */
export const TREE_PAGE = 500;

/**
 * One logical file across swimlanes: a row per setting, a column per swimlane (the caller's `lanes`, in that order). The rows
 * are paged by cursor inside one cache entry. With `sizes` the first page has `sizes.first` rows and the others `sizes.next`:
 * one page of 500 rows by 40 swimlanes is 2.7 MB of JSON and the hub sends no compression, so the real screen always passes
 * GRID_PAGE_SIZES. Without it no `limit` is sent (the server's default), which is what the shared query test expects.
 */
export const gridQuery = (file: string, lanes: readonly SwimlaneId[], onlyDiff: boolean, channel: string | null, sizes?: PageSizes) =>
  pagedQuery({
    queryKey: compareKeys.grid(file, lanes, onlyDiff, channel),
    fetch: (cursor, signal) =>
      getGrid(
        {
          file,
          swimlanes: [...lanes],
          ...(onlyDiff ? { only_differing: true } : {}),
          ...(channel === null ? {} : { channel }),
          ...(sizes === undefined ? {} : { limit: cursor === undefined ? sizes.first : sizes.next }),
          ...(cursor === undefined ? {} : { cursor }),
        },
        { signal },
      ).then((grid) => grid.rows),
    staleTime: EVENT_FED,
  });

/**
 * The files that differ between two sides under a folder, as pages that also say how many identical subtrees the server
 * skipped (`skipped`, the same on every page). It is the key of `compareTreeQuery` with a page that keeps that number, which
 * `compareTreeQuery` drops; a screen mounts one of them for a key, never both.
 */
export const treeComparisonQuery = (left: string, right: string, prefix: string) =>
  pagedQuery({
    queryKey: compareKeys.tree(left, right, prefix),
    fetch: (cursor, signal) =>
      compareTree({ left, right, limit: TREE_PAGE, ...(prefix === "" ? {} : { prefix }), ...(cursor === undefined ? {} : { cursor }) }, { signal }).then((tree) => ({
        items: tree.entries.items,
        next: tree.entries.next,
        skipped: tree.skipped_identical_subtrees,
      })),
    staleTime: EVENT_FED,
  });

/** The branches and tags of one of the two repositories, filtered by a prefix. */
export const gitRefsQuery = (repo: RepoKind, kind: GitRefKind | null, q: string) =>
  pagedQuery({
    queryKey: gitKeys.refs(repo, kind ?? "", q),
    fetch: (cursor, signal) => listGitRefs({ repo, ...(kind === null ? {} : { kind }), ...(q === "" ? {} : { q }), ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });
