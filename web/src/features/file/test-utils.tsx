// Helpers for the tests of the file view. A component here may need a query client, the write gate (who may press a write
// button), the session, the keyboard registry and a router for its links; `renderFile` mounts all of them around the
// component under test, with `fetch` answered by a function the test supplies. Nothing here touches the network.
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { render, type RenderResult } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import type { ReactElement } from "react";
import type { Me, Role } from "@/api/generated/model";
import { SessionContext } from "@/auth/session";
import { ROLE_ORDER } from "@/auth/roles";
import { WriteGateProvider } from "@/components/app/write-gate";
import { KeyboardProvider } from "@/keyboard";
import { createQueryClient } from "@/lib/query/client";
import { json, me, problem, routes, stubApi, type Api, type Call } from "@/test/app";

export { json, problem, routes, stubApi, type Api, type Call };

export type FileHarness = RenderResult & { calls: Call[]; client: QueryClient };

export type FileHarnessOptions = {
  role?: Role | null;
  /** False: the live stream is down, so writes are off ("Unavailable offline"). */
  online?: boolean;
  api?: Api;
  /** Fields of the signed-in user to override (requires_approval, needs_github_token). */
  user?: Partial<Me>;
  client?: QueryClient;
};

const NOT_FOUND = (): Response => problem(404, "not_found");

/** Render `ui` as a signed-in person with `role`, with a router (for links), the keyboard registry and a fake server. */
export function renderFile(ui: ReactElement, options: FileHarnessOptions = {}): FileHarness {
  const { role = "editor", online = true, api = NOT_FOUND, user = {} } = options;
  const calls = stubApi(api);
  const client = options.client ?? createQueryClient();
  const session = { status: "ready" as const, me: { ...me(role), ...user } as unknown as Me, role };
  const root = createRootRoute({
    component: () => (
      <SessionContext.Provider value={session}>
        <QueryClientProvider client={client}>
          <WriteGateProvider value={{ roles: ROLE_ORDER, role, online }}>
            <KeyboardProvider>{ui}</KeyboardProvider>
          </WriteGateProvider>
        </QueryClientProvider>
      </SessionContext.Provider>
    ),
  });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ["/"] }) });
  const view = render(<RouterProvider router={router} />);
  return Object.assign(view, { calls, client });
}

export const HASH_A = "a".repeat(64);
export const HASH_B = "b".repeat(64);
export const HASH_C = "c".repeat(64);
