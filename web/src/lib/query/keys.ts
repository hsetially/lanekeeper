// The vocabulary of query keys, one factory per domain: every cache of the app is named here and nowhere else (plan, "Query
// keys, retry and SSE invalidation"). The colocated query functions (src/features/<domain>/queries.ts), the live events
// (src/live) and the write invalidation table (src/lib/query/invalidate.ts) all build keys with these factories and with
// nothing else: a lint rule bans a literal array as a key, and src/lib/query/keys-usage.test.ts scans the source for the
// ways around it. The point is that an event that says "swimlane sit1 changed" finds exactly the caches a screen made for
// it, whichever screen made them.
//
// Rules the keys follow:
//  - the first element is the root; the rest are exactly what the request depends on, in a fixed order, so two screens that
//    ask for the same thing share one cache entry;
//  - a list key holds the filters and never the cursor: paging happens inside one entry (src/lib/query/cursor-pages.ts), so
//    a new page never makes a new key;
//  - a key built from a content hash is a pure function of immutable bytes. `content` and `blob` are the only such roots.
//    They never go stale, so they are never invalidated and never refetched on a resync (isImmutableKey);
//  - `compare` and `content-ref` are NOT immutable: they are keyed by refs (`nfs:<swimlane>`, `git:base@main`), and a ref
//    moves. They are revalidated by ETag in the fetch layer and invalidated by `file_observed`;
//  - an id is a string in a key, whether it came from a route (a string) or from an event (a number): `proposal(12)` and
//    `proposal("12")` are one key.
//
// Values are plain strings, numbers, booleans, null and arrays or objects of them, so every key survives JSON and hashes
// the same whatever the order of an object's properties.
//
// The `matchers` at the end know the shape of the keys above, so that the code that invalidates does not have to: "every tree
// page that lists a folder above this path" is a question about the tree key, and the tree key is defined here.

import type {
  AdminListAuditParams,
  DriftState,
  GetFeatureStatusParams,
  ListDocsParams,
  ListFindingsParams,
  ListPrsParams,
  ListProposalsParams,
} from "@/api/generated/model";

type WithoutCursor<T> = Omit<T, "cursor">;
type Id = string | number;

export type QueryKey = readonly unknown[];

/** The roots whose keys are functions of a content hash. */
export const IMMUTABLE_ROOTS: ReadonlySet<string> = new Set(["content", "blob"]);

/** The signed-in user and what belongs to them alone. */
export const meKeys = {
  all: () => ["me"] as const,
  githubCredential: () => ["me-github-credential"] as const,
  mcpTokens: () => ["me-mcp-tokens"] as const,
};

/** Swimlanes: the list, one swimlane, its services' pickup state, its activity. */
export const swimlaneKeys = {
  all: () => ["swimlanes"] as const,
  /** The prefix of every swimlane's detail. */
  details: () => ["swimlane"] as const,
  detail: (id: string) => ["swimlane", id] as const,
  pickup: (id: string) => ["pickup", id] as const,
  activity: (id: string) => ["activity", id] as const,
};

/** Files of one swimlane: the tree, the head of a file, its content by hash, a git read, its history. */
export const fileKeys = {
  /** The prefix of every tree page-set of a swimlane. */
  trees: (id: string) => ["tree", id] as const,
  /** One page-set of a folder. `prefix` is the folder, `""` for the root; `drift` is the drift filter, null for all entries. */
  tree: (id: string, prefix: string, drift: DriftState | null) => ["tree", id, prefix, drift] as const,
  /** The prefix of every file head of a swimlane. */
  heads: (id: string) => ["file-head", id] as const,
  head: (id: string, path: string) => ["file-head", id, path] as const,
  /** A file's content at one hash (served by `getFileContent` with `source=blob` and `hash`). Immutable. */
  content: (id: string, path: string, hash: string) => ["content", id, path, hash] as const,
  /** A blob by its hash. Immutable. */
  blob: (hash: string) => ["blob", hash] as const,
  /** A file read from git at a ref. A ref moves, so this is not immutable. */
  contentRef: (id: string, path: string, repo: string, ref: string) => ["content-ref", id, path, repo, ref] as const,
  history: (id: string, path: string) => ["history", id, path] as const,
};

/** Comparisons: one file between two sides, the grid across swimlanes, the tree of a comparison. */
export const compareKeys = {
  diffs: () => ["compare"] as const,
  diff: (left: string, right: string, file: string, channel: string | null) => ["compare", left, right, file, channel] as const,
  grids: () => ["grid"] as const,
  /** A copy of `lanes`: a caller that later changes its array cannot change a key that was already made. */
  grid: (file: string, lanes: readonly string[], onlyDiff: boolean, channel: string | null) => ["grid", file, [...lanes], onlyDiff, channel] as const,
  trees: () => ["compare-tree"] as const,
  tree: (left: string, right: string, prefix: string) => ["compare-tree", left, right, prefix] as const,
};

/** The branches and tags of the two repositories. */
export const gitKeys = {
  repo: (repo: string) => ["git-refs", repo] as const,
  refs: (repo: string, kind: string, q: string) => ["git-refs", repo, kind, q] as const,
};

export const findingKeys = {
  all: () => ["findings"] as const,
  list: (params: WithoutCursor<ListFindingsParams>) => ["findings", params] as const,
};

/** Change proposals (the approval queue). */
export const proposalKeys = {
  lists: () => ["proposals"] as const,
  list: (params: WithoutCursor<ListProposalsParams>) => ["proposals", params] as const,
  detail: (id: Id) => ["proposal", String(id)] as const,
};

/** Pull-request links and the jobs that open them. */
export const prKeys = {
  lists: () => ["prs"] as const,
  list: (params: WithoutCursor<ListPrsParams>) => ["prs", params] as const,
  job: (id: Id) => ["pr-job", String(id)] as const,
};

/** The documents, and the feature status that depends on them. */
export const docKeys = {
  lists: () => ["docs"] as const,
  list: (params: WithoutCursor<ListDocsParams>) => ["docs", params] as const,
  detail: (id: Id) => ["doc", String(id)] as const,
  featureStatuses: () => ["features"] as const,
  features: (params: WithoutCursor<GetFeatureStatusParams>) => ["features", params] as const,
};

export const auditKeys = {
  all: () => ["audit"] as const,
  list: (params: WithoutCursor<AdminListAuditParams>) => ["audit", params] as const,
};

/** Every admin list: `section` is the screen (`users`, `agents`, `access-requests`, ...). */
export const ADMIN_SECTIONS = [
  "users",
  "access-requests",
  "agents",
  "swimlanes",
  "tenants",
  "auto-notify",
  "folder-mappings",
  "host-tiers",
  "merge-modes",
  "notifications",
  "os-login-users",
  "path-mappings",
  "sentinels",
  "severity-rules",
  "retention",
] as const;
export type AdminSection = (typeof ADMIN_SECTIONS)[number];

export const adminKeys = {
  all: () => ["admin"] as const,
  section: (section: AdminSection) => ["admin", section] as const,
  list: (section: AdminSection, params: Readonly<Record<string, unknown>>) => ["admin", section, params] as const,
};

/** Every domain, for the places that need to walk them all (the usage test). */
export const keyFactories = { meKeys, swimlaneKeys, fileKeys, compareKeys, gitKeys, findingKeys, proposalKeys, prKeys, docKeys, auditKeys, adminKeys } as const;

/** The first element of a key: the name of the cache it belongs to, or an empty string for a malformed key. */
export function keyRoot(key: QueryKey): string {
  const root = key[0];
  return typeof root === "string" ? root : "";
}

/** True for a key that is a function of a content hash alone: never stale, never refetched on a resync. */
export function isImmutableKey(key: QueryKey): boolean {
  return IMMUTABLE_ROOTS.has(keyRoot(key));
}

const SIDE = /^(?:nfs|baseline|effective):(.+)$/;

/** True when `prefix` is the folder of `path` or any folder above it (`""` is the root and is above everything). */
function isFolderOnTheWayTo(prefix: string, path: string): boolean {
  const folder = prefix.replace(/\/+$/, "");
  return folder === "" || path === folder || path.startsWith(`${folder}/`);
}

function sideNames(side: unknown, swimlane: string): boolean {
  if (typeof side !== "string") return false;
  return SIDE.exec(side)?.[1] === swimlane;
}

/**
 * Questions the live events and the writes ask about keys. Each takes a key of any shape and says false for one that is
 * not what its root promises, so a malformed entry is never invalidated by accident and never throws.
 */
export const matchers = {
  /** A tree page-set of `swimlane` that lists a folder on the way to `path`: the folder that holds it, and every one above. */
  treeCovers(key: QueryKey, swimlane: string, path: string): boolean {
    if (key[0] !== "tree" || key[1] !== swimlane) return false;
    const prefix = key[2];
    // A tree key without its folder is not one of ours; refetching it is the safe reading.
    return typeof prefix === "string" ? isFolderOnTheWayTo(prefix, path) : true;
  },
  /** A `compare` or `compare-tree` entry with `swimlane` on either side, read from NFS, its baseline or its effective config. */
  comparesSwimlane(key: QueryKey, swimlane: string): boolean {
    if (key[0] !== "compare" && key[0] !== "compare-tree") return false;
    return sideNames(key[1], swimlane) || sideNames(key[2], swimlane);
  },
  /** A grid that has `swimlane` among its columns. */
  gridHasLane(key: QueryKey, swimlane: string): boolean {
    if (key[0] !== "grid") return false;
    const lanes = key[2];
    return Array.isArray(lanes) && lanes.includes(swimlane);
  },
  /** A git read of `repo` at the ref `branch`. */
  contentRefOnBranch(key: QueryKey, repo: string, branch: string): boolean {
    return key[0] === "content-ref" && key[3] === repo && key[4] === branch;
  },
} as const;
