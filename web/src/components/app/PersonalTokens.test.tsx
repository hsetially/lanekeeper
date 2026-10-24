import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { apiCaches } from "@/api/client";
import { createQueryClient } from "@/lib/query/client";
import { PersonalTokens } from "./PersonalTokens";

// S7, S8, S21 in the browser: the one-time value of a personal MCP token lives only in this component's state, never in the
// query cache or the mutation cache, whatever the outcome; the list, the five-token limit, revoking, and the mode that is not
// switched on (the hub answers 404) are all here, once, for the two screens that mount it.

const SECRET = "lk_mcp_ZxY9w8V7u6T5s4R3q2P1o0NmLkJiHgFeDcBa";
const NOW = 1_800_000_000_000;
const DAY = 86_400_000;

type Row = { id: number; label: string; created_at: number; expires_at: number; revoked_at: number | null; last_used_at: number | null };
let rows: Row[];
let calls: Array<{ method: string; path: string; body: string | undefined; headers: Headers }>;
let create: () => Response;
let client: QueryClient;

const ok = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const problem = (status: number, type: string, title: string) => new Response(JSON.stringify({ type: `urn:lanekeeper:problem:${type}`, title, status, request_id: "req-tok-1" }), { status, headers: { "content-type": "application/problem+json" } });

function mount(heading?: string) {
  client = createQueryClient();
  const view = render(
    <QueryClientProvider client={client}>
      <PersonalTokens now={() => NOW} {...(heading === undefined ? {} : { heading })} />
    </QueryClientProvider>,
  );
  return view;
}

const secretsIn = (c: QueryClient): string =>
  JSON.stringify([c.getQueryCache().getAll().map((q) => q.state.data), c.getMutationCache().getAll().map((m) => [m.state.variables, m.state.data, m.state.error])]);

beforeEach(() => {
  apiCaches.clear();
  Object.defineProperty(document, "cookie", { configurable: true, get: () => "__Host-lk_csrf=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde", set: () => undefined });
  rows = [
    { id: 1, label: "laptop", created_at: NOW - 2 * DAY, expires_at: NOW + 5 * DAY, revoked_at: null, last_used_at: NOW - 3_600_000 },
    { id: 2, label: "old", created_at: NOW - 20 * DAY, expires_at: NOW - 13 * DAY, revoked_at: null, last_used_at: null },
    { id: 3, label: "gone", created_at: NOW - 3 * DAY, expires_at: NOW + 4 * DAY, revoked_at: NOW - DAY, last_used_at: null },
  ];
  calls = [];
  create = () => ok({ id: 9, label: "ci", created_at: NOW, expires_at: NOW + 7 * DAY, revoked_at: null, last_used_at: null, token: SECRET }, 201);
  const respond = (input: string, init: RequestInit): Response => {
      const method = (init.method ?? "GET").toUpperCase();
      calls.push({ method, path: input, body: typeof init.body === "string" ? init.body : undefined, headers: new Headers(init.headers) });
      if (input.startsWith("/api/v1/me/mcp-tokens") && method === "GET") return ok({ items: rows, next: null });
      if (input === "/api/v1/me/mcp-tokens" && method === "POST") return create();
      if (/^\/api\/v1\/me\/mcp-tokens\/\d+$/.test(input) && method === "DELETE") return new Response(null, { status: 204 });
      return problem(404, "not_found", "Not found");
  };
  vi.stubGlobal("fetch", vi.fn((input: string, init: RequestInit = {}) => Promise.resolve(respond(input, init))));
});
afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

describe("the heading", () => {
  it("is 'Personal MCP tokens' unless the screen that mounts it already says so in its own title", async () => {
    mount();
    expect(await screen.findByRole("heading", { level: 2, name: "Personal MCP tokens" })).toBeInTheDocument();
  });

  it("is the one a screen gives it, and names the section", async () => {
    mount("Your tokens");
    expect(await screen.findByRole("heading", { level: 2, name: "Your tokens" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Your tokens" })).toBeInTheDocument();
    // The list keeps its own name, so a screen reader still hears what the tokens are for.
    expect(await screen.findByRole("list", { name: "Personal MCP tokens" })).toBeInTheDocument();
  });
});

describe("the list", () => {
  it("shows each token's label, when it was made, when it ends, when it was last used and whether it works", async () => {
    mount();
    const list = await screen.findByRole("list", { name: "Personal MCP tokens" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(items[0]).toHaveTextContent("laptop");
    expect(items[0]).toHaveTextContent("Active");
    expect(items[0]).toHaveTextContent(/expires in 5 d/i);
    expect(items[0]).toHaveTextContent(/used 1 h ago/i);
    expect(items[1]).toHaveTextContent("Expired");
    expect(items[1]).toHaveTextContent(/never used/i);
    expect(items[2]).toHaveTextContent("Revoked");
  });

  it("offers Revoke only for a token that works", async () => {
    mount();
    const list = await screen.findByRole("list", { name: "Personal MCP tokens" });
    const items = within(list).getAllByRole("listitem");
    expect(within(items[0] as HTMLElement).getByRole("button", { name: "Revoke laptop" })).toBeInTheDocument();
    expect(within(items[1] as HTMLElement).queryByRole("button")).toBeNull();
    expect(within(items[2] as HTMLElement).queryByRole("button")).toBeNull();
  });

  it("says there are none when there are none", async () => {
    rows = [];
    mount();
    expect(await screen.findByText("No personal tokens yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create token" })).toBeInTheDocument();
  });

  it("says the mode is off, and shows no form, when the hub answers 404", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(problem(404, "not_found", "Not found"))));
    mount();
    expect(await screen.findByText("Personal MCP tokens are not switched on")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create token" })).toBeNull();
  });

  it("says it could not load on another failure, with the request id", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(problem(500, "internal", "Internal error"))));
    mount();
    expect(await screen.findByText("Couldn't load personal tokens", {}, { timeout: 5000 })).toBeInTheDocument();
    expect(screen.getByText("req-tok-1")).toBeInTheDocument();
  });
});

describe("making a token", () => {
  it("sends the label with the CSRF token and an Idempotency-Key, shows the token once, and keeps it out of every cache", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await user.type(screen.getByLabelText("Label"), "ci");
    await user.click(screen.getByRole("button", { name: "Create token" }));
    const field = await screen.findByLabelText("Your new token");
    expect(field).toHaveValue(SECRET);
    expect(field).toHaveAttribute("readonly");
    expect(screen.getByText("Copy your new token now")).toBeInTheDocument();
    const post = calls.find((c) => c.method === "POST");
    expect(post?.path).toBe("/api/v1/me/mcp-tokens");
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ label: "ci" });
    expect(post?.headers.get("x-csrf-token")).toBeTruthy();
    expect(post?.headers.get("idempotency-key")).toBeTruthy();
    // While it is on screen it is in the component's state and nowhere else.
    expect(secretsIn(client)).not.toContain(SECRET);
    // The list is refreshed, so the new token appears in it (without its value).
    await waitFor(() => expect(calls.filter((c) => c.method === "GET").length).toBeGreaterThan(1));
  });

  it("has a copy button for the token (the human's decision of 2026-10-17): one click copies it, says it will not be shown again, and keeps it out of every cache", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await user.type(screen.getByLabelText("Label"), "ci");
    await user.click(screen.getByRole("button", { name: "Create token" }));
    await screen.findByLabelText("Your new token");
    // Showing the token copies nothing.
    expect(await navigator.clipboard.readText()).toBe("");
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(await navigator.clipboard.readText()).toBe(SECRET);
    expect(screen.getByText("Copied. It will not be shown again.")).toBeInTheDocument();
    expect(secretsIn(client)).not.toContain(SECRET);
    // Only the field that shows it holds it.
    expect(document.querySelectorAll(`[value="${SECRET}"]`)).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "I have stored it" }));
    expect(screen.queryByRole("button", { name: /Copy token|Copied/ })).toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(document.body.textContent).not.toContain("will not be shown again");
  });

  it("forgets the token when it is dismissed", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await user.type(screen.getByLabelText("Label"), "ci");
    await user.click(screen.getByRole("button", { name: "Create token" }));
    await screen.findByLabelText("Your new token");
    await user.click(screen.getByRole("button", { name: "I have stored it" }));
    expect(screen.queryByLabelText("Your new token")).toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(secretsIn(client)).not.toContain(SECRET);
  });

  it("forgets the token when the screen goes away", async () => {
    const user = userEvent.setup();
    const view = mount();
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await user.type(screen.getByLabelText("Label"), "ci");
    await user.click(screen.getByRole("button", { name: "Create token" }));
    await screen.findByLabelText("Your new token");
    view.unmount();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(secretsIn(client)).not.toContain(SECRET);
  });

  it("says the limit has been reached on a 409, keeps the label, shows the request id and no token", async () => {
    create = () => problem(409, "conflict", "Too many tokens");
    const user = userEvent.setup();
    mount();
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await user.type(screen.getByLabelText("Label"), "sixth");
    await user.click(screen.getByRole("button", { name: "Create token" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/five|limit/i);
    expect(alert).toHaveTextContent("req-tok-1");
    expect(screen.getByLabelText("Label")).toHaveValue("sixth");
    expect(screen.queryByLabelText("Your new token")).toBeNull();
    expect(secretsIn(client)).not.toContain(SECRET);
  });

  it("will not send an empty label", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await user.click(screen.getByRole("button", { name: "Create token" }));
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    expect(screen.getByLabelText("Label")).toHaveAttribute("aria-invalid", "true");
  });

  it("keeps the token out of the mutation cache even when the request fails after the answer was read", async () => {
    create = () => problem(500, "internal", "Internal error");
    const user = userEvent.setup();
    mount();
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await user.type(screen.getByLabelText("Label"), "ci");
    await user.click(screen.getByRole("button", { name: "Create token" }));
    await screen.findByRole("alert");
    expect(secretsIn(client)).not.toContain("ci");
  });
});

describe("revoking", () => {
  it("asks first, names the consequence, then deletes the token and refreshes the list", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByRole("list", { name: "Personal MCP tokens" });
    await user.click(screen.getByRole("button", { name: "Revoke laptop" }));
    const dialog = await screen.findByRole("alertdialog", { name: /Revoke/ });
    expect(dialog).toHaveTextContent(/stops working at once/i);
    rows = rows.map((r) => (r.id === 1 ? { ...r, revoked_at: NOW } : r));
    await user.click(within(dialog).getByRole("button", { name: "Revoke token" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/api/v1/me/mcp-tokens/1")).toBe(true));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(within(screen.getAllByRole("listitem")[0] as HTMLElement).getByText("Revoked")).toBeInTheDocument());
  });
});
