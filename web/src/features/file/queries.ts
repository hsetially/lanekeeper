import { getBlob } from "@/api/generated/endpoints/content-store/content-store";
import { getFileContent, listFileHistory, listSwimlaneTree } from "@/api/generated/endpoints/swimlanes/swimlanes";
import type { DriftState, RepoKind } from "@/api/generated/model";
import { EVENT_FED, IMMUTABLE, IMMUTABLE_GC_MS } from "@/lib/query/freshness";
import { fileKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";

// Files of one swimlane. The fetch layer revalidates the content answers by ETag (src/api/client.ts keeps the tag and sends
// If-None-Match), so a refetch of a head or a git read that did not change is a 304 with no body.
//
//  - tree, head, git read, history: EVENT_FED. file_observed, drift_changed, attribution_updated and git_head_moved name
//    them, so they are refetched when something they show changes and never because of their age;
//  - content by hash and blobs: IMMUTABLE. A hash names the same bytes forever, so they never go stale, no event ever names
//    them, a resync never refetches them, and they are kept ten minutes after the last screen leaves.

/** One folder, as a page-set. `prefix` is the folder, `""` for the root; `drift` filters to one state, null for all. */
export const fileTreeQuery = (id: string, prefix: string, drift: DriftState | null) =>
  pagedQuery({
    queryKey: fileKeys.tree(id, prefix, drift),
    fetch: (cursor, signal) => listSwimlaneTree(id, { ...(prefix === "" ? {} : { prefix }), ...(drift === null ? {} : { drift }), ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });

/** The file as it is on NFS now: its text and hash. This is what a save's expected hash comes from. */
export const fileHeadQuery = (id: string, path: string) =>
  apiQuery({
    queryKey: fileKeys.head(id, path),
    queryFn: ({ signal }) => getFileContent(id, { path, source: "nfs" }, { signal }),
    staleTime: EVENT_FED,
  });

/** The file at one hash: the same bytes forever. */
export const fileContentQuery = (id: string, path: string, hash: string) =>
  apiQuery({
    queryKey: fileKeys.content(id, path, hash),
    queryFn: ({ signal }) => getFileContent(id, { path, source: "blob", hash }, { signal }),
    staleTime: IMMUTABLE,
    gcTime: IMMUTABLE_GC_MS,
  });

/** The file as git has it at a ref. A ref moves (git_head_moved), so this is not immutable. */
export const fileRefQuery = (id: string, path: string, repo: RepoKind, ref: string) =>
  apiQuery({
    queryKey: fileKeys.contentRef(id, path, repo, ref),
    queryFn: ({ signal }) => getFileContent(id, { path, source: "git", repo, ref }, { signal }),
    staleTime: EVENT_FED,
  });

/** A blob by its hash (a binary file, an image): the same bytes forever. */
export const blobQuery = (hash: string) =>
  apiQuery({
    queryKey: fileKeys.blob(hash),
    queryFn: ({ signal }) => getBlob(hash, { signal }),
    staleTime: IMMUTABLE,
    gcTime: IMMUTABLE_GC_MS,
  });

/** The versions of a file, newest first. */
export const fileHistoryQuery = (id: string, path: string) =>
  pagedQuery({
    queryKey: fileKeys.history(id, path),
    fetch: (cursor, signal) => listFileHistory(id, { path, ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });
