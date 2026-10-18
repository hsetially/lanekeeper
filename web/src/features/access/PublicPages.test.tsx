import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { json, problem, renderApp, type Api } from "@/test/app";

// The three screens before sign-in (the design's Access Screen: sign in, access requested, access disabled). A public route makes
// no API call and opens no event stream: there is no session to ask for, and no navigation to draw.

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

const silent: Api = () => problem(401, "unauthenticated");

describe("the sign-in screen", () => {
  it("is the product's name, one line about it, and one way in", async () => {
    renderApp("/signin", silent);
    expect(await screen.findByRole("heading", { level: 1, name: "Lanekeeper" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByText("Review and edit configuration across the SIT and PRESIT swimlanes.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Sign in with Microsoft" })).toHaveAttribute("href", "/auth/login");
  });

  it("is a plain link to the hub's login: a GET navigation, not a form and not a request of the app", async () => {
    const view = renderApp("/signin", silent);
    const link = await screen.findByRole("link", { name: "Sign in with Microsoft" });
    expect(link.tagName).toBe("A");
    expect(document.querySelector("form")).toBeNull();
    expect(view.calls).toHaveLength(0);
  });

  it("carries the page the person wanted as return_to, once it passed the same-origin check", async () => {
    renderApp("/signin?next=%2Fswimlanes%2Fsitb%2Ffiles%3Fpath%3Da.yml", silent);
    const link = await screen.findByRole("link", { name: "Sign in with Microsoft" });
    expect(link).toHaveAttribute("href", `/auth/login?return_to=${encodeURIComponent("/swimlanes/sitb/files?path=a.yml")}`);
  });

  it.each([["//evil.example/x"], ["https://evil.example/"], ["/\\evil.example"], ["javascript:alert(1)"], ["/signin"], ["/auth/logout"]])("drops a next that is not a same-origin page of the app (%s)", async (next) => {
    renderApp(`/signin?next=${encodeURIComponent(next)}`, silent);
    const link = await screen.findByRole("link", { name: "Sign in with Microsoft" });
    expect(link).toHaveAttribute("href", "/auth/login");
  });

  it("draws no navigation at all and asks the hub for nothing", async () => {
    const view = renderApp("/signin", silent);
    await screen.findByRole("heading", { level: 1 });
    expect(screen.queryByRole("navigation")).toBeNull();
    expect(screen.queryByRole("banner")).toBeNull();
    // Give a stray effect the time it would need.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(view.calls).toEqual([]);
  });
});

describe.each([
  {
    path: "/access-requested",
    heading: "Access requested",
    badge: "Pending approval",
    words: "An admin must approve your access before you can use Lanekeeper. You'll be able to sign in as soon as they do.",
  },
  {
    path: "/access-disabled",
    heading: "Your access is disabled",
    badge: "Access disabled",
    words: "An admin turned off your account. You can't use Lanekeeper until it is turned back on.",
  },
])("the screen at $path", ({ path, heading, badge, words }) => {
  it("says what happened in the design's words, with its badge, under one h1", async () => {
    renderApp(path, silent);
    expect(await screen.findByRole("heading", { level: 1, name: heading })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByText(badge)).toBeInTheDocument();
    expect(screen.getByText(words)).toBeInTheDocument();
  });

  it("asks the hub for nothing and has no navigation: the hub answers a pending or disabled user 403 and names nobody", async () => {
    const view = renderApp(path, silent);
    await screen.findByRole("heading", { level: 1 });
    expect(screen.queryByRole("navigation")).toBeNull();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(view.calls).toEqual([]);
  });

  it("signs out by a request, with the CSRF token, and then shows the sign-in screen", async () => {
    const user = userEvent.setup();
    const view = renderApp(path, (call) => (call.path === "/auth/logout" ? new Response(null, { status: 204 }) : problem(401, "unauthenticated")));
    await user.click(await screen.findByRole("button", { name: "Sign out" }));
    await waitFor(() => expect(view.location()).toBe("/signin"));
    const logout = view.calls.find((c) => c.path === "/auth/logout");
    expect(logout?.method).toBe("POST");
    expect(logout?.headers.get("x-csrf-token")).toBeTruthy();
    expect(await screen.findByRole("heading", { level: 1, name: "Lanekeeper" })).toBeInTheDocument();
  });

  it("stays where it is and says so when the hub does not confirm the sign-out", async () => {
    const user = userEvent.setup();
    const view = renderApp(path, (call) => (call.path === "/auth/logout" ? problem(500, "internal") : problem(401, "unauthenticated")));
    await user.click(await screen.findByRole("button", { name: "Sign out" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not sign out");
    expect(view.location()).toBe(path);
  });
});

describe("the pending screen and the disabled screen", () => {
  it("do not promise what the contract cannot give: no request time, no name of an admin", async () => {
    renderApp("/access-requested", () => json({}));
    await screen.findByRole("heading", { level: 1 });
    expect(screen.queryByText(/Request sent/)).toBeNull();
    expect(screen.queryByText(/Signed in as/)).toBeNull();
  });

  it("tell a disabled person to contact an admin, without a name", async () => {
    renderApp("/access-disabled", () => json({}));
    expect(await screen.findByText("To get access back, contact an admin.")).toBeInTheDocument();
  });
});
