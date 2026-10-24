import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { json, me, problem, renderApp, type Api } from "@/test/app";

// The personal MCP tokens page (route /me/tokens). The list, the making and the revoking are the shared component's, and it has
// its own tests; what is proven here is the page around it: one h1, the right role, the mode that is not switched on (the hub
// answers the list with 404) as an empty state on a direct link, and a token that is shown once and kept out of every cache when
// it is made through this page.

const SECRET = "lkpat_ZxY9w8V7u6T5s4R3q2P1o0NmLkJiHgFeDcBaZxY9w8V7u6T5s4R3q2P1o0N";
const DAY = 86_400_000;

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

function server(role: string | null, mode: "on" | "off"): Api {
  const rows: unknown[] = [{ id: 1, label: "laptop", created_at: Date.now() - DAY, expires_at: Date.now() + 6 * DAY, revoked_at: null, last_used_at: null }];
  return (call) => {
    if (call.path === "/api/v1/me") return json(me(role));
    if (call.path.startsWith("/api/v1/swimlanes")) return json({ items: [], next: null });
    if (call.path.startsWith("/api/v1/me/mcp-tokens")) {
      if (mode === "off") return problem(404, "not_found");
      if (call.method === "POST") {
        rows.unshift({ id: 2, label: "ci", created_at: Date.now(), expires_at: Date.now() + 7 * DAY, revoked_at: null, last_used_at: null });
        return json({ id: 2, label: "ci", created_at: Date.now(), expires_at: Date.now() + 7 * DAY, revoked_at: null, last_used_at: null, token: SECRET }, 201);
      }
      return json({ items: rows, next: null });
    }
    return problem(404, "not_found");
  };
}

describe("the personal MCP tokens page", () => {
  it("is one h1 over the shared tokens component, for a Viewer too", async () => {
    renderApp("/me/tokens", server("viewer", "on"));
    expect(await screen.findByRole("heading", { level: 1, name: "Personal MCP tokens" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(await screen.findByRole("list", { name: "Personal MCP tokens" })).toBeInTheDocument();
    expect(screen.getByText("laptop")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create token" })).toBeInTheDocument();
  });

  it("says the mode is not switched on, with no form, when the hub answers the list with 404 (a direct link to a hidden page)", async () => {
    renderApp("/me/tokens", server("viewer", "off"));
    expect(await screen.findByRole("heading", { level: 1, name: "Personal MCP tokens" })).toBeInTheDocument();
    expect(await screen.findByText("Personal MCP tokens are not switched on")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create token" })).toBeNull();
    expect(screen.queryByText("Couldn't load personal tokens")).toBeNull();
  });

  it("shows a token once, and no query or mutation of the app holds it", async () => {
    const user = userEvent.setup();
    const view = renderApp("/me/tokens", server("editor", "on"));
    await user.type(await screen.findByLabelText("Label"), "ci");
    await user.click(screen.getByRole("button", { name: "Create token" }));
    expect(await screen.findByLabelText("Your new token")).toHaveValue(SECRET);
    const held = JSON.stringify([view.queryClient.getQueryCache().getAll().map((q) => q.state.data), view.queryClient.getMutationCache().getAll().map((m) => [m.state.variables, m.state.data])]);
    expect(held).not.toContain(SECRET);
    await user.click(screen.getByRole("button", { name: "I have stored it" }));
    await waitFor(() => expect(screen.queryByLabelText("Your new token")).toBeNull());
  });

  it("makes no request for a token list before the person has a session", async () => {
    const view = renderApp("/me/tokens", () => problem(401, "unauthenticated"));
    await waitFor(() => expect(view.location()).toMatch(/^\/signin/));
    expect(view.calls.some((c) => c.path.startsWith("/api/v1/me/mcp-tokens"))).toBe(false);
  });
});
