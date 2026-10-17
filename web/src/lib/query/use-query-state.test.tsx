import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiProblem } from "@/api/problem";
import type { Me } from "@/api/generated/model";
import { SessionContext, type SessionState } from "@/auth/session";
import { LiveProvider } from "@/live/LiveProvider";
import { OFFLINE_AFTER_FAILURES } from "@/live/sse-client";
import { FakeHub } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";
import { cursorPagesSource, isRetryableProblem, useQueryState, type QuerySource, type QueryState } from "./use-query-state";

// The live engine is a chunk of its own, imported when a signed-in user with a role is on screen. Warm in the module cache, so
// that the import resolves in the next turn of the event loop that the fake timers yield.
beforeAll(async () => {
  await import("@/live/live");
});

// One hook that says what a screen should show for a query, the same way everywhere: loading, error, empty or ready as the
// STATE (what replaces the content), and two overlays that never replace it: `refreshing` (a background refetch) and `stale`
// (the data may be out of date, and why). The design's States index is the vocabulary: Loading, Empty, Error, and the
// "data as of" stamp of the offline and paused rows.

const problem = (status: number, kind = "x") => new ApiProblem({ status, kind, title: `t${status}` });

let client: QueryClient;
let calls: Array<{ resolve: (v: number[]) => void; reject: (e: unknown) => void }>;
const queryFn = () => new Promise<number[]>((resolve, reject) => calls.push({ resolve, reject }));

beforeEach(() => {
  vi.useFakeTimers();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  calls = [];
});
afterEach(() => {
  client.clear();
  vi.useRealTimers();
});

function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function setup(options: { staleTime?: number; isEmpty?: (d: number[]) => boolean; refreshDelayMs?: number } = {}) {
  const states: Array<QueryState<number[]>["state"]> = [];
  const hook = renderHook(
    () => {
      const query = useQuery<number[], ApiProblem>({ queryKey: ["zz-test"], queryFn, staleTime: options.staleTime ?? Infinity });
      const view = useQueryState(query, { ...(options.isEmpty ? { isEmpty: options.isEmpty } : {}), ...(options.refreshDelayMs === undefined ? {} : { refreshDelayMs: options.refreshDelayMs }) });
      states.push(view.state);
      return view;
    },
    { wrapper },
  );
  return { ...hook, states };
}
const flush = () =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
const advance = (ms: number) =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
const answer = async (value: number[]) => {
  calls.at(-1)?.resolve(value);
  await flush();
};
const fail = async (error: ApiProblem) => {
  calls.at(-1)?.reject(error);
  await flush();
};

describe("the four states", () => {
  it("is loading until there is data, and has no overlay while it is", async () => {
    const { result } = setup();
    await flush();
    expect(result.current).toMatchObject({ state: "loading", data: undefined, refreshing: false, stale: null, error: null });
  });

  it("is ready with the data once it arrives", async () => {
    const { result } = setup();
    await flush();
    await answer([1, 2]);
    expect(result.current).toMatchObject({ state: "ready", data: [1, 2], refreshing: false, stale: null });
  });

  it("is empty when the caller says the data has nothing in it, and ready when it has something", async () => {
    const { result } = setup({ isEmpty: (d) => d.length === 0 });
    await flush();
    await answer([]);
    expect(result.current.state).toBe("empty");
    expect(result.current.data).toEqual([]);
    act(() => {
      void client.invalidateQueries({ queryKey: ["zz-test"] });
    });
    await flush();
    await answer([1]);
    expect(result.current.state).toBe("ready");
  });

  it("is an error when the first load failed, with the problem, and says whether trying again can help", async () => {
    const { result } = setup();
    await flush();
    await fail(problem(503));
    expect(result.current).toMatchObject({ state: "error", data: undefined, retryable: true });
    expect(result.current.error?.status).toBe(503);
    expect(result.current.stale).toBeNull();
  });

  it("is an error that cannot be retried for an answer that retrying will not change (403, 404)", async () => {
    for (const status of [403, 404]) {
      client.clear();
      calls = [];
      const { result, unmount } = setup();
      await flush();
      await fail(problem(status));
      expect(result.current).toMatchObject({ state: "error", retryable: false });
      unmount();
    }
  });
});

describe("what is retryable", () => {
  it.each([
    [0, "network", true],
    [0, "timeout", true],
    [0, "aborted", false],
    [429, "too_many_requests", true],
    [500, "internal", true],
    [502, "x", true],
    [503, "x", true],
    [504, "x", true],
    [400, "x", false],
    [401, "x", false],
    [403, "x", false],
    [404, "x", false],
    [409, "x", false],
    [422, "x", false],
  ])("status %i (%s): %s", (status, kind, expected) => {
    expect(isRetryableProblem(problem(status, kind))).toBe(expected);
  });
});

describe("a refetch never replaces the content", () => {
  it("never goes back to loading while there is data, and shows `refreshing` only after the delay (no flash for a fast one)", async () => {
    const { result, states } = setup();
    await flush();
    await answer([1]);
    states.length = 0;
    act(() => result.current.refetch());
    await flush();
    expect(result.current.state).toBe("ready");
    expect(result.current.refreshing).toBe(false); // a refetch that is still young shows nothing
    await advance(399);
    expect(result.current.refreshing).toBe(false);
    await advance(2);
    expect(result.current.refreshing).toBe(true);
    expect(result.current.state).toBe("ready");
    await answer([2]);
    expect(result.current.refreshing).toBe(false);
    expect(result.current.data).toEqual([2]);
    expect(states).not.toContain("loading");
  });

  it("shows nothing at all for a refetch that finishes inside the delay", async () => {
    const { result } = setup();
    await flush();
    await answer([1]);
    act(() => result.current.refetch());
    await advance(100);
    await answer([2]);
    await advance(1000);
    expect(result.current.refreshing).toBe(false);
  });

  it("can be told a different delay", async () => {
    const { result } = setup({ refreshDelayMs: 0 });
    await flush();
    await answer([1]);
    act(() => result.current.refetch());
    await advance(1);
    expect(result.current.refreshing).toBe(true);
  });
});

describe("stale: the data may be out of date", () => {
  it("is not stale while it is fresh, and is once it is older than its staleTime (and says when it is from)", async () => {
    const { result } = setup({ staleTime: 1000 });
    await flush();
    await answer([1]);
    const updatedAt = Date.now();
    expect(result.current.stale).toBeNull();
    // TanStack marks the entry stale one millisecond after its staleTime, and tells React on the next tick.
    await advance(1020);
    expect(result.current.stale).toEqual({ causes: ["age"], asOf: updatedAt });
  });

  it("is never stale by age for an entry that has no staleTime (events keep it fresh)", async () => {
    const { result } = setup({ staleTime: Infinity });
    await flush();
    await answer([1]);
    await advance(10 * 60_000);
    expect(result.current.stale).toBeNull();
  });

  it("is not stale by age while it is being refetched: that is `refreshing`, not a banner", async () => {
    const { result } = setup({ staleTime: 1000, refreshDelayMs: 0 });
    await flush();
    await answer([1]);
    await advance(1020);
    expect(result.current.stale?.causes).toEqual(["age"]);
    act(() => result.current.refetch());
    await advance(1);
    expect(result.current.stale).toBeNull();
    expect(result.current.refreshing).toBe(true);
    await answer([2]);
    expect(result.current.stale).toBeNull();
  });

  it("is stale with the data kept when a refetch FAILS: the screen shows the old data, not an error, and says it could not refresh", async () => {
    const { result } = setup();
    await flush();
    await answer([1]);
    act(() => result.current.refetch());
    await flush();
    await fail(problem(503));
    expect(result.current.state).toBe("ready");
    expect(result.current.data).toEqual([1]);
    expect(result.current.error?.status).toBe(503);
    expect(result.current.stale?.causes).toEqual(["refetch-failed"]);
    expect(result.current.stale?.asOf).not.toBeNull();
  });

  it("goes back to fresh when the retry works: loading, ready, (refetch fails) ready+stale, (retry works) ready", async () => {
    const { result, states } = setup();
    await flush();
    await answer([1]);
    act(() => result.current.refetch());
    await flush();
    await fail(problem(500));
    expect(result.current.stale).not.toBeNull();
    act(() => result.current.refetch());
    await flush();
    await answer([2]);
    expect(result.current).toMatchObject({ state: "ready", data: [2], error: null, stale: null });
    expect(new Set(states)).toEqual(new Set(["loading", "ready"]));
  });
});

describe("stale: the live stream is down", () => {
  const ME = { id: "u1", email: "a@example.test", display_name: "A", role: "viewer" } as unknown as Me;
  const session: SessionState = { status: "ready", me: ME, role: "viewer" };
  let hub: FakeHub;

  function liveWrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>
        <SessionContext.Provider value={session}>
          <LiveProvider config={{ stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } }}>{children}</LiveProvider>
        </SessionContext.Provider>
      </QueryClientProvider>
    );
  }

  it("is stale with the cause `live-paused` once the stream is offline, and fresh again when it is back; events could have been missed", async () => {
    hub = new FakeHub();
    const { result } = renderHook(
      () => {
        const query = useQuery<number[], ApiProblem>({ queryKey: ["zz-test"], queryFn, staleTime: Infinity });
        return useQueryState(query);
      },
      { wrapper: liveWrapper },
    );
    await advance(1);
    await answer([1]);
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await vi.advanceTimersByTimeAsync(0);
    });
    // The first connect refetches what is on screen (nothing to resume from): let it answer.
    await advance(150);
    await answer([1]);
    expect(result.current.stale).toBeNull();
    for (let i = 0; i < OFFLINE_AFTER_FAILURES; i++) {
      await act(async () => {
        hub.last.fail();
        await vi.advanceTimersByTimeAsync(0);
      });
      if (i < OFFLINE_AFTER_FAILURES - 1) await advance(20_000);
    }
    expect(result.current.state).toBe("ready");
    expect(result.current.stale?.causes).toEqual(["live-paused"]);
    await advance(20_000);
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await vi.advanceTimersByTimeAsync(0);
    });
    await advance(150);
    await answer([1]);
    expect(result.current.stale).toBeNull();
  });

  it("does not turn a screen with no data into a stale one: loading and error stay what they are", async () => {
    hub = new FakeHub();
    const { result } = renderHook(
      () => {
        const query = useQuery<number[], ApiProblem>({ queryKey: ["zz-test"], queryFn, staleTime: Infinity });
        return useQueryState(query);
      },
      { wrapper: liveWrapper },
    );
    await advance(1);
    for (let i = 0; i < OFFLINE_AFTER_FAILURES; i++) {
      await act(async () => {
        hub.last.fail();
        await vi.advanceTimersByTimeAsync(0);
      });
      if (i < OFFLINE_AFTER_FAILURES - 1) await advance(20_000);
    }
    expect(result.current).toMatchObject({ state: "loading", stale: null });
  });
});

describe("a source that is not a query (the cursor pages of a list)", () => {
  const pages = (patch: Partial<Parameters<typeof cursorPagesSource<number>>[0]>) =>
    cursorPagesSource<number>({ items: [1, 2], status: "success", error: null, loadMoreError: null, hasMore: false, truncated: false, isFetching: false, dataUpdatedAt: 5000, isStale: false, loadMore: () => Promise.resolve(), refetch: () => Promise.resolve(), ...patch });

  it("has its items as data once a page has arrived, and none before", () => {
    expect(pages({}).data).toEqual([1, 2]);
    expect(pages({ status: "pending", items: [] }).data).toBeUndefined();
    expect(pages({ status: "error", items: [], error: problem(503) }).data).toBeUndefined();
  });

  it("reports the first page's error as the screen's error, and a later page's error as a failed refresh of what is shown", () => {
    expect(pages({ status: "error", items: [], error: problem(503) }).error?.status).toBe(503);
    expect(pages({ loadMoreError: problem(500) }).error?.status).toBe(500);
    expect(pages({ loadMoreError: problem(500) }).data).toEqual([1, 2]);
  });

  it("carries when it was fetched and whether it is stale", () => {
    const source: QuerySource<number[]> = pages({ dataUpdatedAt: 5000, isStale: true });
    expect(source.dataUpdatedAt).toBe(5000);
    expect(source.isStale).toBe(true);
  });
});
