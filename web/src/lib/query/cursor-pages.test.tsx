import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { ApiProblem } from "@/api/problem";
import { createQueryClient } from "./client";
import { useCursorPages, type CursorPageOptions } from "./cursor-pages";

// The paging path of every long list (grid, tree, findings, history, search): the server sends a page of at most 500 and a
// `next` cursor, and the screen decides how many to ask for. These tests use a server that is a function, so the order of
// calls, the cursors sent and the stop conditions are checked, not mocked.

type Page = { items: number[]; next: string | null };

/** A server of `total` items, `size` to a page, whose cursor is the offset spelled as text. */
function server(total: number, size: number) {
  const calls: Array<string | undefined> = [];
  const fetchPage = vi.fn((cursor: string | undefined): Promise<Page> => {
    calls.push(cursor);
    const from = cursor === undefined ? 0 : Number(cursor);
    const items = Array.from({ length: Math.max(0, Math.min(size, total - from)) }, (_, i) => from + i);
    const end = from + items.length;
    return Promise.resolve({ items, next: end < total ? String(end) : null });
  });
  return { calls, fetchPage };
}

function setup() {
  const client: QueryClient = createQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { client, wrapper };
}

const select = (page: Page) => page;
const options = (fetchPage: CursorPageOptions<Page, number>["fetchPage"], extra: Partial<CursorPageOptions<Page, number>> = {}): CursorPageOptions<Page, number> => ({
  queryKey: ["test-pages", "a"],
  fetchPage,
  select,
  ...extra,
});

describe("useCursorPages", () => {
  it("loads the first page, sends no cursor for it, and then the cursor of the page before for the next", async () => {
    const { wrapper } = setup();
    const s = server(25, 10);
    const { result } = renderHook(() => useCursorPages(options(s.fetchPage)), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    expect(result.current.items).toHaveLength(10);
    expect(result.current.hasMore).toBe(true);
    await act(async () => {
      await result.current.loadMore();
    });
    await waitFor(() => expect(result.current.items).toHaveLength(20));
    await act(async () => {
      await result.current.loadMore();
    });
    await waitFor(() => expect(result.current.items).toEqual(Array.from({ length: 25 }, (_, i) => i)));
    expect(result.current.hasMore).toBe(false);
    expect(s.calls).toEqual([undefined, "10", "20"]);
  });

  it("does not ask again when there is nothing more, and does not ask twice for the same page at once", async () => {
    const { wrapper } = setup();
    const s = server(12, 10);
    const { result } = renderHook(() => useCursorPages(options(s.fetchPage)), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    await act(async () => {
      await Promise.all([result.current.loadMore(), result.current.loadMore(), result.current.loadMore()]);
    });
    await waitFor(() => expect(result.current.hasMore).toBe(false));
    expect(s.calls).toEqual([undefined, "10"]);
    await act(async () => {
      await result.current.loadMore();
    });
    expect(s.calls).toHaveLength(2);
  });

  it("with autoLoad keeps going to the end, one page after another, and says it is loading until then", async () => {
    const { wrapper } = setup();
    const s = server(5_000, 500);
    const { result } = renderHook(() => useCursorPages(options(s.fetchPage, { autoLoad: true })), { wrapper });
    await waitFor(() => expect(result.current.items).toHaveLength(5_000), { timeout: 5_000 });
    expect(s.calls).toHaveLength(10);
    expect(result.current.hasMore).toBe(false);
    expect(result.current.truncated).toBe(false);
    expect(result.current.items[4_999]).toBe(4_999);
  });

  it("stops at maxPages and says the result was cut off, instead of loading without end (rule 5)", async () => {
    const { wrapper } = setup();
    const s = server(10_000, 100);
    const { result } = renderHook(() => useCursorPages(options(s.fetchPage, { autoLoad: true, maxPages: 3 })), { wrapper });
    await waitFor(() => expect(result.current.items).toHaveLength(300));
    await waitFor(() => expect(result.current.isFetching).toBe(false));
    expect(s.calls).toHaveLength(3);
    expect(result.current.truncated).toBe(true);
    expect(result.current.hasMore).toBe(false);
    await act(async () => {
      await result.current.loadMore();
    });
    expect(s.calls).toHaveLength(3);
  });

  it("shows an error as an error, keeps the items it has, and a retry asks for the page that failed", async () => {
    const { wrapper } = setup();
    const s = server(30, 10);
    let fail = true;
    const fetchPage = (cursor: string | undefined): Promise<Page> => {
      if (cursor === "10" && fail) return Promise.reject(new ApiProblem({ status: 400, kind: "bad_request", title: "Bad request" }));
      return s.fetchPage(cursor);
    };
    const { result } = renderHook(() => useCursorPages(options(fetchPage)), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    await act(async () => {
      await result.current.loadMore();
    });
    await waitFor(() => expect(result.current.loadMoreError).not.toBeNull());
    expect(result.current.items).toHaveLength(10);
    expect(result.current.loadMoreError).toBeInstanceOf(ApiProblem);
    fail = false;
    await act(async () => {
      await result.current.loadMore();
    });
    await waitFor(() => expect(result.current.items).toHaveLength(20));
    expect(result.current.loadMoreError).toBeNull();
  });

  it("is an error state with no items when the first page fails", async () => {
    const { wrapper } = setup();
    const { result } = renderHook(() => useCursorPages(options(() => Promise.reject(new ApiProblem({ status: 403, kind: "forbidden", title: "Forbidden" })))), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current.items).toEqual([]);
    expect(result.current.error).toBeInstanceOf(ApiProblem);
  });

  it("does nothing while disabled", () => {
    const { wrapper } = setup();
    const s = server(5, 5);
    const { result } = renderHook(() => useCursorPages(options(s.fetchPage, { enabled: false })), { wrapper });
    expect(result.current.status).toBe("pending");
    expect(s.calls).toEqual([]);
  });

  it("gives the same items array while nothing changed, so a list does not re-render for nothing", async () => {
    const { wrapper } = setup();
    const s = server(10, 10);
    const { result, rerender } = renderHook(() => useCursorPages(options(s.fetchPage)), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("success"));
    const first = result.current.items;
    rerender();
    expect(result.current.items).toBe(first);
  });

  it("shares one cache entry between two screens that ask the same key", async () => {
    const { wrapper } = setup();
    const s = server(10, 10);
    const a = renderHook(() => useCursorPages(options(s.fetchPage)), { wrapper });
    const b = renderHook(() => useCursorPages(options(s.fetchPage)), { wrapper });
    await waitFor(() => expect(a.result.current.status).toBe("success"));
    await waitFor(() => expect(b.result.current.status).toBe("success"));
    expect(s.calls).toHaveLength(1);
  });

  it("passes the abort signal on, so a page that is no longer wanted can be dropped", async () => {
    const { wrapper } = setup();
    let seen: AbortSignal | undefined;
    const { result } = renderHook(
      () =>
        useCursorPages(
          options((_cursor, signal) => {
            seen = signal;
            return Promise.resolve({ items: [1], next: null });
          }),
        ),
      { wrapper },
    );
    await waitFor(() => expect(result.current.status).toBe("success"));
    expect(seen).toBeInstanceOf(AbortSignal);
  });
});
