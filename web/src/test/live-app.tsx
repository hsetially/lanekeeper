// A small live app for the tests of what changes with the stream (the pickup chips): a QueryClient, a signed-in session, the real
// live layer (parser, client, invalidation plan, batcher) on a fake hub, and `fetch` answered from a world the test changes
// between two answers. It mounts no router: a probe component stands in for the screen. Test code only, and in src/test because
// it needs the fake hub, which reads Node's file system (the app project has no Node types).
import { act, render, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { expect, vi } from "vitest";
import type { DomainEvent, Me } from "@/api/generated/model";
import { apiCaches } from "@/api/client";
import { SessionContext } from "@/auth/session";
import { LiveProvider } from "@/live/LiveProvider";
import { createQueryClient } from "@/lib/query/client";
import { FakeHub, frame } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";
import type { Api, Call } from "@/test/app";

const ME = { id: "u1", email: "ada@example.test", display_name: "Ada", role: "operator" } as unknown as Me;

export type LiveHarness = {
  hub: FakeHub;
  calls: Call[];
  /** Push an event on the stream and let it reach the cache. */
  send: (event: DomainEvent | { type: "resync" }) => Promise<void>;
  /** The number of answers given for a path (exact, without the query string). */
  count: (path: string) => number;
};

/**
 * Renders `ui` with the live stream open and past its first refetch (a stream with nothing to resume from reads everything once),
 * so that what the test does next is the effect of the event it sends and nothing else.
 */
export async function renderLive(api: Api, ui: ReactElement, watch: string): Promise<LiveHarness> {
  const calls: Call[] = [];
  Object.defineProperty(document, "cookie", { configurable: true, get: () => "__Host-lk_csrf=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde", set: () => undefined });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init: RequestInit = {}) => {
      const call: Call = { method: (init.method ?? "GET").toUpperCase(), path: input, headers: new Headers(init.headers), body: typeof init.body === "string" ? init.body : undefined };
      calls.push(call);
      return api(call);
    }),
  );
  apiCaches.clear();
  const hub = new FakeHub();
  const client = createQueryClient();
  const count = (path: string): number => calls.filter((c) => new URL(c.path, "http://hub.test").pathname === path).length;
  render(
    <QueryClientProvider client={client}>
      <SessionContext.Provider value={{ status: "ready", me: ME, role: "operator" }}>
        <LiveProvider config={{ stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } }}>{ui}</LiveProvider>
      </SessionContext.Provider>
    </QueryClientProvider>,
  );
  await waitFor(() => expect(hub.connections).toHaveLength(1), { timeout: 8000 });
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await Promise.resolve();
  });
  await waitFor(() => expect(count(watch)).toBeGreaterThanOrEqual(1), { timeout: 8000 });
  // The first frame makes one refetch of what is on screen; let it finish so that the next one is the event's.
  await new Promise((resolve) => setTimeout(resolve, 250));
  let id = 41;
  const send = async (event: DomainEvent | { type: "resync" }): Promise<void> => {
    await act(async () => {
      hub.last.push(event.type === "resync" ? 'event: resync\ndata: {"type":"resync"}\n\n' : frame(id++, event));
      await Promise.resolve();
    });
  };
  return { hub, calls, send, count };
}
