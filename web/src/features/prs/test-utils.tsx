// Helpers for the tests of the pull-request pieces. A component here may need a query client, the write gate, the session and a
// router (for its links); `renderPrs` mounts them around the component under test, with `fetch` answered by a function the test
// supplies. Nothing here touches the network.
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { render, type RenderResult } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import type { ReactElement } from "react";
import { vi } from "vitest";
import { apiCaches } from "@/api/client";
import type { Me, PrLink, Role } from "@/api/generated/model";
import { SessionContext } from "@/auth/session";
import { ROLE_ORDER } from "@/auth/roles";
import { WriteGateProvider } from "@/components/app/write-gate";
import { createQueryClient } from "@/lib/query/client";
import type { LiveConfig } from "@/live/live";
import { LiveProvider } from "@/live/LiveProvider";
import { json, me, problem, TOKEN, type Api, type Call } from "@/test/app";

export { json, problem, type Api, type Call };

export type PrsHarness = RenderResult & { calls: Call[]; client: QueryClient };

export type PrsHarnessOptions = {
  role?: Role | null;
  api?: Api;
  user?: Partial<Me>;
  client?: QueryClient;
  /** Mount the real live layer (the parser, the client, the plan of what an event invalidates) with these settings. */
  live?: LiveConfig;
};

const NOT_FOUND = (): Response => problem(404, "not_found");

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

/** Render `ui` as a signed-in person with `role`, with a router (for links) and a fake server. */
export function renderPrs(ui: ReactElement, options: PrsHarnessOptions = {}): PrsHarness {
  const { role = "viewer", api = NOT_FOUND, user = {}, live } = options;
  const calls = stubApi(api);
  const client = options.client ?? createQueryClient();
  const session = { status: "ready" as const, me: { ...me(role), ...user } as unknown as Me, role };
  const root = createRootRoute({
    component: () => (
      <SessionContext.Provider value={session}>
        <QueryClientProvider client={client}>
          {live === undefined ? <WriteGateProvider value={{ roles: ROLE_ORDER, role, online: true }}>{ui}</WriteGateProvider> : <LiveProvider config={live}>{ui}</LiveProvider>}
        </QueryClientProvider>
      </SessionContext.Provider>
    ),
  });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ["/"] }) });
  const view = render(<RouterProvider router={router} />);
  return Object.assign(view, { calls, client });
}

/** A pull-request link, whole, for a test: `over` replaces any member. */
export function prLink(over: Partial<PrLink> = {}): PrLink {
  return {
    id: 1,
    swimlane: "sitb",
    path: "tx-infinity-api/tx-infinity-core-sit7.yml",
    nfs_hash: "a".repeat(64),
    state: "open",
    pr_url: "https://github.com/csp/csp-tenant-data/pull/418",
    pr_number: 418,
    author: "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002",
    created_at: 1_760_000_000_000 - 2 * 3_600_000,
    closed_at: null,
    ...over,
  };
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
