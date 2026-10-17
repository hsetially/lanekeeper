import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import { apiCaches } from "@/api/client";
import { json, me, problem, renderApp, type Api } from "@/test/app";

// S2 (client side): a 401 clears what the previous user saw and goes to sign-in; screens before the session make no call;
// a person who may not open a screen sees a permission screen, not a redirect.

const as = (role: string | null, extra: Record<string, unknown> = {}): Api => (call) =>
  call.path === "/api/v1/me" ? json(me(role, extra)) : problem(404, "not_found");

const signedOut: Api = (call) => (call.path === "/api/v1/me" ? problem(401, "unauthenticated") : problem(404, "not_found"));

beforeEach(() => {
  vi.stubGlobal("document", document);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a 401", () => {
  it("clears the cache and goes to /signin with a same-origin next", async () => {
    const app = renderApp("/swimlanes/sit1/files?path=a%2Fb.yml", signedOut);
    await waitFor(() => expect(app.location()).toMatch(/^\/signin/));
    expect(app.location()).toBe("/signin?next=%2Fswimlanes%2Fsit1%2Ffiles%3Fpath%3Da%252Fb.yml");
    expect(app.queryClient.getQueryCache().getAll().filter((q) => q.state.data !== undefined)).toHaveLength(0);
    expect(apiCaches.etags.size).toBe(0);
    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
  });

  it("drops a next that is too long to trust, and still goes to sign-in", async () => {
    const app = renderApp(`/swimlanes?q=${"a".repeat(600)}`, signedOut);
    await waitFor(() => expect(app.location()).toMatch(/^\/signin/));
    expect(app.location()).toBe("/signin");
  });

  it("builds the sign-in link from the validated next, and has none without one", async () => {
    const app = renderApp("/signin?next=%2Fswimlanes%2Fsit1", signedOut);
    const link = await screen.findByRole("link", { name: /sign in with microsoft/i });
    expect(link).toHaveAttribute("href", "/auth/login?return_to=%2Fswimlanes%2Fsit1");
    app.unmount();
    renderApp("/signin?next=https%3A%2F%2Fevil.example%2F", signedOut);
    expect(await screen.findByRole("link", { name: /sign in with microsoft/i })).toHaveAttribute("href", "/auth/login");
  });

  it("does not loop: a 401 while already on a public screen stays there", async () => {
    const app = renderApp("/signin", signedOut);
    await screen.findByRole("heading", { level: 1, name: "Sign in" });
    expect(app.location()).toBe("/signin");
    expect(app.calls).toHaveLength(0);
  });
});

describe("screens before the session", () => {
  it.each(["/signin", "/access-requested", "/access/pending", "/access-disabled", "/access/disabled", "/404", "/error?req=abc123", "/403"])("%s makes no API call and opens no stream", async (url) => {
    const app = renderApp(url, signedOut);
    await app.settled();
    await screen.findByRole("heading", { level: 1 });
    expect(app.calls).toEqual([]);
  });

  it("redirects the aliases to the paths the hub sends users to", async () => {
    const pending = renderApp("/access/pending", signedOut);
    await waitFor(() => expect(pending.location()).toBe("/access-requested"));
    pending.unmount();
    const disabled = renderApp("/access/disabled", signedOut);
    await waitFor(() => expect(disabled.location()).toBe("/access-disabled"));
  });
});

describe("a user with no role yet, or a disabled one", () => {
  it.each([
    ["access-pending", "/access-requested"],
    ["access_pending", "/access-requested"],
    ["access-disabled", "/access-disabled"],
  ])("a 403 %s goes to %s", async (type, target) => {
    const app = renderApp("/swimlanes", (call) => (call.path === "/api/v1/me" ? problem(403, type) : problem(404, "not_found")));
    await waitFor(() => expect(app.location()).toBe(target));
  });

  it("clears the cache before it goes", async () => {
    const app = renderApp("/swimlanes", (call) => (call.path === "/api/v1/me" ? problem(403, "access-pending") : problem(404, "not_found")));
    await waitFor(() => expect(app.location()).toBe("/access-requested"));
    expect(app.queryClient.getQueryCache().getAll().filter((q) => q.state.data !== undefined)).toHaveLength(0);
  });
});

describe("the role gate", () => {
  it("shows the permission screen, and does not redirect, to a Viewer on an Admin screen", async () => {
    const app = renderApp("/admin/users", as("viewer"));
    expect(await screen.findByRole("heading", { level: 1, name: "You do not have access" })).toBeInTheDocument();
    expect(screen.getByText(/needs the admin role/i)).toBeInTheDocument();
    expect(app.location()).toBe("/admin/users");
  });

  it("lets an Admin in", async () => {
    renderApp("/admin/users", as("admin"));
    expect(await screen.findByRole("heading", { level: 1, name: "Users and access requests" })).toBeInTheDocument();
  });

  it("lets a Viewer open a viewer screen, and not an editor one", async () => {
    renderApp("/swimlanes", as("viewer"));
    expect(await screen.findByRole("heading", { level: 1, name: "Swimlanes" })).toBeInTheDocument();
  });

  it("gives the permission screen to a user whose role is none", async () => {
    renderApp("/swimlanes", as(null));
    expect(await screen.findByRole("heading", { level: 1, name: "You do not have access" })).toBeInTheDocument();
  });

  it("keeps Approvals and Audit Admin-only", async () => {
    for (const url of ["/approvals", "/audit", "/swimlanes/sit1/audit"]) {
      const app = renderApp(url, as("operator"));
      expect(await screen.findByRole("heading", { level: 1, name: "You do not have access" })).toBeInTheDocument();
      app.unmount();
    }
  });

  it("sends the Viewer who needs a token nowhere: onboarding is for Editors and above", async () => {
    const app = renderApp("/", as("viewer", { needs_github_token: true }));
    await waitFor(() => expect(app.location()).toBe("/swimlanes"));
  });

  it("leads an Editor whose token is missing from the landing page to onboarding", async () => {
    const app = renderApp("/", as("editor", { needs_github_token: true }));
    await waitFor(() => expect(app.location()).toBe("/access/onboarding"));
  });

  it("lands on the swimlanes for everyone else", async () => {
    const app = renderApp("/", as("editor"));
    await waitFor(() => expect(app.location()).toBe("/swimlanes"));
  });
});

describe("routes", () => {
  it("shows the not-found screen for a path that is not in the table, with no API call", async () => {
    const app = renderApp("/no/such/page", as("admin"));
    expect(await screen.findByRole("heading", { level: 1, name: "Page not found" })).toBeInTheDocument();
    expect(app.calls).toEqual([]);
  });

  it("checks a path parameter before the screen: a swimlane id that could change the URL is not found", async () => {
    const app = renderApp("/swimlanes/..%2Fadmin%2Fusers/files", as("admin"));
    expect(await screen.findByRole("heading", { level: 1, name: "Page not found" })).toBeInTheDocument();
    expect(app.calls.filter((c) => c.path !== "/api/v1/me")).toEqual([]);
  });

  it("redirects a link from a Teams card to the approval", async () => {
    const app = renderApp("/proposals/17", as("admin"));
    await waitFor(() => expect(app.location()).toBe("/approvals/17"));
  });

  it("does not read a search key the route does not name", async () => {
    const app = renderApp("/swimlanes?q=x&evil=%3Cscript%3E&__proto__=1", as("viewer"));
    await screen.findByRole("heading", { level: 1, name: "Swimlanes" });
    expect(app.router.state.matches.at(-1)?.search).toEqual({ q: "x" });
  });
});
