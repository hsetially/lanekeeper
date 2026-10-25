import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { json, me, problem, renderApp, type Api, type Call } from "@/test/app";

// The first run (the design's "GitHub token onboarding", frames 1g to 1r): an Editor or above whose token is missing is sent here
// from `/`, pastes a token, and sees only the login and the expiry once it is saved. A Viewer cannot use a token: the screen is
// the permission state, and the menu has no entry for it.

const SECRET = "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const DAY = 86_400_000;

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

type Options = { role?: string | null; needsToken?: boolean; put?: (call: Call) => Response; githubLogin?: string | null };
function server({ role = "editor", needsToken = true, put, githubLogin = "mdiaz" }: Options = {}): Api {
  let stored = !needsToken;
  return (call) => {
    if (call.path === "/api/v1/me") return json(me(role, { display_name: "Marco Diaz", github_login: githubLogin, needs_github_token: !stored }));
    if (call.path.startsWith("/api/v1/swimlanes")) return json({ items: [], next: null });
    if (call.path === "/api/v1/me/github-token" && call.method === "PUT") {
      const answer = put?.(call) ?? json({ github_login: "mdiaz", verified_at: Date.now(), scopes: ["push:acme/configuration-base-saas", "push:acme/csp-tenant-data"], expires_at: Date.now() + 90 * DAY - 60_000, expires_soon: false });
      if (answer.ok) stored = true;
      return answer;
    }
    if (call.path === "/api/v1/me/github-token") return problem(404, "not_found");
    return problem(404, "not_found");
  };
}

describe("the onboarding screen", () => {
  it("asks an Editor for a token under one h1, with the design's intro, the instructions and the field", async () => {
    renderApp("/access/onboarding", server());
    expect(await screen.findByRole("heading", { level: 1, name: "Add your GitHub token" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByText("Lanekeeper raises pull requests as you, so it needs a fine-grained token that you create. It's stored encrypted and never shown again.")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "1 · Create the token on GitHub with these settings" })).toBeInTheDocument();
    const field = screen.getByLabelText("2 · Paste the token");
    expect(field).toHaveAttribute("type", "password");
    expect(screen.getByRole("button", { name: "Verify and save" })).toBeInTheDocument();
  });

  it("is where `/` sends an Editor whose token is missing, and where a Viewer is never sent", async () => {
    const editor = renderApp("/", server());
    await waitFor(() => expect(editor.location()).toBe("/access/onboarding"));
    editor.unmount();
    const viewer = renderApp("/", server({ role: "viewer", needsToken: true }));
    await waitFor(() => expect(viewer.location()).toBe("/swimlanes"));
  });

  it("shows what was stored, and only that, once the hub verified the token: the login, the expiry, and the way on", async () => {
    const user = userEvent.setup();
    const view = renderApp("/access/onboarding", server());
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));

    const done = await screen.findByRole("status", { name: /Token verified and saved/ });
    expect(within(done).getByText("Token verified and saved")).toBeInTheDocument();
    expect(done).toHaveTextContent("Linked GitHub login");
    expect(done).toHaveTextContent("mdiaz");
    expect(done).toHaveTextContent("Expires");
    expect(done).toHaveTextContent("in 90 days");
    expect(done).toHaveTextContent("The token itself is stored encrypted and can't be viewed again. To change it later, use My GitHub token.");
    expect(within(done).getByRole("link", { name: "Continue to Lanekeeper" })).toHaveAttribute("href", "/swimlanes");
    // A link in the middle of a sentence is underlined, not told apart by its colour alone (axe `link-in-text-block`).
    expect(within(done).getByRole("link", { name: "My GitHub token" }).className).toMatch(/(^|\s)underline(\s|$)/);
    // The form is gone, the token is nowhere in the page, and the user was asked again so that the rest of the app knows.
    expect(screen.queryByLabelText("2 · Paste the token")).toBeNull();
    expect(document.body.textContent).not.toContain(SECRET);
    await waitFor(() => expect(view.calls.filter((c) => c.path === "/api/v1/me").length).toBeGreaterThan(1));
  });

  it("says when GitHub gave the token no expiry", async () => {
    const user = userEvent.setup();
    renderApp("/access/onboarding", server({ put: () => json({ github_login: "mdiaz", verified_at: Date.now(), scopes: [], expires_at: null, expires_soon: false }) }));
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    const done = await screen.findByRole("status", { name: /Token verified and saved/ });
    expect(done).toHaveTextContent("GitHub did not report an expiry");
  });

  it("goes on to the swimlanes from the success", async () => {
    const user = userEvent.setup();
    const view = renderApp("/access/onboarding", server());
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    await user.click(await screen.findByRole("link", { name: "Continue to Lanekeeper" }));
    await waitFor(() => expect(view.location()).toBe("/swimlanes"));
  });

  it("names the GitHub login linked to the person when the token belongs to another account", async () => {
    const user = userEvent.setup();
    renderApp("/access/onboarding", server({ put: () => problem(422, "github_account_mismatch") }));
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Linked to you");
    expect(alert).toHaveTextContent("mdiaz");
  });

  it("tells a person who already has a token that this replaces it, and where it is managed", async () => {
    renderApp("/access/onboarding", server({ needsToken: false }));
    expect(await screen.findByText(/You already have a GitHub token stored/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "My GitHub token" })).toHaveAttribute("href", "/me/github-token");
    expect(screen.getByRole("link", { name: "My GitHub token" }).className).toMatch(/(^|\s)underline(\s|$)/);
  });

  it("is the permission state for a Viewer, with no form and no request for a token", async () => {
    const view = renderApp("/access/onboarding", server({ role: "viewer" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Connect your GitHub token needs the Editor role" })).toBeInTheDocument();
    expect(screen.queryByLabelText("2 · Paste the token")).toBeNull();
    expect(view.calls.some((c) => c.path === "/api/v1/me/github-token")).toBe(false);
  });
});
