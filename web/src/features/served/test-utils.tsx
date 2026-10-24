// Helpers for the tests of the "As served" view. A component of the view may need a query client and a router (for its links and its
// address); `renderServed` mounts them around the component under test, with `fetch` answered by a function the test supplies.
// Nothing here touches the network.
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { act, render, type RenderResult } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { useSyncExternalStore, type ReactElement } from "react";
import { vi } from "vitest";
import { apiCaches } from "@/api/client";
import { createQueryClient } from "@/lib/query/client";
import { json, problem, TOKEN, type Api, type Call } from "@/test/app";

export { json, problem, type Api, type Call };

export type ServedHarness = RenderResult & {
  calls: Call[];
  client: QueryClient;
  location: () => string;
  navigations: string[];
  /** Put another element where the first was, inside the same router and query client (a rerender that keeps the state of the tree). */
  update: (ui: ReactElement) => void;
};

/** Answer `fetch` with `api` and record every call. */
export function stubApi(api: Api): Call[] {
  const calls: Call[] = [];
  Object.defineProperty(document, "cookie", { configurable: true, get: () => `__Host-lk_csrf=${TOKEN}`, set: () => undefined });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init: RequestInit = {}) => {
      const call: Call = { method: (init.method ?? "GET").toUpperCase(), path: input, headers: new Headers(init.headers), body: typeof init.body === "string" ? init.body : undefined };
      calls.push(call);
      return api(call);
    }),
  );
  apiCaches.clear();
  return calls;
}

const NOT_FOUND = (): Response => problem(404, "not_found");

/** Render `ui` with a router (for links and the address) and a fake server. `navigations` lists every address the view moved to. */
export function renderServed(ui: ReactElement, options: { api?: Api; client?: QueryClient; url?: string } = {}): ServedHarness {
  const { api = NOT_FOUND, url = "/" } = options;
  const calls = stubApi(api);
  const client = options.client ?? createQueryClient();
  // The element under test lives in a tiny store, so that `update` changes it without remounting the router or the client.
  let current = ui;
  const listeners = new Set<() => void>();
  const subscribe = (listener: () => void): (() => void) => {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  };
  const Current = () => <>{useSyncExternalStore(subscribe, () => current)}</>;
  const root = createRootRoute({
    component: () => (
      <QueryClientProvider client={client}>
        <Current />
      </QueryClientProvider>
    ),
  });
  const history = createMemoryHistory({ initialEntries: [url] });
  const navigations: string[] = [];
  const push = history.push.bind(history);
  history.push = (...args: Parameters<typeof push>) => {
    navigations.push(args[0]);
    push(...args);
  };
  const router = createRouter({ routeTree: root, history });
  const view = render(<RouterProvider router={router} />);
  const update = (next: ReactElement): void => {
    act(() => {
      current = next;
      for (const listener of listeners) listener();
    });
  };
  return Object.assign(view, { calls, client, navigations, update, location: () => router.history.location.href });
}

/** A fake server by "METHOD /path": the handler's return is the body of a 200, or a Response of its own. The rest is a 404 problem. */
export function routes(table: Record<string, (call: Call) => unknown>): Api {
  return (call) => {
    const path = call.path.split("?")[0] ?? call.path;
    const handler = table[`${call.method} ${path}`];
    if (!handler) return problem(404, "not_found");
    const out = handler(call);
    return out instanceof Response ? out : json(out);
  };
}

/** The search parameters of a call. */
export const queryOf = (call: Call): URLSearchParams => new URL(call.path, "http://hub.test").searchParams;
