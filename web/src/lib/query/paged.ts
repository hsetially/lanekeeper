import type { CursorPageOptions } from "./cursor-pages";
import type { QueryKey } from "./keys";

// A list the server pages with a keyset cursor, as the options of `useCursorPages`. Every paged operation of the contract
// answers `{ items, next }`, so one stable `select` serves them all: it has to be ONE function, because useCursorPages
// derives its `items` array from the pages and `select`, and a new function each render would make a new array each render.

export type Page<T> = { readonly items: readonly T[]; readonly next: string | null };

/** The items and the next cursor of a standard page. Module-level on purpose: its identity never changes. */
export function selectPage<T>(page: Page<T>): { readonly items: readonly T[]; readonly next: string | null } {
  return { items: page.items, next: page.next };
}

export type PagedFreshness = {
  /** `Infinity` when live events keep the list fresh; a number of milliseconds when nothing tells the app that it changed. */
  staleTime: number;
  gcTime?: number;
};

/** The options of a paged list. `fetch` is the generated operation, called with the cursor and the abort signal. */
export function pagedQuery<TPage extends Page<unknown>>(options: { queryKey: QueryKey; fetch: (cursor: string | undefined, signal: AbortSignal) => Promise<TPage> } & PagedFreshness): CursorPageOptions<TPage, TPage["items"][number]> {
  return {
    queryKey: options.queryKey,
    fetchPage: options.fetch,
    select: selectPage,
    staleTime: options.staleTime,
    ...(options.gcTime === undefined ? {} : { gcTime: options.gcTime }),
  };
}
