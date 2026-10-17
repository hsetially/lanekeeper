// The vocabulary of query keys: every cache of the app is named here and nowhere else (plan, "Query keys, retry and SSE
// invalidation"). A screen builds its key with these functions and hands it to the generated hook as `query.queryKey`, so
// the live events (src/live, T3) can invalidate by prefix and by name without knowing a screen.
//
// Rules the keys follow:
//  - the first element is the root; the rest are exactly what the request depends on, in a fixed order, so two screens that
//    ask for the same thing share one cache entry;
//  - a list key holds the filters and never the cursor: paging happens inside one entry (src/lib/query/cursor-pages.ts), so
//    a new page never makes a new key;
//  - a key built from a content hash is a pure function of immutable bytes. `content` and `blob` are the only such roots.
//    They never go stale, so they are never invalidated and never refetched on a resync (isImmutableKey);
//  - `compare` and `content-ref` are NOT immutable: they are keyed by refs (`nfs:<swimlane>`, `git:base@main`), and a ref
//    moves. They are revalidated by ETag in the fetch layer and invalidated by `file_observed`.
//
// Values are plain strings, numbers, booleans, null and arrays or objects of them, so every key survives JSON and hashes
// the same whatever the order of an object's properties.

import type {
  AdminListAuditParams,
  DriftState,
  ListDocsParams,
  ListFindingsParams,
  ListPrsParams,
  ListProposalsParams,
  GetFeatureStatusParams,
} from "@/api/generated/model";

type WithoutCursor<T> = Omit<T, "cursor">;

/** The roots whose keys are functions of a content hash. */
export const IMMUTABLE_ROOTS: ReadonlySet<string> = new Set(["content", "blob"]);

export const keys = {
  me: () => ["me"] as const,
  swimlanes: () => ["swimlanes"] as const,
  swimlane: (id: string) => ["swimlane", id] as const,
  pickup: (id: string) => ["pickup", id] as const,
  /** One page-set of a folder. `drift` is the drift filter, null for all entries. */
  tree: (id: string, prefix: string, drift: DriftState | null) => ["tree", id, prefix, drift] as const,
  fileHead: (id: string, path: string) => ["file-head", id, path] as const,
  /** A file's content at one hash (served by `getFileContent` with `hash`). Immutable. */
  content: (id: string, path: string, hash: string) => ["content", id, path, hash] as const,
  /** A blob by its hash. Immutable. */
  blob: (hash: string) => ["blob", hash] as const,
  contentRef: (id: string, path: string, repo: string, ref: string) => ["content-ref", id, path, repo, ref] as const,
  compare: (left: string, right: string, file: string, channel: string | null) => ["compare", left, right, file, channel] as const,
  history: (id: string, path: string) => ["history", id, path] as const,
  /** A copy of `lanes`: a caller that later changes its array cannot change a key that was already made. */
  grid: (file: string, lanes: readonly string[], onlyDiff: boolean, channel: string | null) => ["grid", file, [...lanes], onlyDiff, channel] as const,
  compareTree: (left: string, right: string, prefix: string) => ["compare-tree", left, right, prefix] as const,
  gitRefs: (repo: string, kind: string, q: string) => ["git-refs", repo, kind, q] as const,
  findings: (params: WithoutCursor<ListFindingsParams>) => ["findings", params] as const,
  proposals: (params: WithoutCursor<ListProposalsParams>) => ["proposals", params] as const,
  proposal: (id: string) => ["proposal", id] as const,
  prs: (params: WithoutCursor<ListPrsParams>) => ["prs", params] as const,
  prJob: (id: string) => ["pr-job", id] as const,
  docs: (params: WithoutCursor<ListDocsParams>) => ["docs", params] as const,
  doc: (id: string) => ["doc", id] as const,
  features: (params: WithoutCursor<GetFeatureStatusParams>) => ["features", params] as const,
  audit: (params: WithoutCursor<AdminListAuditParams>) => ["audit", params] as const,
  activity: (id: string) => ["activity", id] as const,
  meGithubCredential: () => ["me-github-credential"] as const,
  meMcpTokens: () => ["me-mcp-tokens"] as const,
  /** Every admin list: `section` is the screen (`users`, `agents`, `access-requests`, ...). */
  admin: (section: string, params: Readonly<Record<string, unknown>>) => ["admin", section, params] as const,
} as const;

export type QueryKey = readonly unknown[];

/** The first element of a key: the name of the cache it belongs to, or an empty string for a malformed key. */
export function keyRoot(key: QueryKey): string {
  const root = key[0];
  return typeof root === "string" ? root : "";
}

/** True for a key that is a function of a content hash alone: never stale, never refetched on a resync. */
export function isImmutableKey(key: QueryKey): boolean {
  return IMMUTABLE_ROOTS.has(keyRoot(key));
}
