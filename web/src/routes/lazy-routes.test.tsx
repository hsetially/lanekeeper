import { act, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory } from "@tanstack/react-router";
import { createQueryClient } from "@/lib/query/client";
import { resetStaleForTests } from "@/lib/deploy/stale";
import { clearDirtyForTests, markDirty } from "@/auth/dirty";
import { createAppRouter } from "@/router";
import { json, me, renderApp } from "@/test/app";
import { PAGE_LOADERS, type PageLoaders } from "./pages";
import { ROUTES } from "./table";

// Route-level code splitting (P10): every screen is its own chunk, loaded when the route is first used, preloaded on hover or
// focus and on idle for the sidebar's destinations. A chunk that cannot be fetched after a deploy ends in the "new version"
// screen, never in a silent reload that loses an edit.

afterEach(() => {
  resetStaleForTests();
  clearDirtyForTests();
  vi.restoreAllMocks();
});

const signedIn = (call: { path: string }) => (call.path === "/api/v1/me" ? json(me("admin")) : json({}, 404));
const chunkError = () => new TypeError("Failed to fetch dynamically imported module: https://hub.example/assets/Placeholder-1.js");

describe("page loaders", () => {
  it("has a loader for every route that shows a screen, and for no other", () => {
    expect(Object.keys(PAGE_LOADERS).sort()).toEqual(ROUTES.filter((r) => r.redirect === undefined).map((r) => r.id).sort());
  });

  it("makes nothing eager: each loader is a function that has not run yet", () => {
    for (const [id, loader] of Object.entries(PAGE_LOADERS)) expect(typeof loader, id).toBe("function");
  });
});

describe("the router", () => {
  it("does not load a screen until its route is used, and loads it for a route that is", async () => {
    const loaded: string[] = [];
    const pages: PageLoaders = Object.fromEntries(
      Object.keys(PAGE_LOADERS).map((id) => [
        id,
        () => {
          loaded.push(id);
          return Promise.resolve({ default: () => <h1>screen {id}</h1> });
        },
      ]),
    );
    expect(loaded).toEqual([]);
    const app = renderApp("/swimlanes", signedIn, { pages });
    expect(await screen.findByRole("heading", { level: 1, name: "screen swimlanes" })).toBeInTheDocument();
    await app.settled();
    expect(loaded).toEqual(["swimlanes"]);
  });

  it("preloads a route's screen without showing it (what a hover on a link does)", async () => {
    const loader = vi.fn(() => Promise.resolve({ default: () => <h1>screen compare</h1> }));
    const router = createAppRouter({ queryClient: createQueryClient(), history: createMemoryHistory({ initialEntries: ["/"] }), pages: { ...PAGE_LOADERS, compare: loader } });
    await router.load();
    await act(async () => {
      await router.preloadRoute({ to: "/compare" as never });
    });
    expect(loader).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("screen compare")).not.toBeInTheDocument();
  });

  it("preloads on intent: a link that is hovered or focused loads its screen", () => {
    const router = createAppRouter({ queryClient: createQueryClient(), history: createMemoryHistory({ initialEntries: ["/"] }) });
    expect(router.options.defaultPreload).toBe("intent");
  });

  it("shows 'A new version is available' when a screen's chunk is gone, and says nothing else about it", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    renderApp("/swimlanes", signedIn, { pages: { ...PAGE_LOADERS, swimlanes: () => Promise.reject(chunkError()) } });
    expect(await screen.findByRole("heading", { level: 1, name: "A new version is available" }, { timeout: 4_000 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reload" })).toBeInTheDocument();
  });

  it("does not reload the page by itself, and does not use session storage for it (the library's lazy component does both)", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const reload = vi.fn();
    const original = window.location;
    Object.defineProperty(window, "location", { configurable: true, value: { reload } });
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    renderApp("/swimlanes", signedIn, { pages: { ...PAGE_LOADERS, swimlanes: () => Promise.reject(chunkError()) } });
    await screen.findByRole("heading", { level: 1, name: "A new version is available" }, { timeout: 4_000 });
    expect(reload).not.toHaveBeenCalled();
    expect(setItem).not.toHaveBeenCalled();
    Object.defineProperty(window, "location", { configurable: true, value: original });
  });

  it("holds the reload while there is unsaved work", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    markDirty("edit-1");
    renderApp("/swimlanes", signedIn, { pages: { ...PAGE_LOADERS, swimlanes: () => Promise.reject(chunkError()) } });
    await screen.findByRole("heading", { level: 1, name: "A new version is available" }, { timeout: 4_000 });
    expect(screen.getByRole("button", { name: "Reload" })).toHaveAttribute("aria-disabled", "true");
  });

  it("shows the ordinary error screen for an error that is not a missing chunk", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    renderApp("/swimlanes", signedIn, { pages: { ...PAGE_LOADERS, swimlanes: () => Promise.reject(new Error("boom")) } });
    expect(await screen.findByRole("heading", { level: 1, name: "Something went wrong" })).toBeInTheDocument();
    expect(screen.queryByText("A new version is available")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("boom")).not.toBeInTheDocument());
  });
});
