import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ApiProblem } from "@/api/problem";
import { useOneShot, usePagedOneShot } from "./one-shot";

// A search is asked once for what is on screen and its answer is kept for as long as that screen shows it and no longer: it is
// not put in the app's query cache, where it would outlive the page (the palette does the same, src/features/shell/palette).

const problem = (status = 500) => new ApiProblem({ status, kind: "internal", title: "Internal error", requestId: "req-1" });
const later = <T,>() => {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
};

describe("useOneShot", () => {
  it("asks for nothing while there is no request", () => {
    const { result } = renderHook(() => useOneShot<string>(null));
    expect(result.current.idle).toBe(true);
  });

  it("asks once for a key and keeps the answer", async () => {
    const load = vi.fn(() => Promise.resolve("answer"));
    const { result, rerender } = renderHook(({ key }) => useOneShot({ key, load }), { initialProps: { key: "a" } });
    expect(result.current.idle).toBe(false);
    expect(result.current.source.status).toBe("pending");
    expect(result.current.source.data).toBeUndefined();
    await waitFor(() => expect(result.current.source.status).toBe("success"));
    expect(result.current.source.data).toBe("answer");
    expect(result.current.source.dataUpdatedAt).toBeGreaterThan(0);
    rerender({ key: "a" });
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("drops the old answer at once when the key changes, aborts the old request and ignores its late answer", async () => {
    const first = later<string>();
    const second = later<string>();
    const signals: AbortSignal[] = [];
    const load = vi.fn((signal: AbortSignal) => {
      signals.push(signal);
      return signals.length === 1 ? first.promise : second.promise;
    });
    const { result, rerender } = renderHook(({ key }) => useOneShot({ key, load }), { initialProps: { key: "a" } });
    rerender({ key: "b" });
    expect(signals[0]?.aborted).toBe(true);
    expect(result.current.source.data).toBeUndefined();
    await act(async () => {
      first.resolve("old");
      await first.promise;
    });
    expect(result.current.source.data).toBeUndefined();
    await act(async () => {
      second.resolve("new");
      await second.promise;
    });
    expect(result.current.source.data).toBe("new");
  });

  it("says what went wrong as a problem, and retries on request", async () => {
    let fail = true;
    const load = vi.fn(() => (fail ? Promise.reject(problem()) : Promise.resolve("ok")));
    const { result } = renderHook(() => useOneShot({ key: "a", load }));
    await waitFor(() => expect(result.current.source.status).toBe("error"));
    expect(result.current.source.error?.requestId).toBe("req-1");
    fail = false;
    act(() => {
      void result.current.source.refetch();
    });
    await waitFor(() => expect(result.current.source.status).toBe("success"));
    expect(result.current.source.data).toBe("ok");
    expect(result.current.source.error).toBeNull();
  });

  it("turns anything that is not a problem into a plain one, without its message", async () => {
    const load = () => Promise.reject(new Error("secret internals"));
    const { result } = renderHook(() => useOneShot({ key: "a", load }));
    await waitFor(() => expect(result.current.source.status).toBe("error"));
    expect(result.current.source.error).toBeInstanceOf(ApiProblem);
    expect(JSON.stringify(result.current.source.error?.title)).not.toContain("secret");
  });

  it("aborts when the screen goes away", () => {
    let signal: AbortSignal | undefined;
    const load = (s: AbortSignal) => {
      signal = s;
      return new Promise<string>(() => undefined);
    };
    const { unmount } = renderHook(() => useOneShot({ key: "a", load }));
    unmount();
    expect(signal?.aborted).toBe(true);
  });

  it("does not ask again when only the function changes, and asks with the latest one when told to", async () => {
    const loads = [vi.fn(() => Promise.resolve(1)), vi.fn(() => Promise.resolve(2))] as const;
    const { result, rerender } = renderHook(({ i }) => useOneShot({ key: "a", load: loads[i] }), { initialProps: { i: 0 as 0 | 1 } });
    await waitFor(() => expect(result.current.source.data).toBe(1));
    rerender({ i: 1 });
    expect(loads[0]).toHaveBeenCalledTimes(1);
    expect(loads[1]).not.toHaveBeenCalled();
    act(() => {
      void result.current.source.refetch();
    });
    await waitFor(() => expect(result.current.source.data).toBe(2));
  });
});

type Page = { items: number[]; next: string | null };
const select = (page: Page) => page;

describe("usePagedOneShot", () => {
  it("loads the first page, then the next on request, and says when there is no more", async () => {
    const fetch = vi.fn((cursor: string | undefined) => Promise.resolve<Page>(cursor === undefined ? { items: [1, 2], next: "c1" } : { items: [3], next: null }));
    const { result } = renderHook(() => usePagedOneShot({ key: "a", fetch, select, maxPages: 5 }));
    await waitFor(() => expect(result.current.items).toEqual([1, 2]));
    expect(result.current.hasMore).toBe(true);
    await act(async () => {
      await result.current.loadMore();
    });
    expect(result.current.items).toEqual([1, 2, 3]);
    expect(result.current.hasMore).toBe(false);
    expect(fetch.mock.calls.map((c) => c[0])).toEqual([undefined, "c1"]);
  });

  it("does not ask for a page that is already on its way, or when there is none", async () => {
    const hold = later<Page>();
    const fetch = vi.fn((cursor: string | undefined) => (cursor === undefined ? Promise.resolve<Page>({ items: [1], next: "c1" }) : hold.promise));
    const { result } = renderHook(() => usePagedOneShot({ key: "a", fetch, select, maxPages: 5 }));
    await waitFor(() => expect(result.current.items).toEqual([1]));
    act(() => {
      void result.current.loadMore();
      void result.current.loadMore();
    });
    expect(fetch).toHaveBeenCalledTimes(2);
    await act(async () => {
      hold.resolve({ items: [2], next: null });
      await hold.promise;
    });
    await act(async () => {
      await result.current.loadMore();
    });
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("stops at the most pages it may ask for, and says the list was cut", async () => {
    const fetch = vi.fn((cursor: string | undefined) => Promise.resolve<Page>({ items: [cursor === undefined ? 0 : Number(cursor)], next: String((cursor === undefined ? 0 : Number(cursor)) + 1) }));
    const { result } = renderHook(() => usePagedOneShot({ key: "a", fetch, select, maxPages: 2 }));
    await waitFor(() => expect(result.current.items).toEqual([0]));
    await act(async () => {
      await result.current.loadMore();
    });
    expect(result.current.items).toEqual([0, 1]);
    expect(result.current.hasMore).toBe(false);
    expect(result.current.truncated).toBe(true);
  });

  it("keeps the rows on screen when a later page fails, and says so", async () => {
    const fetch = vi.fn((cursor: string | undefined) => (cursor === undefined ? Promise.resolve<Page>({ items: [1], next: "c1" }) : Promise.reject(problem(503))));
    const { result } = renderHook(() => usePagedOneShot({ key: "a", fetch, select, maxPages: 5 }));
    await waitFor(() => expect(result.current.items).toEqual([1]));
    await act(async () => {
      await result.current.loadMore();
    });
    expect(result.current.items).toEqual([1]);
    expect(result.current.loadMoreError?.status).toBe(503);
    expect(result.current.source.status).toBe("success");
  });

  it("starts over when the key changes", async () => {
    const fetch = vi.fn((_cursor: string | undefined) => Promise.resolve<Page>({ items: [1], next: null }));
    const { result, rerender } = renderHook(({ key }) => usePagedOneShot({ key, fetch, select, maxPages: 5 }), { initialProps: { key: "a" } });
    await waitFor(() => expect(result.current.items).toEqual([1]));
    rerender({ key: "b" });
    expect(result.current.items).toEqual([]);
    expect(result.current.source.status).toBe("pending");
    await waitFor(() => expect(result.current.items).toEqual([1]));
  });
});
