import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { json, me, problem, renderApp, type Api, type Call } from "@/test/app";

// Structure and names of every state of the access screens, by axe-core in jsdom (names, roles, ids, ARIA). Contrast, target size
// and the landmarks that need real layout run in Playwright (T4, phase C). The keyboard claims are here too: the form is
// submitted with Enter from the field, Escape closes the dialog, nothing needs a pointer.

const SECRET = "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const DAY = 86_400_000;

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

const credential = (over: Record<string, unknown> = {}) => ({ github_login: "mdiaz", verified_at: Date.now() - 3_600_000, scopes: ["push:acme/configuration-base-saas", "push:acme/csp-tenant-data"], expires_at: Date.now() + 60 * DAY, expires_soon: false, ...over });

function server(options: { role?: string; stored?: unknown; put?: (call: Call) => Response } = {}): Api {
  const { role = "editor", stored = null, put } = options;
  return (call) => {
    if (call.path === "/api/v1/me") return json(me(role, { display_name: "Marco Diaz", github_login: "mdiaz", needs_github_token: stored === null }));
    if (call.path.startsWith("/api/v1/swimlanes")) return json({ items: [], next: null });
    if (call.path === "/api/v1/me/github-token" && call.method === "GET") return stored === null ? problem(404, "not_found") : json(stored);
    if (call.path === "/api/v1/me/github-token" && call.method === "PUT") return put ? put(call) : json(credential());
    if (call.path.startsWith("/api/v1/me/mcp-tokens")) return json({ items: [{ id: 1, label: "laptop", created_at: Date.now() - DAY, expires_at: Date.now() + 6 * DAY, revoked_at: null, last_used_at: null }], next: null });
    return problem(404, "not_found");
  };
}

describe("before sign-in", () => {
  it.each([["/signin", "Lanekeeper"], ["/access-requested", "Access requested"], ["/access-disabled", "Your access is disabled"]])("%s has no violations", async (path, heading) => {
    renderApp(path, () => problem(401, "unauthenticated"));
    await screen.findByRole("heading", { level: 1, name: heading });
    await expectNoAxeViolations();
  });
});

describe("the token screens", () => {
  async function refuse(response: Response) {
    const user = userEvent.setup();
    renderApp("/access/onboarding", server({ put: () => response }));
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    await screen.findByRole("alert");
  }

  it("the onboarding, empty, has no violations", async () => {
    renderApp("/access/onboarding", server());
    await screen.findByRole("heading", { level: 1, name: "Add your GitHub token" });
    await expectNoAxeViolations();
  });

  it.each([
    ["classic", problem(422, "classic_token")],
    ["repository", problem(422, "token_missing_access", { base: true, tenant: false })],
    ["mismatch", problem(422, "github_account_mismatch")],
    ["rejected", problem(422, "token_rejected")],
    ["unavailable", problem(503, "unavailable")],
    ["retry", problem(422, "idempotency_conflict")],
  ])("the onboarding after a %s refusal has no violations, and the field is described by the answer", async (_name, response) => {
    await refuse(response);
    const field = screen.getByLabelText("2 · Paste the token");
    const described = (field.getAttribute("aria-describedby") ?? "").split(" ").map((id) => document.getElementById(id));
    expect(described.every((el) => el !== null)).toBe(true);
    expect(described.some((el) => el?.contains(screen.getByRole("alert")))).toBe(true);
    await expectNoAxeViolations();
  });

  it("the onboarding while verifying has no violations", async () => {
    const user = userEvent.setup();
    renderApp("/access/onboarding", (call) => (call.method === "PUT" ? new Promise<Response>(() => undefined) : server()(call)));
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    await screen.findByRole("button", { name: "Verifying…" });
    await expectNoAxeViolations();
  });

  it("the onboarding, saved, has no violations", async () => {
    const user = userEvent.setup();
    renderApp("/access/onboarding", server());
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    await screen.findByText("Token verified and saved");
    await expectNoAxeViolations();
  });

  it("sends the token when Enter is pressed in the field: no pointer is needed", async () => {
    const user = userEvent.setup();
    const view = renderApp("/access/onboarding", server());
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.keyboard("{Enter}");
    await screen.findByText("Token verified and saved");
    expect(view.calls.filter((c) => c.method === "PUT")).toHaveLength(1);
  });

  it.each([
    ["healthy", credential()],
    ["ending soon", credential({ expires_at: Date.now() + 5 * DAY, expires_soon: true })],
    ["expired", credential({ expires_at: Date.now() - DAY, expires_soon: true })],
  ])("My GitHub token, %s, has no violations", async (_name, stored) => {
    renderApp("/me/github-token", server({ stored }));
    await screen.findByText("Linked GitHub login");
    await expectNoAxeViolations();
  });

  it("My GitHub token, with nothing stored, has no violations", async () => {
    renderApp("/me/github-token", server());
    await screen.findByText("No GitHub token stored");
    await expectNoAxeViolations();
  });

  it("My GitHub token, with the replace form and the delete dialog open, has no violations", async () => {
    const user = userEvent.setup();
    renderApp("/me/github-token", server({ stored: credential() }));
    await user.click(await screen.findByRole("button", { name: "Replace token" }));
    await screen.findByLabelText("2 · Paste the token");
    await expectNoAxeViolations();
    await user.click(screen.getByRole("button", { name: "Delete token" }));
    await screen.findByRole("alertdialog", { name: "Delete your GitHub token" });
    await expectNoAxeViolations();
  });

  it("My GitHub token, failed to load, has no violations", async () => {
    renderApp("/me/github-token", (call) => (call.path === "/api/v1/me/github-token" ? problem(500, "internal") : server({ stored: credential() })(call)));
    await screen.findByText("Couldn't load your GitHub token");
    await expectNoAxeViolations();
  });

  it("the personal MCP tokens page has no violations", async () => {
    renderApp("/me/tokens", server({ role: "viewer" }));
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await expectNoAxeViolations();
  });
});

describe("the role change dialog", () => {
  it("has no violations while open and after the request was sent, closes with Escape, and gives the focus back", async () => {
    const user = userEvent.setup();
    const { RequestRoleButton } = await import("./RequestRoleDialog");
    const { QueryClientProvider } = await import("@tanstack/react-query");
    const { createQueryClient } = await import("@/lib/query/client");
    const { render } = await import("@testing-library/react");
    const { WriteGateProvider } = await import("@/components/app/write-gate");
    const { ROLE_ORDER } = await import("@/auth/roles");
    Object.defineProperty(document, "cookie", { configurable: true, get: () => "__Host-lk_csrf=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde", set: () => undefined });
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(json({ id: 1, user: "u", email: "a@example.test", requested_role: "editor", reason: "r", status: "pending", created_at: 1, decided_by: null, decided_at: null }, 202))));
    render(
      <QueryClientProvider client={createQueryClient()}>
        <WriteGateProvider value={{ roles: ROLE_ORDER, role: "viewer", online: true }}>
          <main>
            <h1>Admin</h1>
            <RequestRoleButton have="viewer" />
          </main>
        </WriteGateProvider>
      </QueryClientProvider>,
    );
    const trigger = screen.getByRole("button", { name: "Request a role change" });
    await user.click(trigger);
    await screen.findByRole("alertdialog");
    await expectNoAxeViolations();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(trigger).toHaveFocus();

    await user.click(trigger);
    const again = await screen.findByRole("alertdialog");
    await user.type(within(again).getByLabelText("Why you need it"), "to fix a typo");
    await user.click(within(again).getByRole("button", { name: "Send request" }));
    await screen.findByText("Request sent");
    await expectNoAxeViolations();
  });
});
