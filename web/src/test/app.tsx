// A helper for the tests that need the whole app shell: the real router, built from the route table, on a memory history,
// with `fetch` answered by a function the test supplies. Nothing here touches the network.
import { render, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { expect, vi } from "vitest";
import { apiCaches } from "@/api/client";
import { createQueryClient } from "@/lib/query/client";
import { createAppRouter } from "@/router";

export type Call = { method: string; path: string; headers: Headers; body: string | undefined };
export type Api = (call: Call) => Response | Promise<Response>;

export const TOKEN = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde";

export function problem(status: number, type: string, extra: Record<string, unknown> = {}): Response {
  return new Response(JSON.stringify({ type: `urn:lanekeeper:problem:${type}`, title: type, status, request_id: "req-test-1", ...extra }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });
}

export function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

export function me(role: string | null, extra: Record<string, unknown> = {}) {
  return {
    id: "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002",
    email: "ada@example.test",
    display_name: "Ada Lovelace",
    role,
    status: "active",
    requires_approval: false,
    github_login: null,
    last_seen_at: null,
    swimlane_access: "all",
    needs_github_token: false,
    ...extra,
  };
}

export function renderApp(url: string, api: Api) {
  const calls: Call[] = [];
  // jsdom refuses a `__Host-` cookie on an http origin, so the test supplies the cookie string the client reads.
  Object.defineProperty(document, "cookie", { configurable: true, get: () => `__Host-lk_csrf=${TOKEN}` });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init: RequestInit = {}) => {
      const call: Call = {
        method: (init.method ?? "GET").toUpperCase(),
        path: input,
        headers: new Headers(init.headers),
        body: typeof init.body === "string" ? init.body : undefined,
      };
      calls.push(call);
      return api(call);
    }),
  );
  apiCaches.clear();
  const queryClient = createQueryClient();
  const history = createMemoryHistory({ initialEntries: [url] });
  const router = createAppRouter({ queryClient, history });
  const view = render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  const location = () => router.state.location.href;
  const settled = () => waitFor(() => expect(router.state.status).toBe("idle"));
  return { ...view, calls, queryClient, router, location, settled };
}
