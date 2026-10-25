import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Role } from "@/api/generated/model";
import { json, me, renderApp } from "@/test/app";
import { idlePreload, prefetchTargets, SIDEBAR_DESTINATIONS } from "./prefetch";

// The sidebar's destinations are the screens a person opens next. They are loaded while the browser is idle, so that the
// click (or the key) that follows finds the screen already there, and a route change waits for nothing (P10: under 100 ms
// when the data is cached). Only the screens the role may open are loaded: a Viewer never fetches the Admin chunks.

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("prefetchTargets", () => {
  it("is the sidebar's destinations, in the sidebar's order", () => {
    expect(SIDEBAR_DESTINATIONS).toEqual(["swimlanes", "compare", "status", "findings", "approvals", "audit", "admin-users"]);
  });

  it("gives each role the destinations it may open, and no more", () => {
    const ids = (role: Role | null) => prefetchTargets(role).map((t) => t.id);
    expect(ids(null)).toEqual([]);
    expect(ids(Role.viewer)).toEqual(["swimlanes", "compare", "status", "findings"]);
    expect(ids(Role.editor)).toEqual(["swimlanes", "compare", "status", "findings"]);
    expect(ids(Role.operator)).toEqual(["swimlanes", "compare", "status", "findings"]);
    expect(ids(Role.admin)).toEqual(["swimlanes", "compare", "status", "findings", "approvals", "audit", "admin-users"]);
  });

  it("has no destination that needs a path parameter, since there is nothing to fill it with", () => {
    for (const target of prefetchTargets(Role.admin)) expect(target.path).not.toContain(":");
  });
});

describe("idlePreload", () => {
  /** An idle scheduler the test runs by hand. */
  function idle() {
    const queue: Array<() => void> = [];
    return { schedule: (run: () => void) => (queue.push(run), () => queue.splice(queue.indexOf(run), 1)), runNext: () => queue.shift()?.(), pending: () => queue.length };
  }

  it("loads one destination at each idle moment, in order, and then stops", async () => {
    const router = { preloadRoute: vi.fn((_options: { to: string }) => Promise.resolve()) };
    const clock = idle();
    idlePreload(router, prefetchTargets(Role.viewer), { schedule: clock.schedule, saveData: () => false });
    expect(router.preloadRoute).not.toHaveBeenCalled();
    clock.runNext();
    expect(router.preloadRoute).toHaveBeenCalledTimes(1);
    expect(router.preloadRoute).toHaveBeenLastCalledWith({ to: "/swimlanes" });
    clock.runNext();
    clock.runNext();
    clock.runNext();
    expect(router.preloadRoute.mock.calls.map((c) => c[0].to)).toEqual(["/swimlanes", "/compare", "/status", "/findings"]);
    await Promise.resolve();
    expect(clock.pending()).toBe(0);
  });

  it("loads nothing when the person asked the browser to save data", () => {
    const router = { preloadRoute: vi.fn((_options: { to: string }) => Promise.resolve()) };
    const clock = idle();
    idlePreload(router, prefetchTargets(Role.admin), { schedule: clock.schedule, saveData: () => true });
    expect(clock.pending()).toBe(0);
    expect(router.preloadRoute).not.toHaveBeenCalled();
  });

  it("stops when cancelled, even with destinations left", () => {
    const router = { preloadRoute: vi.fn((_options: { to: string }) => Promise.resolve()) };
    const clock = idle();
    const cancel = idle_(router, clock);
    clock.runNext();
    cancel();
    expect(clock.pending()).toBe(0);
    expect(router.preloadRoute).toHaveBeenCalledTimes(1);
  });

  it("does not fail, and goes on, when one destination cannot be loaded", async () => {
    const router = { preloadRoute: vi.fn((_options: { to: string }) => Promise.resolve()).mockRejectedValueOnce(new Error("offline")) };
    const clock = idle();
    idlePreload(router, prefetchTargets(Role.viewer), { schedule: clock.schedule, saveData: () => false });
    clock.runNext();
    await Promise.resolve();
    await Promise.resolve();
    clock.runNext();
    expect(router.preloadRoute).toHaveBeenCalledTimes(2);
  });

  function idle_(router: { preloadRoute: (o: { to: string }) => Promise<unknown> }, clock: ReturnType<typeof idle>) {
    return idlePreload(router, prefetchTargets(Role.viewer), { schedule: clock.schedule, saveData: () => false });
  }
});

describe("in the app", () => {
  it("loads the destinations of the role after the page is up, and not the others", async () => {
    const idleCallbacks: Array<() => void> = [];
    vi.stubGlobal("requestIdleCallback", (cb: () => void) => idleCallbacks.push(cb));
    vi.stubGlobal("cancelIdleCallback", () => undefined);
    const loaded: string[] = [];
    const { PAGE_LOADERS } = await import("./pages");
    const pages = Object.fromEntries(Object.keys(PAGE_LOADERS).map((id) => [id, () => (loaded.push(id), Promise.resolve({ default: () => <h1>{id}</h1> }))]));
    const app = renderApp("/swimlanes", (call) => (call.path === "/api/v1/me" ? json(me("viewer")) : json({}, 404)), { pages });
    expect(await screen.findByRole("heading", { level: 1, name: "swimlanes" })).toBeInTheDocument();
    await app.settled();
    for (let i = 0; i < 20 && idleCallbacks.length > 0; i++) {
      await act(async () => {
        idleCallbacks.shift()?.();
        await Promise.resolve();
      });
    }
    expect(loaded).toEqual(expect.arrayContaining(["swimlanes", "compare", "status", "findings"]));
    expect(loaded).not.toContain("admin-users");
    expect(loaded).not.toContain("audit");
    render(<p>done</p>);
  });
});
