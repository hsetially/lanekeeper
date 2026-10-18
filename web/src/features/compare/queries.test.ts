import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { apiCaches } from "@/api/client";
import { createQueryClient } from "@/lib/query/client";
import { EVENT_FED } from "@/lib/query/freshness";
import { compareKeys } from "@/lib/query/keys";
import { fileHeadQuery } from "../file/queries";
import { GRID_FIRST_PAGE, GRID_PAGE, GRID_PAGE_SIZES, TREE_PAGE, compareFileQuery, gridQuery, treeComparisonQuery } from "./queries";

// The compare queries that this screen adds to the shared ones (src/features/queries.test.ts covers those): the page sizes of
// the grid, and the tree comparison that keeps the number of subtrees it skipped.

type Call = { url: URL; init: RequestInit };
let calls: Call[];
const signal = () => new AbortController().signal;

beforeEach(() => {
  calls = [];
  apiCaches.clear();
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      calls.push({ url: new URL(input, "http://hub.test"), init });
      const body = input.startsWith("/api/v1/grid")
        ? { swimlanes: ["sit"], rows: { items: [], next: "g2" } }
        : { entries: { items: [{ file: "a.yml", state: "different" }], next: null }, skipped_identical_subtrees: 38 };
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } }));
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the grid's page sizes", () => {
  it("asks for 50 rows first and then 500: one page of 500 rows by 40 swimlanes is 2.7 MB and the hub does not compress", () => {
    expect(GRID_FIRST_PAGE).toBe(50);
    expect(GRID_PAGE).toBe(500);
    expect(GRID_PAGE_SIZES).toEqual({ first: 50, next: 500 });
  });

  it("sends limit=50 with no cursor for the first page and limit=500 with the cursor for every other", async () => {
    const options = gridQuery("a/b.yml", ["sit", "sitb"], true, null, GRID_PAGE_SIZES);
    await options.fetchPage(undefined, signal());
    await options.fetchPage("g2", signal());
    expect(calls[0]?.url.searchParams.get("limit")).toBe("50");
    expect(calls[0]?.url.searchParams.has("cursor")).toBe(false);
    expect(calls[1]?.url.searchParams.get("limit")).toBe("500");
    expect(calls[1]?.url.searchParams.get("cursor")).toBe("g2");
  });

  it("sends the swimlanes comma separated, in the order of the columns, and only what was asked for", async () => {
    await gridQuery("a/b.yml", ["sitb", "sit"], false, "atm", GRID_PAGE_SIZES).fetchPage(undefined, signal());
    const url = calls[0]?.url;
    expect(url?.searchParams.get("swimlanes")).toBe("sitb,sit");
    expect(url?.searchParams.get("file")).toBe("a/b.yml");
    expect(url?.searchParams.get("channel")).toBe("atm");
    expect(url?.searchParams.has("only_differing")).toBe(false);
  });

  it("is keyed by the grid key and is fed by live events", () => {
    const options = gridQuery("a/b.yml", ["sit"], true, null, GRID_PAGE_SIZES);
    expect(options.queryKey).toEqual(compareKeys.grid("a/b.yml", ["sit"], true, null));
    expect(options.staleTime).toBe(EVENT_FED);
  });

  it("keeps the shared form: no page size, no limit", async () => {
    await gridQuery("a/b.yml", ["sit"], true, null).fetchPage(undefined, signal());
    expect(calls[0]?.url.searchParams.has("limit")).toBe(false);
  });
});

describe("a tree comparison", () => {
  it("keeps the count of identical subtrees that the server skipped, with the entries", async () => {
    const page = await treeComparisonQuery("effective:sit", "effective:sitb", "").fetchPage(undefined, signal());
    expect(page.items).toEqual([{ file: "a.yml", state: "different" }]);
    expect(page.next).toBeNull();
    expect(page.skipped).toBe(38);
  });

  it("asks for pages of 500 and gives its folder only when there is one", async () => {
    const options = treeComparisonQuery("nfs:sit", "git:tenant@sit7", "core/");
    await options.fetchPage("t2", signal());
    const url = calls[0]?.url;
    expect(url?.searchParams.get("limit")).toBe(String(TREE_PAGE));
    expect(url?.searchParams.get("prefix")).toBe("core/");
    expect(url?.searchParams.get("cursor")).toBe("t2");
    expect(url?.searchParams.get("left")).toBe("nfs:sit");
    expect(url?.searchParams.get("right")).toBe("git:tenant@sit7");
    await treeComparisonQuery("nfs:sit", "nfs:sitb", "").fetchPage(undefined, signal());
    expect(calls[1]?.url.searchParams.has("prefix")).toBe(false);
    expect(options.queryKey).toEqual(compareKeys.tree("nfs:sit", "git:tenant@sit7", "core/"));
  });
});

describe("a comparison of one file", () => {
  it("is keyed by the two references and the file, never by a hash learned earlier", () => {
    const options = compareFileQuery("effective:sit", "effective:sitb", "a/b.yml", null);
    expect(options.queryKey).toEqual(["compare", "effective:sit", "effective:sitb", "a/b.yml", null]);
    expect(JSON.stringify(options.queryKey)).not.toMatch(/[0-9a-f]{64}/);
  });
});

describe("a file that changes between its head and its comparison", () => {
  it("gives the comparison of the new file, asking with the tag of the old one, whatever hash the head had when the comparison was first read", async () => {
    const hash = (n: number) => `${n}`.repeat(64).slice(0, 64);
    let version = 1;
    const seen: Array<{ path: string; tag: string | null }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string, init: RequestInit = {}) => {
        const url = new URL(input, "http://hub.test");
        const tag = new Headers(init.headers).get("if-none-match");
        seen.push({ path: url.pathname, tag });
        if (url.pathname.endsWith("/files/content")) {
          return Promise.resolve(new Response(JSON.stringify({ path: "a.yml", hash: hash(version), text: `v${version}` }), { status: 200, headers: { "content-type": "application/json", etag: `"head-${version}"` } }));
        }
        const body = { left: "nfs:sitb", right: "git:tenant@sit7", file: "a.yml", class: "structured", identical: false, eol_only: false, hunks: [], setting_changes: [], binary: null, marker: version };
        return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json", etag: `"cmp-${version}"` } }));
      }),
    );
    const client = createQueryClient();
    const head = fileHeadQuery("sitb", "a.yml");
    const compare = { ...compareFileQuery("nfs:sitb", "git:tenant@sit7", "a.yml", null), staleTime: 0 };
    const first = await client.query(head);
    const firstCompare = await client.query(compare);
    expect(first.hash).toBe(hash(1));
    expect((firstCompare as unknown as { marker: number }).marker).toBe(1);

    // The file changes: its head has a new hash, and the comparison is asked for again under the same key.
    version = 2;
    const second = await client.query({ ...head, staleTime: 0 });
    const secondCompare = await client.query(compare);
    expect(second.hash).toBe(hash(2));
    expect((secondCompare as unknown as { marker: number }).marker).toBe(2);
    expect(client.getQueryCache().findAll({ queryKey: compareKeys.diffs() })).toHaveLength(1);
    const compares = seen.filter((c) => c.path === "/api/v1/compare");
    expect(compares.map((c) => c.tag)).toEqual([null, '"cmp-1"']);
  });
});
