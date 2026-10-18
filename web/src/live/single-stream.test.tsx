import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { json, me, problem, renderApp, type Api } from "@/test/app";
import { FakeHub } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";

// The plan's `live/single-stream.test.tsx`: the whole app, with its real router and session, counts the streams it holds open.
// One per tab, none before sign-in, closed on sign-out, never doubled by navigation, a 401 or a role change.

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const as = (role: string | null): Api => (call) => (call.path === "/api/v1/me" ? json(me(role)) : call.path === "/auth/logout" ? new Response(null, { status: 204 }) : problem(404, "not_found"));
const signedOut: Api = (call) => (call.path === "/api/v1/me" ? problem(401, "unauthenticated") : problem(404, "not_found"));

function app(url: string, api: Api, hub: FakeHub) {
  return renderApp(url, api, { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
}
const quiet = (ms = 150) => new Promise<void>((resolve) => setTimeout(resolve, ms));

describe("screens before the session open no stream", () => {
  it.each(["/signin", "/access-requested", "/access/pending", "/access-disabled", "/access/disabled", "/404", "/error?req=abc123", "/403"])("%s", async (url) => {
    const hub = new FakeHub();
    const view = app(url, signedOut, hub);
    await view.settled();
    await screen.findByRole("heading", { level: 1 });
    await quiet();
    expect(hub.connections).toHaveLength(0);
    expect(view.calls.filter((c) => c.path.startsWith("/api/v1/events"))).toEqual([]);
  });

  it("a user with no role yet (the hub answers 403 access-pending) opens none", async () => {
    const hub = new FakeHub();
    const view = app("/swimlanes", (call) => (call.path === "/api/v1/me" ? problem(403, "access-pending") : problem(404, "not_found")), hub);
    await waitFor(() => expect(view.location()).toBe("/access-requested"));
    await quiet();
    expect(hub.connections).toHaveLength(0);
  });
});

describe("a signed-in user holds exactly one stream", () => {
  it("opens one for a screen that needs the session, and is live from its first frame", async () => {
    const hub = new FakeHub();
    app("/swimlanes", as("viewer"), hub);
    await screen.findByRole("navigation", { name: "Sections" });
    await waitFor(() => expect(hub.connections).toHaveLength(1));
    expect(hub.last.url).toBe("/api/v1/events");
    hub.last.push(FIRST_FRAME);
    expect(await screen.findByText("Live", { selector: "[aria-hidden='true']" })).toBeInTheDocument();
  });

  it("opens one under React's development double effects too (the app's root is strict)", async () => {
    const hub = new FakeHub();
    renderApp("/swimlanes", as("viewer"), { strict: true, live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
    await screen.findByRole("navigation", { name: "Sections" });
    await quiet();
    expect(hub.connections).toHaveLength(1);
    expect(hub.open).toBe(1);
  });

  it("is still one after navigating between screens and after the session is asked again", async () => {
    const hub = new FakeHub();
    const view = app("/swimlanes", as("admin"), hub);
    await screen.findByRole("navigation", { name: "Sections" });
    await waitFor(() => expect(hub.connections).toHaveLength(1));
    for (const to of ["/compare", "/status", "/findings", "/swimlanes", "/admin/users", "/audit"]) {
      await view.router.navigate({ to: to });
      await view.settled();
      await quiet(40);
    }
    expect(hub.connections).toHaveLength(1);
    expect(hub.open).toBe(1);
  });

  it("closes it on sign-out, and opens none on the sign-in screen it lands on", async () => {
    const hub = new FakeHub();
    const view = app("/swimlanes", as("viewer"), hub);
    await userEvent.click(await screen.findByRole("button", { name: /Ada Lovelace/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Sign out" }));
    await waitFor(() => expect(view.location()).toBe("/signin"));
    await waitFor(() => expect(hub.open).toBe(0));
    await quiet();
    expect(hub.connections).toHaveLength(1);
  });

  it("sends the user to sign-in on a 401 of the stream, like any other request, and does not try again", async () => {
    const hub = new FakeHub();
    hub.respond(401, { type: "unauthenticated" });
    const view = app("/swimlanes", as("viewer"), hub);
    await waitFor(() => expect(view.location()).toMatch(/^\/signin/));
    expect(view.location()).toBe("/signin?next=%2Fswimlanes");
    await quiet(300);
    expect(hub.connections).toHaveLength(1);
    expect(view.queryClient.getQueryCache().getAll().filter((q) => q.state.data !== undefined)).toHaveLength(0);
  });
});
