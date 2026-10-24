import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { StrictMode, type ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { createQueryClient } from "@/lib/query/client";
import { compareKeys } from "@/lib/query/keys";
import { InvalidationBatcher } from "@/live/apply";
import { plan } from "@/live/invalidation";
import { invalidateTargets } from "@/lib/query/targets";
import { gridServer, stubHub } from "../test-utils";
import { useGridPages, type GridSpec } from "./useGridPages";

// The grid's pages and what a live event does to them. A grid of 5,000 rows is eleven pages (50, then 500 each); the live
// layer invalidates a grid when a swimlane in its columns changes, and an invalidated infinite query refetches EVERY page it
// holds. Eleven requests, one after the other, for one event, would be the cost of keeping the screen right. So the grid
// reads again only what the person can see: the first page when they are at the top, as many as reach the last row in view
// when they are further down, and drops the rest (they are read again when the person scrolls to them).

const LANES = ["sit", "sitb", "sitc"];
const SPEC: GridSpec = { file: "app/core.yml", lanes: LANES, onlyDiff: false, channel: null };

let client: QueryClient;
function setup(options: { strict?: boolean } = {}) {
  client = createQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => {
    const tree = <QueryClientProvider client={client}>{children}</QueryClientProvider>;
    return options.strict ? <StrictMode>{tree}</StrictMode> : tree;
  };
  return { wrapper };
}
const key = (spec: GridSpec = SPEC) => compareKeys.grid(spec.file, spec.lanes, spec.onlyDiff, spec.channel);
const event = (swimlane: string) => invalidateTargets(client, [{ kind: "grids", swimlane }]);

afterEach(() => {
  client.clear();
});

describe("the pages of the grid", () => {
  it("asks for 50 rows first, with no cursor, and nothing more until the person looks further down", async () => {
    const hub = gridServer(5000, LANES);
    stubHub(hub.handler);
    const { wrapper } = setup();
    const { result } = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    expect(result.current.items).toHaveLength(50);
    expect(result.current.hasMore).toBe(true);
    expect(hub.requests).toEqual([{ limit: 50, cursor: null }]);
  });

  it("asks once for the first page, whatever React does to the effects", async () => {
    const hub = gridServer(5000, LANES);
    stubHub(hub.handler);
    const { wrapper } = setup({ strict: true });
    const { result } = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    expect(hub.requests).toEqual([{ limit: 50, cursor: null }]);
  });

  it("reads the next page, 500 rows, when the person scrolls to within a screen of the end, and not before", async () => {
    const hub = gridServer(5000, LANES);
    stubHub(hub.handler);
    const { wrapper } = setup();
    const { result } = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    act(() => result.current.lookAt(2));
    await waitFor(() => expect(result.current.items).toHaveLength(550));
    expect(hub.requests).toEqual([
      { limit: 50, cursor: null },
      { limit: 500, cursor: "o50" },
    ]);
    // 28 rows in view of 550 loaded: far from the end, so nothing is asked for.
    act(() => result.current.lookAt(30));
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(hub.requests).toHaveLength(2);
    // Near the end of the 550.
    act(() => result.current.lookAt(480));
    await waitFor(() => expect(result.current.items).toHaveLength(1050));
    expect(hub.requests[2]).toEqual({ limit: 500, cursor: "o550" });
  });

  it("loads all 5,000 rows in eleven requests when the person keeps scrolling: 50, and then ten of 500", async () => {
    const hub = gridServer(5000, LANES);
    stubHub(hub.handler);
    const { wrapper } = setup();
    const { result } = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    for (let i = 0; i < 40 && result.current.hasMore; i++) {
      const before = result.current.items.length;
      act(() => result.current.lookAt(before - 1));
      await waitFor(() => expect(result.current.items.length).toBeGreaterThan(before));
    }
    expect(result.current.items).toHaveLength(5000);
    expect(result.current.hasMore).toBe(false);
    expect(hub.requests.map((r) => r.limit)).toEqual([50, ...Array.from({ length: 10 }, () => 500)]);
  });

  it("does not ask again after a page failed: the screen shows the error and the person decides", async () => {
    let calls = 0;
    const hub = gridServer(5000, LANES);
    stubHub((call) => {
      calls += 1;
      return calls === 1 ? hub.handler(call) : new Response("{}", { status: 400, headers: { "content-type": "application/problem+json" } });
    });
    const { wrapper } = setup();
    const { result } = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    act(() => result.current.lookAt(10));
    await waitFor(() => expect(result.current.loadMoreError).not.toBeNull());
    const asked = calls;
    act(() => result.current.lookAt(20));
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(calls).toBe(asked);
    expect(result.current.items).toHaveLength(50);
  });
});

describe("a live event", () => {
  async function loadThreePages() {
    const hub = gridServer(5000, LANES);
    const calls = stubHub(hub.handler);
    const { wrapper } = setup();
    const view = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(view.result.current.status).toBe("success"));
    act(() => view.result.current.lookAt(2));
    await waitFor(() => expect(view.result.current.items).toHaveLength(550));
    act(() => view.result.current.lookAt(480));
    await waitFor(() => expect(view.result.current.items).toHaveLength(1050));
    return { ...view, hub, calls };
  }

  it("reads the first page again and drops the rest when the person is at the top: one request, not three", async () => {
    const { result, hub } = await loadThreePages();
    act(() => result.current.lookAt(5));
    await new Promise((resolve) => setTimeout(resolve, 20));
    hub.requests.length = 0;
    await act(async () => {
      await event("sitb");
    });
    await waitFor(() => expect(result.current.items).toHaveLength(50));
    expect(hub.requests).toEqual([{ limit: 50, cursor: null }]);
    expect(result.current.hasMore).toBe(true);
  });

  it("reads as many pages as reach the last row in view when the person is further down", async () => {
    const { result, hub } = await loadThreePages();
    act(() => result.current.lookAt(700));
    await new Promise((resolve) => setTimeout(resolve, 20));
    hub.requests.length = 0;
    await act(async () => {
      await event("sit");
    });
    await waitFor(() => expect(hub.requests).toHaveLength(3));
    expect(hub.requests.map((r) => r.limit)).toEqual([50, 500, 500]);
    expect(result.current.items).toHaveLength(1050);
  });

  it("keeps showing the rows it has while it reads (the screen does not go back to a skeleton)", async () => {
    const { result } = await loadThreePages();
    act(() => result.current.lookAt(5));
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const hub = gridServer(5000, LANES);
    stubHub(async (call) => {
      await gate;
      return hub.handler(call);
    });
    await act(async () => {
      void event("sit");
      await Promise.resolve();
    });
    await waitFor(() => expect(result.current.isFetching).toBe(true));
    expect(result.current.status).toBe("success");
    expect(result.current.items).toHaveLength(1050);
    release();
    await waitFor(() => expect(result.current.items).toHaveLength(50));
  });

  it("is what the live layer does with a file_observed event of a column's swimlane: its own plan, coalesced by its own batcher", async () => {
    const { result, hub } = await loadThreePages();
    act(() => result.current.lookAt(5));
    await new Promise((resolve) => setTimeout(resolve, 20));
    hub.requests.length = 0;
    const batcher = new InvalidationBatcher(client, { windowMs: 5 });
    batcher.push(plan({ type: "file_observed", swimlane: "sitc", path: "app/core.yml", hash: "a".repeat(64), source: "scan" }));
    batcher.push(plan({ type: "drift_changed", swimlane: "sit", paths: ["app/core.yml"] }));
    await waitFor(() => expect(result.current.items).toHaveLength(50));
    batcher.dispose();
    // Two events, two swimlanes of the grid, one window: one read of one page.
    expect(hub.requests).toEqual([{ limit: 50, cursor: null }]);
  });

  /** The grid requests the stub has seen, as the server saw them: gated ones are in the list from the moment they are made. */
  const gridAsks = (calls: readonly { url: URL }[]): Array<{ limit: number | null; cursor: string | null }> =>
    calls
      .filter((call) => call.url.pathname === "/api/v1/grid")
      .map((call) => ({ limit: call.url.searchParams.has("limit") ? Number(call.url.searchParams.get("limit")) : null, cursor: call.url.searchParams.get("cursor") }));

  it("does not cancel a Load more that is out when an event comes: the page is read, and the refresh that follows keeps the rows the person asked for", async () => {
    const hub = gridServer(5000, LANES);
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const calls = stubHub(async (call) => {
      if (call.url.searchParams.get("cursor") === "o50") await gate;
      return hub.handler(call);
    });
    const { wrapper } = setup();
    const { result } = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    // The button under the rows: the next 500, asked for by the person and not by scrolling.
    let loading: Promise<void> | undefined;
    act(() => {
      loading = result.current.loadMore();
    });
    await waitFor(() => expect(gridAsks(calls)).toHaveLength(2));
    // The stream's event (or its first connection) comes while the page is on its way.
    await act(async () => {
      await event("sitb");
    });
    release();
    await act(async () => {
      await loading;
    });
    await waitFor(() => expect(result.current.items).toHaveLength(550));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(result.current.items).toHaveLength(550);
    expect(result.current.hasMore).toBe(true);
    // The rows were read again after the event (the first page, then the page below it), so none of them is older than the event.
    expect(gridAsks(calls).slice(2)).toEqual([
      { limit: 50, cursor: null },
      { limit: 500, cursor: "o50" },
    ]);
  });

  it("does not cancel a refresh that is out when Load more is pressed: the page is asked for after it", async () => {
    const hub = gridServer(5000, LANES);
    let hold = false;
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const calls = stubHub(async (call) => {
      if (hold && call.url.searchParams.get("cursor") === null) await gate;
      return hub.handler(call);
    });
    const { wrapper } = setup();
    const { result } = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    hold = true;
    const before = gridAsks(calls).length;
    await act(async () => {
      void event("sit");
      await Promise.resolve();
    });
    await waitFor(() => expect(gridAsks(calls)).toHaveLength(before + 1));
    let loading: Promise<void> | undefined;
    act(() => {
      loading = result.current.loadMore();
    });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(gridAsks(calls)).toHaveLength(before + 1);
    release();
    await act(async () => {
      await loading;
    });
    await waitFor(() => expect(result.current.items).toHaveLength(550));
    expect(gridAsks(calls).slice(before).map((r) => r.cursor)).toEqual([null, "o50"]);
  });

  it("does nothing for a swimlane that is not a column of the grid", async () => {
    const { result, hub } = await loadThreePages();
    hub.requests.length = 0;
    await act(async () => {
      await event("presita");
    });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(hub.requests).toEqual([]);
    expect(result.current.items).toHaveLength(1050);
  });

  it("is the same for a resync, which names every active query", async () => {
    const { result, hub } = await loadThreePages();
    act(() => result.current.lookAt(5));
    await new Promise((resolve) => setTimeout(resolve, 20));
    hub.requests.length = 0;
    await act(async () => {
      await invalidateTargets(client, "everything");
    });
    await waitFor(() => expect(result.current.items).toHaveLength(50));
    expect(hub.requests).toEqual([{ limit: 50, cursor: null }]);
  });

  it("is applied when the grid comes back on screen, if the event came while it was away", async () => {
    const hub = gridServer(5000, LANES);
    stubHub(hub.handler);
    const { wrapper } = setup();
    const first = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(first.result.current.status).toBe("success"));
    act(() => first.result.current.lookAt(2));
    await waitFor(() => expect(first.result.current.items).toHaveLength(550));
    first.unmount();
    await event("sit");
    expect(client.getQueryState(key())?.isInvalidated).toBe(true);
    hub.requests.length = 0;
    const second = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(second.result.current.items).toHaveLength(50));
    expect(hub.requests).toEqual([{ limit: 50, cursor: null }]);
  });

  it("is not applied when the grid comes back and nothing changed: it shows what it has", async () => {
    const hub = gridServer(5000, LANES);
    stubHub(hub.handler);
    const { wrapper } = setup();
    const first = renderHook(() => useGridPages(SPEC), { wrapper });
    await waitFor(() => expect(first.result.current.status).toBe("success"));
    act(() => first.result.current.lookAt(2));
    await waitFor(() => expect(first.result.current.items).toHaveLength(550));
    first.unmount();
    hub.requests.length = 0;
    const second = renderHook(() => useGridPages(SPEC), { wrapper });
    expect(second.result.current.items).toHaveLength(550);
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(hub.requests).toEqual([]);
  });
});
