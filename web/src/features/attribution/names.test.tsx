import { QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { apiCaches } from "@/api/client";
import type { Me, Role, User } from "@/api/generated/model";
import { SessionContext, type SessionState } from "@/auth/session";
import { createQueryClient } from "@/lib/query/client";
import { json, me, problem, TOKEN } from "@/test/app";
import { useActorNames } from "./names";

// Who is who, for the screens that name a person from an id. The contract carries a user as an id only; the one list of names is
// the Admin-only user list. So an Admin's screens can name people, and everyone else's say "Lanekeeper user" and a short id.
// The hook asks for the list ONLY for an Admin (a Viewer's request would be a 403 on every screen), and a list that fails to
// load leaves the screens as they are for everyone else: names missing, never invented.

const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const MARCO = "11111111-1111-4111-8111-111111111111:3f2a91bc-0000-4000-8000-000000000003";

const user = (id: string, display_name: string): User => ({ id, email: `${display_name.toLowerCase().replace(/\W+/g, ".")}@example.test`, display_name, role: "editor", status: "active", requires_approval: false, github_login: null, last_seen_at: null });

let calls: string[] = [];

function wrap(session: SessionState, answer: (path: string) => Response) {
  calls = [];
  Object.defineProperty(document, "cookie", { configurable: true, get: () => `__Host-lk_csrf=${TOKEN}`, set: () => undefined });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      calls.push(input);
      return Promise.resolve(answer(input));
    }),
  );
  apiCaches.clear();
  const client = createQueryClient();
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <SessionContext.Provider value={session}>
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      </SessionContext.Provider>
    );
  };
}

const ready = (role: Role | null): SessionState => ({ status: "ready", me: { ...me(role), id: ME } as unknown as Me, role });

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
  vi.unstubAllGlobals();
});

describe("useActorNames", () => {
  it("names people for an Admin, from the user list", async () => {
    const wrapper = wrap(ready("admin"), () => json({ items: [user(ME, "Priya Shah"), user(MARCO, "Marco Diaz")], next: null }));
    const { result } = renderHook(() => useActorNames(), { wrapper });
    expect(result.current.loaded).toBe(false);
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.you).toBe(ME);
    expect(result.current.nameOf(MARCO)).toBe("Marco Diaz");
    expect(result.current.nameOf(ME)).toBe("Priya Shah");
    expect(calls.filter((c) => c.includes("/api/v1/admin/users"))).toHaveLength(1);
  });

  it("has no name for a person who is not in the list, and does not make one up", async () => {
    const wrapper = wrap(ready("admin"), () => json({ items: [user(MARCO, "Marco Diaz")], next: null }));
    const { result } = renderHook(() => useActorNames(), { wrapper });
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.nameOf("22222222-2222-4222-8222-222222222222:44444444-4444-4444-8444-444444444444")).toBeUndefined();
  });

  it.each(["viewer", "editor", "operator"] as const)("asks for no list at all for a %s: the list is Admin-only and the request would be refused", async (role) => {
    const wrapper = wrap(ready(role), () => problem(403, "forbidden"));
    const { result } = renderHook(() => useActorNames(), { wrapper });
    expect(result.current.loaded).toBe(true);
    expect(result.current.you).toBe(ME);
    expect(result.current.nameOf(MARCO)).toBeUndefined();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(calls).toEqual([]);
  });

  it("leaves names missing, not invented, when the list fails to load", async () => {
    const wrapper = wrap(ready("admin"), () => problem(500, "internal"));
    const { result } = renderHook(() => useActorNames(), { wrapper });
    await waitFor(() => expect(calls.length).toBeGreaterThan(0));
    expect(result.current.nameOf(MARCO)).toBeUndefined();
    expect(result.current.you).toBe(ME);
  });

  it("asks for nothing and knows nobody before the session is there", () => {
    const wrapper = wrap({ status: "loading" }, () => problem(401, "unauthorized"));
    const { result } = renderHook(() => useActorNames(), { wrapper });
    expect(result.current.you).toBeNull();
    expect(result.current.nameOf(MARCO)).toBeUndefined();
    expect(calls).toEqual([]);
  });

  it("is the same object between renders while nothing changed, so a list of rows does not redraw for it", () => {
    const wrapper = wrap(ready("editor"), () => problem(403, "forbidden"));
    const { result, rerender } = renderHook(() => useActorNames(), { wrapper });
    const first = result.current;
    rerender();
    expect(result.current).toBe(first);
  });
});
