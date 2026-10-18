// A helper for the tests that need the whole app shell: the real router, built from the route table, on a memory history,
// with `fetch` answered by a function the test supplies. Nothing here touches the network.
import { render, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { expect, vi } from "vitest";
import { apiCaches } from "@/api/client";
import { createQueryClient } from "@/lib/query/client";
import { createAppRouter } from "@/router";
import type { LiveConfig } from "@/live/live";
import type { PageLoaders } from "@/routes/pages";
import { toasts } from "@/features/shell/toast/store";

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

/**
 * Render the whole app on a memory history. The live stream is OFF unless the test passes `live`: most tests are about
 * something else, and a stream that asks `fetch` for `/api/v1/events` would add a call to every list of calls they assert on.
 */
export function renderApp(url: string, api: Api, options: { pages?: PageLoaders; live?: LiveConfig; strict?: boolean } = {}) {
  const calls: Call[] = [];
  // jsdom refuses a `__Host-` cookie on an http origin, so the test supplies the cookie string the client reads.
  // The setter takes what the app writes (the theme cookie) and drops it: nothing in a test reads it back.
  Object.defineProperty(document, "cookie", { configurable: true, get: () => `__Host-lk_csrf=${TOKEN}`, set: () => undefined });
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
  // The toasts live in a store of the tab; a test starts without the last one's.
  toasts.clear();
  const queryClient = createQueryClient();
  const history = createMemoryHistory({ initialEntries: [url] });
  const router = createAppRouter({ queryClient, history, live: options.live ?? { enabled: false }, ...(options.pages ? { pages: options.pages } : {}) });
  const tree = (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
  // StrictMode has to be the root element for React to double-invoke effects in development, as src/main.tsx does.
  const view = render(options.strict ? <StrictMode>{tree}</StrictMode> : tree);
  const location = () => router.state.location.href;
  const settled = () => waitFor(() => expect(router.state.status).toBe("idle"));
  return { ...view, calls, queryClient, router, location, settled };
}
