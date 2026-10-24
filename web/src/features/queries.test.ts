import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CursorPageOptions } from "@/lib/query/cursor-pages";
import { EVENT_FED, IMMUTABLE, IMMUTABLE_GC_MS, POLLED_MS } from "@/lib/query/freshness";
import { adminKeys, auditKeys, compareKeys, docKeys, fileKeys, findingKeys, gitKeys, meKeys, prKeys, proposalKeys, swimlaneKeys } from "@/lib/query/keys";
import { githubCredentialQuery, mcpTokensQuery, meQuery } from "./access/queries";
import { adminQueries } from "./admin/queries";
import { proposalListQuery, proposalQuery } from "./approvals/queries";
import { compareFileQuery, compareTreeQuery, gitRefsQuery, gridQuery } from "./compare/queries";
import { docListQuery } from "./docs/queries";
import { blobQuery, fileContentQuery, fileHeadQuery, fileHistoryQuery, fileRefQuery, fileTreeQuery } from "./file/queries";
import { findingListQuery } from "./findings/queries";
import { prJobQuery, prListQuery } from "./prs/queries";
import { SERVED_GC_MS, servedFileQuery } from "./served/queries";
import { featureStatusQuery } from "./status/queries";
import { swimlaneListQuery, swimlaneQuery } from "./swimlanes/queries";
import { auditListQuery } from "./audit/queries";

// The colocated query modules: for each, the key is the one the key factory makes, the call is the generated operation with
// the right path and parameters (and the abort signal), and the freshness is the rule the module says it follows. The calls
// go through the real generated client and the real fetch layer, to a fetch that records what it was asked.

type Call = { url: URL; init: RequestInit };
let calls: Call[];

beforeEach(() => {
  calls = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      calls.push({ url: new URL(input, "http://hub.test"), init });
      const blob = input.startsWith("/api/v1/blobs/");
      return Promise.resolve(new Response(blob ? "bytes" : "{}", { status: 200, headers: { "content-type": blob ? "application/octet-stream" : "application/json" } }));
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

type Plain = { queryKey: readonly unknown[]; queryFn: (context: never) => Promise<unknown>; staleTime?: unknown; gcTime?: unknown };
const isPaged = (o: unknown): o is CursorPageOptions<unknown, unknown> => typeof o === "object" && o !== null && "fetchPage" in o;

type Case = { name: string; options: () => unknown; key: readonly unknown[]; path: string; query?: Record<string, string | string[]>; staleTime: number; gcTime?: number };

const HASH = "a".repeat(64);
const CASES: Case[] = [
  // me
  { name: "meQuery", options: () => meQuery(), key: meKeys.all(), path: "/api/v1/me", staleTime: POLLED_MS },
  { name: "githubCredentialQuery", options: () => githubCredentialQuery(), key: meKeys.githubCredential(), path: "/api/v1/me/github-token", staleTime: POLLED_MS },
  { name: "mcpTokensQuery", options: () => mcpTokensQuery(), key: meKeys.mcpTokens(), path: "/api/v1/me/mcp-tokens", staleTime: POLLED_MS },
  // swimlanes
  { name: "swimlaneListQuery", options: () => swimlaneListQuery(), key: swimlaneKeys.all(), path: "/api/v1/swimlanes", staleTime: EVENT_FED },
  { name: "swimlaneQuery", options: () => swimlaneQuery("sit1"), key: swimlaneKeys.detail("sit1"), path: "/api/v1/swimlanes/sit1", staleTime: EVENT_FED },
  // files
  { name: "fileTreeQuery (a folder, drift filter)", options: () => fileTreeQuery("sit1", "core", "git_ahead"), key: fileKeys.tree("sit1", "core", "git_ahead"), path: "/api/v1/swimlanes/sit1/tree", query: { prefix: "core", drift: "git_ahead" }, staleTime: EVENT_FED },
  { name: "fileTreeQuery (the root)", options: () => fileTreeQuery("sit1", "", null), key: fileKeys.tree("sit1", "", null), path: "/api/v1/swimlanes/sit1/tree", query: {}, staleTime: EVENT_FED },
  { name: "fileHeadQuery", options: () => fileHeadQuery("sit1", "core/a.yml"), key: fileKeys.head("sit1", "core/a.yml"), path: "/api/v1/swimlanes/sit1/files/content", query: { path: "core/a.yml", source: "nfs" }, staleTime: EVENT_FED },
  { name: "fileContentQuery (immutable)", options: () => fileContentQuery("sit1", "core/a.yml", HASH), key: fileKeys.content("sit1", "core/a.yml", HASH), path: "/api/v1/swimlanes/sit1/files/content", query: { path: "core/a.yml", source: "blob", hash: HASH }, staleTime: IMMUTABLE, gcTime: IMMUTABLE_GC_MS },
  { name: "fileRefQuery", options: () => fileRefQuery("sit1", "core/a.yml", "base", "main"), key: fileKeys.contentRef("sit1", "core/a.yml", "base", "main"), path: "/api/v1/swimlanes/sit1/files/content", query: { path: "core/a.yml", source: "git", repo: "base", ref: "main" }, staleTime: EVENT_FED },
  { name: "blobQuery (immutable)", options: () => blobQuery(HASH), key: fileKeys.blob(HASH), path: `/api/v1/blobs/${HASH}`, staleTime: IMMUTABLE, gcTime: IMMUTABLE_GC_MS },
  { name: "fileHistoryQuery", options: () => fileHistoryQuery("sit1", "core/a.yml"), key: fileKeys.history("sit1", "core/a.yml"), path: "/api/v1/swimlanes/sit1/files/history", query: { path: "core/a.yml" }, staleTime: EVENT_FED },
  // comparisons
  { name: "compareFileQuery", options: () => compareFileQuery("nfs:sit1", "git:base@main", "core/a.yml", null), key: compareKeys.diff("nfs:sit1", "git:base@main", "core/a.yml", null), path: "/api/v1/compare", query: { left: "nfs:sit1", right: "git:base@main", file: "core/a.yml" }, staleTime: EVENT_FED },
  { name: "compareFileQuery (a channel)", options: () => compareFileQuery("nfs:sit1", "nfs:sit2", "f", "ch1"), key: compareKeys.diff("nfs:sit1", "nfs:sit2", "f", "ch1"), path: "/api/v1/compare", query: { left: "nfs:sit1", right: "nfs:sit2", file: "f", channel: "ch1" }, staleTime: EVENT_FED },
  { name: "compareTreeQuery", options: () => compareTreeQuery("nfs:sit1", "nfs:sit2", "core"), key: compareKeys.tree("nfs:sit1", "nfs:sit2", "core"), path: "/api/v1/compare/tree", query: { left: "nfs:sit1", right: "nfs:sit2", prefix: "core" }, staleTime: EVENT_FED },
  // `swimlanes` is "comma separated" in the contract (style form, explode false), and the generated client writes it so.
  { name: "gridQuery", options: () => gridQuery("core.yml", ["sit1", "sit2"], true, null), key: compareKeys.grid("core.yml", ["sit1", "sit2"], true, null), path: "/api/v1/grid", query: { file: "core.yml", swimlanes: "sit1,sit2", only_differing: "true" }, staleTime: EVENT_FED },
  { name: "gitRefsQuery", options: () => gitRefsQuery("base", "tag", "v1"), key: gitKeys.refs("base", "tag", "v1"), path: "/api/v1/git/refs", query: { repo: "base", kind: "tag", q: "v1" }, staleTime: EVENT_FED },
  { name: "gitRefsQuery (no filter)", options: () => gitRefsQuery("tenant", null, ""), key: gitKeys.refs("tenant", "", ""), path: "/api/v1/git/refs", query: { repo: "tenant" }, staleTime: EVENT_FED },
  // findings, proposals, pull requests, documents, status, audit
  { name: "findingListQuery", options: () => findingListQuery({ swimlane: "sit1", limit: 50 }), key: findingKeys.list({ swimlane: "sit1", limit: 50 }), path: "/api/v1/findings", query: { swimlane: "sit1", limit: "50" }, staleTime: EVENT_FED },
  { name: "proposalListQuery", options: () => proposalListQuery({ status: "pending" }), key: proposalKeys.list({ status: "pending" }), path: "/api/v1/proposals", query: { status: "pending" }, staleTime: EVENT_FED },
  { name: "proposalQuery", options: () => proposalQuery(12), key: proposalKeys.detail(12), path: "/api/v1/proposals/12", staleTime: EVENT_FED },
  { name: "prListQuery", options: () => prListQuery({ state: "open" }), key: prKeys.list({ state: "open" }), path: "/api/v1/prs", query: { state: "open" }, staleTime: EVENT_FED },
  { name: "prJobQuery", options: () => prJobQuery(9), key: prKeys.job(9), path: "/api/v1/pr-jobs/9", staleTime: EVENT_FED },
  { name: "docListQuery", options: () => docListQuery({ path_prefix: "guides/" }), key: docKeys.list({ path_prefix: "guides/" }), path: "/api/v1/docs", query: { path_prefix: "guides/" }, staleTime: EVENT_FED },
  { name: "featureStatusQuery", options: () => featureStatusQuery({ flag: "auto-notify" }), key: docKeys.features({ flag: "auto-notify" }), path: "/api/v1/features/status", query: { flag: "auto-notify" }, staleTime: EVENT_FED },
  { name: "auditListQuery (no event names it: polled)", options: () => auditListQuery({ swimlane: "sit1" }), key: auditKeys.list({ swimlane: "sit1" }), path: "/api/v1/admin/audit", query: { swimlane: "sit1" }, staleTime: POLLED_MS },
  { name: "servedFileQuery (a channel)", options: () => servedFileQuery("sit1", { file: "core/a.yml", application: "tx-core", tenant: "sit7", channel: "beta" }), key: swimlaneKeys.served("sit1", "tx-core", "sit7", "beta", "core/a.yml"), path: "/api/v1/swimlanes/sit1/served", query: { application: "tx-core", tenant: "sit7", channel: "beta", file: "core/a.yml" }, staleTime: POLLED_MS, gcTime: SERVED_GC_MS },
  // admin
  { name: "admin users", options: () => adminQueries.users({ q: "ada" }), key: adminKeys.list("users", { q: "ada" }), path: "/api/v1/admin/users", query: { q: "ada" }, staleTime: POLLED_MS },
  { name: "admin access requests (access_request_created)", options: () => adminQueries.accessRequests({ status: "pending" }), key: adminKeys.list("access-requests", { status: "pending" }), path: "/api/v1/admin/access-requests", query: { status: "pending" }, staleTime: EVENT_FED },
  { name: "admin agents (agent_status_changed)", options: () => adminQueries.agents(), key: adminKeys.list("agents", {}), path: "/api/v1/admin/agents", staleTime: EVENT_FED },
  { name: "admin folder mappings", options: () => adminQueries.folderMappings(), key: adminKeys.list("folder-mappings", {}), path: "/api/v1/admin/folder-mappings", staleTime: POLLED_MS },
  { name: "admin host tiers", options: () => adminQueries.hostTiers(), key: adminKeys.list("host-tiers", {}), path: "/api/v1/admin/host-tiers", staleTime: POLLED_MS },
  { name: "admin merge modes", options: () => adminQueries.mergeModes(), key: adminKeys.list("merge-modes", {}), path: "/api/v1/admin/merge-modes", staleTime: POLLED_MS },
  { name: "admin notifications", options: () => adminQueries.notifications(), key: adminKeys.list("notifications", {}), path: "/api/v1/admin/notifications", staleTime: POLLED_MS },
  { name: "admin os login users", options: () => adminQueries.osLoginUsers(), key: adminKeys.list("os-login-users", {}), path: "/api/v1/admin/os-login-users", staleTime: POLLED_MS },
  { name: "admin path mappings", options: () => adminQueries.pathMappings(), key: adminKeys.list("path-mappings", {}), path: "/api/v1/admin/path-mappings", staleTime: POLLED_MS },
  { name: "admin sentinels", options: () => adminQueries.sentinels(), key: adminKeys.list("sentinels", {}), path: "/api/v1/admin/sentinels", staleTime: POLLED_MS },
  { name: "admin severity rules", options: () => adminQueries.severityRules(), key: adminKeys.list("severity-rules", {}), path: "/api/v1/admin/severity-rules", staleTime: POLLED_MS },
  { name: "admin retention", options: () => adminQueries.retention(), key: adminKeys.list("retention", {}), path: "/api/v1/admin/retention", staleTime: POLLED_MS },
  { name: "admin tenants (swimlane_changed)", options: () => adminQueries.tenants("sit1"), key: adminKeys.list("tenants", { id: "sit1" }), path: "/api/v1/admin/swimlanes/sit1/tenants", staleTime: EVENT_FED },
  { name: "admin auto-notify (swimlane_changed)", options: () => adminQueries.autoNotify("sit1"), key: adminKeys.list("auto-notify", { id: "sit1" }), path: "/api/v1/admin/swimlanes/sit1/auto-notify", staleTime: EVENT_FED },
];

function expectQuery(url: URL, expected: Case) {
  expect(url.pathname).toBe(expected.path);
  const want = expected.query ?? {};
  const got = new Map<string, string[]>();
  for (const [k, v] of url.searchParams) got.set(k, [...(got.get(k) ?? []), v]);
  expect([...got.keys()].sort()).toEqual(Object.keys(want).sort());
  for (const [k, v] of Object.entries(want)) expect(got.get(k), k).toEqual(Array.isArray(v) ? v : [v]);
}

describe.each(CASES)("$name", (c) => {
  it("is keyed by the key factory, and reads the operation of the contract with its parameters and the abort signal", async () => {
    const options = c.options();
    const signal = new AbortController().signal;
    if (isPaged(options)) {
      expect(options.queryKey).toEqual(c.key);
      await options.fetchPage(undefined, signal);
    } else {
      const plain = options as Plain;
      expect(plain.queryKey).toEqual(c.key);
      await plain.queryFn({ signal, queryKey: c.key } as never);
    }
    expect(calls).toHaveLength(1);
    expect(calls[0]?.init.method).toBe("GET");
    expect(calls[0]?.init.signal).toBeInstanceOf(AbortSignal);
    expectQuery(calls[0]?.url ?? new URL("http://x"), c);
  });

  it("follows the freshness rule it states", () => {
    const options = c.options() as { staleTime?: number; gcTime?: number };
    expect(options.staleTime).toBe(c.staleTime);
    expect(options.gcTime).toBe(c.gcTime);
  });
});

describe("paged lists", () => {
  it("ask for the next page with the cursor, and the first without one", async () => {
    const options = fileHistoryQuery("sit1", "a.yml");
    await options.fetchPage(undefined, new AbortController().signal);
    await options.fetchPage("c1", new AbortController().signal);
    expect(calls[0]?.url.searchParams.has("cursor")).toBe(false);
    expect(calls[1]?.url.searchParams.get("cursor")).toBe("c1");
  });

  it("share one select, so the items of a list are the same array until a page arrives", () => {
    const a = swimlaneListQuery();
    const b = findingListQuery();
    const c = fileHistoryQuery("sit1", "a.yml");
    expect(a.select).toBe(b.select);
    expect(a.select).toBe(c.select);
    expect(a.select({ items: [], next: "n" })).toEqual({ items: [], next: "n" });
  });

  it("take the rows of a grid and the entries of a tree comparison as their pages", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string) => {
        const body = input.startsWith("/api/v1/grid") ? { swimlanes: ["sit1"], rows: { items: [{ id: 1 }], next: "g2" } } : { entries: { items: [{ id: 2 }], next: null }, skipped_identical_subtrees: 0 };
        return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } }));
      }),
    );
    const grid = await gridQuery("f", ["sit1"], false, null).fetchPage(undefined, new AbortController().signal);
    expect(grid).toEqual({ items: [{ id: 1 }], next: "g2" });
    const tree = await compareTreeQuery("nfs:a", "nfs:b", "").fetchPage(undefined, new AbortController().signal);
    expect(tree).toEqual({ items: [{ id: 2 }], next: null });
  });
});

describe("what each module says about keeping its data", () => {
  it("never makes content or a blob stale, and keeps them longer than a list", () => {
    for (const options of [fileContentQuery("s", "p", HASH), blobQuery(HASH)]) {
      expect(options.staleTime).toBe(Infinity);
      expect(options.gcTime).toBeGreaterThan(5 * 60 * 1000);
    }
  });

  it("gives every query an explicit staleTime: none inherits the client's default by accident", () => {
    for (const c of CASES) {
      const options = c.options() as { staleTime?: unknown };
      expect(typeof options.staleTime, c.name).toBe("number");
    }
  });

  it("only treats a query as event-fed when a live event names it (the audit trail, the user and the admin lists nobody is told about are polled)", () => {
    const polled = CASES.filter((c) => c.staleTime === POLLED_MS).map((c) => c.name);
    expect(polled.some((n) => /audit/.test(n))).toBe(true);
    expect(polled.some((n) => /^meQuery/.test(n))).toBe(true);
    const eventFed = CASES.filter((c) => c.staleTime === EVENT_FED).map((c) => c.name);
    expect(eventFed.some((n) => /audit|^meQuery/.test(n))).toBe(false);
  });
});
