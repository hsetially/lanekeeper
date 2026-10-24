import { useCallback, useEffect, useRef, useState } from "react";
import { ApiProblem } from "@/api/problem";
import type { QuerySource } from "@/lib/query/use-query-state";

// A search is asked for what is on screen, answered once, and forgotten with the screen. It is NOT put in the app's query cache:
//  - nothing tells the app that it changed (a live event names a file or a swimlane, not a search), so there is nothing to
//    invalidate it by, and an answer older than its screen is not worth keeping;
//  - a text or docs search answers with the lines of files and docs, and the same address asked later should be asked again;
//  - the palette does the same for its searches (src/features/shell/palette/remote.ts).
// So the answer lives in this hook's state, a new key drops it at once (the screen shows the loading state, never the answer to
// another query), the request that is replaced is aborted, and an answer that arrives late is ignored.
//
// The state is shaped as a `QuerySource`, so `useQueryState` and `QueryBoundary` show it like any other query.

export type OneShotRequest<T> = { key: string; load: (signal: AbortSignal) => Promise<T> };
export type OneShot<T> = { idle: boolean; source: QuerySource<T> };

const GENERIC = (): ApiProblem => new ApiProblem({ status: 0, kind: "unknown", title: "Something went wrong" });
/** A problem for whatever was thrown. The message of an error that is not a problem could hold anything, so it is dropped (S21). */
const asProblem = (error: unknown): ApiProblem => (error instanceof ApiProblem ? error : GENERIC());

type Slot<T> = { key: string; status: "pending" | "success" | "error"; data?: T; error: ApiProblem | null; at: number };

export function useOneShot<T>(request: OneShotRequest<T> | null): OneShot<T> {
  const [slot, setSlot] = useState<Slot<T> | null>(null);
  const [round, setRound] = useState(0);
  const key = request?.key ?? null;
  const latest = useRef(request?.load);
  useEffect(() => {
    latest.current = request?.load;
  });

  useEffect(() => {
    const load = latest.current;
    if (key === null || load === undefined) {
      setSlot(null);
      return undefined;
    }
    const controller = new AbortController();
    setSlot({ key, status: "pending", error: null, at: 0 });
    load(controller.signal).then(
      (data) => {
        if (!controller.signal.aborted) setSlot({ key, status: "success", data, error: null, at: Date.now() });
      },
      (error: unknown) => {
        if (!controller.signal.aborted) setSlot({ key, status: "error", error: asProblem(error), at: Date.now() });
      },
    );
    return () => {
      controller.abort();
    };
  }, [key, round]);

  const refetch = useCallback(() => {
    setRound((n) => n + 1);
  }, []);

  // The slot is for the current key or it is not drawn: the render in which the key changed shows the loading state.
  const mine = slot !== null && slot.key === key ? slot : null;
  const source: QuerySource<T> = {
    data: mine?.status === "success" ? mine.data : undefined,
    error: mine?.status === "error" ? mine.error : null,
    status: mine?.status ?? "pending",
    isFetching: mine === null || mine.status === "pending",
    dataUpdatedAt: mine?.at ?? 0,
    refetch,
  };
  return { idle: key === null, source };
}

export type PagedRequest<TPage, TItem> = {
  key: string;
  fetch: (cursor: string | undefined, signal: AbortSignal) => Promise<TPage>;
  select: (page: TPage) => { readonly items: readonly TItem[]; readonly next: string | null };
  /** The most pages that are asked for. A list that stops there says so (`truncated`). */
  maxPages: number;
};

export type PagedOneShot<TItem> = {
  idle: boolean;
  items: TItem[];
  hasMore: boolean;
  /** `maxPages` was reached with more still on the server. */
  truncated: boolean;
  loadingMore: boolean;
  /** The error of the last attempt at a later page. The rows already loaded stay. */
  loadMoreError: ApiProblem | null;
  loadMore: () => Promise<void>;
  source: QuerySource<TItem[]>;
};

type PagedSlot<TItem> = { key: string; status: "pending" | "success" | "error"; items: TItem[]; next: string | null; pages: number; error: ApiProblem | null; moreError: ApiProblem | null; loading: boolean; at: number };

export function usePagedOneShot<TPage, TItem>(request: PagedRequest<TPage, TItem> | null): PagedOneShot<TItem> {
  const [slot, setSlot] = useState<PagedSlot<TItem> | null>(null);
  const [round, setRound] = useState(0);
  const key = request?.key ?? null;
  const latest = useRef(request);
  useEffect(() => {
    latest.current = request;
  });
  const controller = useRef<AbortController | null>(null);
  const inFlight = useRef(false);

  useEffect(() => {
    const current = latest.current;
    if (key === null || current === null) {
      setSlot(null);
      return undefined;
    }
    const mine = new AbortController();
    controller.current = mine;
    inFlight.current = true;
    setSlot({ key, status: "pending", items: [], next: null, pages: 0, error: null, moreError: null, loading: true, at: 0 });
    current.fetch(undefined, mine.signal).then(
      (page) => {
        if (mine.signal.aborted) return;
        inFlight.current = false;
        const { items, next } = current.select(page);
        setSlot({ key, status: "success", items: [...items], next, pages: 1, error: null, moreError: null, loading: false, at: Date.now() });
      },
      (error: unknown) => {
        if (mine.signal.aborted) return;
        inFlight.current = false;
        setSlot({ key, status: "error", items: [], next: null, pages: 0, error: asProblem(error), moreError: null, loading: false, at: Date.now() });
      },
    );
    return () => {
      mine.abort();
      inFlight.current = false;
    };
  }, [key, round]);

  const view = slot !== null && slot.key === key ? slot : null;
  const maxPages = request?.maxPages ?? 1;
  const hasMore = view !== null && view.status === "success" && view.next !== null && view.pages < maxPages;

  const loadMore = useCallback((): Promise<void> => {
    const current = latest.current;
    const mine = controller.current;
    const from = view;
    if (current === null || mine === null || from === null || from.status !== "success" || from.next === null || from.pages >= current.maxPages || inFlight.current) return Promise.resolve();
    inFlight.current = true;
    setSlot((s) => (s === null || s.key !== from.key ? s : { ...s, loading: true, moreError: null }));
    return current.fetch(from.next, mine.signal).then(
      (page) => {
        if (mine.signal.aborted) return;
        inFlight.current = false;
        const { items, next } = current.select(page);
        setSlot((s) => (s === null || s.key !== from.key ? s : { ...s, items: [...s.items, ...items], next, pages: s.pages + 1, loading: false, moreError: null }));
      },
      (error: unknown) => {
        if (mine.signal.aborted) return;
        inFlight.current = false;
        setSlot((s) => (s === null || s.key !== from.key ? s : { ...s, loading: false, moreError: asProblem(error) }));
      },
    );
  }, [view]);

  const refetch = useCallback(() => {
    setRound((n) => n + 1);
  }, []);

  const items = view?.items ?? EMPTY;
  const source: QuerySource<TItem[]> = {
    data: view?.status === "success" ? view.items : undefined,
    error: view?.status === "error" ? view.error : null,
    status: view?.status ?? "pending",
    isFetching: view === null || view.loading,
    dataUpdatedAt: view?.at ?? 0,
    refetch,
  };
  return {
    idle: key === null,
    items,
    hasMore,
    truncated: view !== null && view.status === "success" && view.next !== null && view.pages >= maxPages,
    loadingMore: view !== null && view.status === "success" && view.loading,
    loadMoreError: view?.moreError ?? null,
    loadMore,
    source,
  };
}

const EMPTY: never[] = [];
