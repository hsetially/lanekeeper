// Helpers for the tests of the notify and restart pieces. A component here may need a query client, the write gate (who may press
// a write button, and whether live updates are up), the session (for `requires_approval`) and a router (for its links);
// `renderNotify` mounts them around the component under test, with `fetch` answered by a function the test supplies. Nothing here
// touches the network. Test code only: nothing outside the tests imports it.
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { render, type RenderResult } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import type { ReactElement } from "react";
import type { Me, Role, ServiceState, SwimlaneDetail } from "@/api/generated/model";
import { ROLE_ORDER } from "@/auth/roles";
import { SessionContext } from "@/auth/session";
import { WriteGateProvider } from "@/components/app/write-gate";
import { createQueryClient } from "@/lib/query/client";
import { json, me, problem, routes, stubApi, type Api, type Call } from "@/test/app";

export { json, problem, routes, stubApi, type Api, type Call };

export type NotifyHarness = RenderResult & { calls: Call[]; client: QueryClient };

export type NotifyHarnessOptions = {
  role?: Role | null;
  /** False: live updates are paused, so writes are off ("Unavailable offline"). */
  online?: boolean;
  api?: Api;
  /** Fields of the signed-in user to override (`requires_approval`). */
  user?: Partial<Me>;
  client?: QueryClient;
};

const NOT_FOUND = (): Response => problem(404, "not_found");

/** Render `ui` as a signed-in person with `role`, with a router (for links) and a fake server. */
export function renderNotify(ui: ReactElement, options: NotifyHarnessOptions = {}): NotifyHarness {
  const { role = "operator", online = true, api = NOT_FOUND, user = {} } = options;
  const calls = stubApi(api);
  const client = options.client ?? createQueryClient();
  const session = { status: "ready" as const, me: { ...me(role), ...user } as unknown as Me, role };
  const root = createRootRoute({
    component: () => (
      <SessionContext.Provider value={session}>
        <QueryClientProvider client={client}>
          <WriteGateProvider value={{ roles: ROLE_ORDER, role, online }}>{ui}</WriteGateProvider>
        </QueryClientProvider>
      </SessionContext.Provider>
    ),
  });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ["/"] }) });
  const view = render(<RouterProvider router={router} />);
  return Object.assign(view, { calls, client });
}

/** A service of the swimlane `sitb`: waiting for a notify or a restart unless `over` says otherwise. */
export function service(name: string, over: Partial<ServiceState> = {}): ServiceState {
  return {
    service: { namespace: "sitb", name },
    pickup: "needs_notify_or_restart",
    pending_since: 1_760_000_000_000 - 120_000,
    live_by: null,
    notifications_enabled: true,
    cache_ttl_secs: 300,
    pending_files: [],
    ...over,
  };
}

/** The detail of `sitb`: an Operator can act on it, its agent is connected and its baseline is confirmed. `over` replaces any member. */
export function laneDetail(over: Partial<SwimlaneDetail> = {}): SwimlaneDetail {
  return {
    id: "sitb",
    display_name: "sitb",
    tenants: ["sit7"],
    agent: "connected",
    last_scan_at: 1_760_000_000_000 - 5_000,
    base_version: null,
    drift_counts: [],
    finding_count: 0,
    baseline: { state: "confirmed", by: null, at: 1_759_000_000_000 },
    services: [],
    auto_notify: false,
    ...over,
  };
}
