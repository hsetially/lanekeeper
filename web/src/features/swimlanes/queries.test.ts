import { partialMatchKey } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EVENT_FED } from "@/lib/query/freshness";
import { swimlaneKeys } from "@/lib/query/keys";
import { swimlaneDriftQuery } from "./queries";

// The one query this slice adds to the colocated module: the Drift tab's list. The others are covered in src/features/queries.test.ts.

type Call = { url: URL; init: RequestInit };
let calls: Call[];

beforeEach(() => {
  calls = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      calls.push({ url: new URL(input, "http://hub.test"), init });
      return Promise.resolve(new Response(JSON.stringify({ items: [], next: null }), { status: 200, headers: { "content-type": "application/json" } }));
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("swimlaneDriftQuery", () => {
  it("asks listDrift for the swimlane, with no filter for all states", async () => {
    const options = swimlaneDriftQuery("sitb", null);
    await options.fetchPage(undefined, new AbortController().signal);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.url.pathname).toBe("/api/v1/swimlanes/sitb/drift");
    expect([...(calls[0]?.url.searchParams.keys() ?? [])]).toEqual([]);
  });

  it("filters by one state and carries the cursor of the next page", async () => {
    const options = swimlaneDriftQuery("sitb", "nfs_ahead");
    await options.fetchPage("c1", new AbortController().signal);
    expect(calls[0]?.url.searchParams.get("state")).toBe("nfs_ahead");
    expect(calls[0]?.url.searchParams.get("cursor")).toBe("c1");
  });

  it("is event-fed, and sits under the swimlane's key so that the events that refresh the swimlane refresh it", () => {
    const options = swimlaneDriftQuery("sitb", "conflict");
    expect(options.staleTime).toBe(EVENT_FED);
    expect(options.queryKey).toEqual(swimlaneKeys.drift("sitb", "conflict"));
    expect(partialMatchKey(options.queryKey, swimlaneKeys.detail("sitb"))).toBe(true);
    expect(partialMatchKey(options.queryKey, swimlaneKeys.detail("sit"))).toBe(false);
  });

  it("keeps one entry per swimlane and per state", () => {
    expect(swimlaneDriftQuery("sitb", null).queryKey).not.toEqual(swimlaneDriftQuery("sitb", "conflict").queryKey);
    expect(swimlaneDriftQuery("sitb", null).queryKey).not.toEqual(swimlaneDriftQuery("sitc", null).queryKey);
  });
});
