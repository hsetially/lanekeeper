import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GithubCredential } from "@/api/generated/model";
import { json, me, problem, renderApp, type Api, type Call } from "@/test/app";

// "My GitHub token" (the design's frames 1s and 1t): what is stored of the person's token, never the token; the warning when it
// is about to end or has ended; replacing it; deleting it. Every state of the screen is here: loading, error with the request id,
// nothing stored, healthy, ending soon, ended, and the permission state for a Viewer.

const SECRET = "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const DAY = 86_400_000;

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

const credential = (over: Partial<GithubCredential> = {}): GithubCredential => ({
  github_login: "mdiaz",
  verified_at: Date.now() - 3_600_000,
  scopes: ["push:acme/configuration-base-saas", "push:acme/csp-tenant-data"],
  expires_at: Date.now() + 60 * DAY - 60_000,
  expires_soon: false,
  ...over,
});

type World = { stored: GithubCredential | null; role?: string; get?: (call: Call) => Response | undefined; put?: (call: Call) => Response | undefined; del?: (call: Call) => Response | undefined };
function server(world: World): Api {
  return (call) => {
    if (call.path === "/api/v1/me") return json(me(world.role ?? "editor", { display_name: "Marco Diaz", github_login: "mdiaz", needs_github_token: world.stored === null }));
    if (call.path.startsWith("/api/v1/swimlanes")) return json({ items: [], next: null });
    if (call.path === "/api/v1/me/github-token") {
      if (call.method === "GET") return world.get?.(call) ?? (world.stored === null ? problem(404, "not_found") : json(world.stored));
      if (call.method === "PUT") {
        const answer = world.put?.(call);
        if (answer) return answer;
        world.stored = credential({ github_login: "mdiaz2" });
        return json(world.stored);
      }
      if (call.method === "DELETE") {
        const answer = world.del?.(call);
        if (answer) return answer;
        world.stored = null;
        return new Response(null, { status: 204 });
      }
    }
    return problem(404, "not_found");
  };
}

const heading = () => screen.findByRole("heading", { level: 1, name: "My GitHub token" });

describe("the page", () => {
  it("shows its h1 and what the token is for before the data arrives", async () => {
    let release: (response: Response) => void = () => undefined;
    renderApp("/me/github-token", (call) => (call.path === "/api/v1/me/github-token" ? new Promise<Response>((resolve) => (release = resolve)) : server({ stored: credential() })(call)));
    expect(await heading()).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByText("Used to raise pull requests as you in configuration-base-saas and csp-tenant-data.")).toBeInTheDocument();
    expect(await screen.findByLabelText("Loading your GitHub token")).toHaveAttribute("aria-busy", "true");
    release(json(credential()));
    expect(await screen.findByText("Linked GitHub login")).toBeInTheDocument();
    expect(screen.queryByLabelText("Loading your GitHub token")).toBeNull();
  });

  it("shows what is stored: the login, when it ends, when the hub checked it, the repositories it can push to, and that it cannot be seen", async () => {
    renderApp("/me/github-token", server({ stored: credential() }));
    await heading();
    const login = await screen.findByText("Linked GitHub login");
    const card = login.closest("dl") as HTMLElement;
    expect(card).toHaveTextContent("mdiaz");
    expect(card).toHaveTextContent("Expires");
    expect(card).toHaveTextContent("in 60 days");
    expect(card).toHaveTextContent("Verified");
    expect(card).toHaveTextContent("1 h ago");
    expect(card).toHaveTextContent("Verified access");
    expect(card).toHaveTextContent("acme/configuration-base-saas");
    expect(card).toHaveTextContent("acme/csp-tenant-data");
    expect(card).toHaveTextContent("Push access");
    expect(card).toHaveTextContent("Stored encrypted. It can't be viewed or copied.");
    expect(screen.getByRole("button", { name: "Replace token" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete token" })).toBeInTheDocument();
    expect(screen.getByText("Deleting means you can't save or raise PRs until you add a new one.")).toBeInTheDocument();
    // Nothing to warn about.
    expect(screen.queryByText(/expires in/i)).toBeNull();
  });

  it("does not draw the design's Added and Last used rows: the contract has no such data", async () => {
    renderApp("/me/github-token", server({ stored: credential() }));
    const card = (await screen.findByText("Linked GitHub login")).closest("dl") as HTMLElement;
    expect(card).not.toHaveTextContent("Added");
    expect(card).not.toHaveTextContent("Last used");
  });

  it("warns when the token ends within a week: a banner with Replace token, and a badge, in words and an icon", async () => {
    renderApp("/me/github-token", server({ stored: credential({ expires_at: Date.now() + 5 * DAY - 60_000, expires_soon: true }) }));
    const banner = await screen.findByText("Your GitHub token expires in 5 days");
    const alert = banner.closest("[role='status']") as HTMLElement;
    expect(alert).toHaveTextContent("Save and Raise PR stop working");
    expect(within(alert).getByRole("button", { name: "Replace token" })).toBeInTheDocument();
    expect(screen.getByText("Expires in 5 days", { selector: "[data-slot='badge']" })).toBeInTheDocument();
  });

  it("says when the token has ended, and that Save and Raise PR do not work until it is replaced", async () => {
    renderApp("/me/github-token", server({ stored: credential({ expires_at: Date.now() - DAY, expires_soon: true }) }));
    const banner = await screen.findByText("Your GitHub token has expired");
    const alert = banner.closest("[role='alert']") as HTMLElement;
    expect(alert).toHaveTextContent("Save and Raise PR don't work until you replace it.");
    expect(within(alert).getByRole("button", { name: "Replace token" })).toBeInTheDocument();
    expect(screen.getByText("Expired", { selector: "[data-slot='badge']" })).toBeInTheDocument();
  });

  it("opens the replace form from the banner's button too", async () => {
    const user = userEvent.setup();
    renderApp("/me/github-token", server({ stored: credential({ expires_at: Date.now() + 5 * DAY, expires_soon: true }) }));
    const banner = (await screen.findByText("Your GitHub token expires in 5 days")).closest("[role='status']") as HTMLElement;
    await user.click(within(banner).getByRole("button", { name: "Replace token" }));
    expect(await screen.findByLabelText("2 · Paste the token")).toBeInTheDocument();
  });

  it("says nothing is stored on a 404, and offers the form to add a token", async () => {
    renderApp("/me/github-token", server({ stored: null }));
    expect(await screen.findByText("No GitHub token stored")).toBeInTheDocument();
    expect(screen.getByLabelText("2 · Paste the token")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete token" })).toBeNull();
    expect(screen.queryByText("Couldn't load your GitHub token")).toBeNull();
  });

  it("shows the problem's title and the request id on any other failure, with Retry", async () => {
    const user = userEvent.setup();
    let fail = true;
    renderApp("/me/github-token", server({ stored: credential(), get: () => (fail ? problem(500, "internal") : undefined) }));
    expect(await screen.findByText("Couldn't load your GitHub token")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    fail = false;
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Linked GitHub login")).toBeInTheDocument();
  });

  it("keeps showing what is stored, and says it could not refresh, when a later fetch fails", async () => {
    let fail = false;
    const view = renderApp("/me/github-token", server({ stored: credential(), get: () => (fail ? problem(500, "internal") : undefined) }));
    await screen.findByText("Linked GitHub login");
    fail = true;
    await act(async () => {
      await view.queryClient.invalidateQueries({ queryKey: ["me-github-credential"] });
    });
    expect(await screen.findByText("Couldn't refresh")).toBeInTheDocument();
    expect(screen.getByText("Linked GitHub login")).toBeInTheDocument();
    expect(screen.getByText("mdiaz", { selector: "bdi" })).toBeInTheDocument();
  });

  it("is the permission state for a Viewer, who has no token to show", async () => {
    const view = renderApp("/me/github-token", server({ stored: null, role: "viewer" }));
    expect(await screen.findByRole("heading", { level: 1, name: "My GitHub token needs the Editor role" })).toBeInTheDocument();
    expect(view.calls.some((c) => c.path === "/api/v1/me/github-token")).toBe(false);
  });
});

describe("adding a token", () => {
  it("stores it, says so, and then shows what is stored", async () => {
    const user = userEvent.setup();
    const view = renderApp("/me/github-token", server({ stored: null }));
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    expect(await screen.findByText("Token verified and saved")).toBeInTheDocument();
    expect(await screen.findByText("Linked GitHub login")).toBeInTheDocument();
    expect(screen.getByText("mdiaz2", { selector: "bdi" })).toBeInTheDocument();
    expect(screen.queryByLabelText("2 · Paste the token")).toBeNull();
    expect(JSON.stringify([view.queryClient.getQueryCache().getAll().map((q) => q.state.data), view.queryClient.getMutationCache().getAll()])).not.toContain(SECRET);
  });
});

describe("replacing the token", () => {
  it("opens the instructions and the field under the summary, and Cancel puts them away", async () => {
    const user = userEvent.setup();
    renderApp("/me/github-token", server({ stored: credential() }));
    await user.click(await screen.findByRole("button", { name: "Replace token" }));
    expect(await screen.findByLabelText("2 · Paste the token")).toHaveFocus();
    expect(screen.getByRole("button", { name: "Replace token" })).toHaveAttribute("aria-expanded", "true");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByLabelText("2 · Paste the token")).toBeNull();
    expect(screen.getByRole("button", { name: "Replace token" })).toHaveAttribute("aria-expanded", "false");
  });

  it("stores the new token, closes the form, says so, and shows the new login without a reload", async () => {
    const user = userEvent.setup();
    const view = renderApp("/me/github-token", server({ stored: credential() }));
    await user.click(await screen.findByRole("button", { name: "Replace token" }));
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    expect(await screen.findByText("Token replaced")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("mdiaz2", { selector: "bdi" })).toBeInTheDocument());
    expect(screen.queryByLabelText("2 · Paste the token")).toBeNull();
    expect(JSON.stringify([view.queryClient.getQueryCache().getAll().map((q) => q.state.data), view.queryClient.getMutationCache().getAll()])).not.toContain(SECRET);
  });

  it("keeps the stored token when the new one is refused, and says why", async () => {
    const user = userEvent.setup();
    renderApp("/me/github-token", server({ stored: credential(), put: () => problem(422, "classic_token") }));
    await user.click(await screen.findByRole("button", { name: "Replace token" }));
    await user.click(await screen.findByLabelText("2 · Paste the token"));
    await user.paste(SECRET);
    await user.click(screen.getByRole("button", { name: "Verify and save" }));
    expect(await screen.findByText("Classic tokens aren't allowed by policy")).toBeInTheDocument();
    expect(screen.getByText("Linked GitHub login")).toBeInTheDocument();
    expect(screen.getByText("mdiaz", { selector: "bdi" })).toBeInTheDocument();
  });
});

describe("deleting the token", () => {
  it("asks first, with the consequence said before the verb, and Cancel changes nothing", async () => {
    const user = userEvent.setup();
    const view = renderApp("/me/github-token", server({ stored: credential() }));
    await user.click(await screen.findByRole("button", { name: "Delete token" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete your GitHub token" });
    expect(dialog).toHaveTextContent("You can't save or raise PRs until you add a new token");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(view.calls.some((c) => c.method === "DELETE")).toBe(false);
    // The focus is back on the button that opened it.
    await waitFor(() => expect(screen.getByRole("button", { name: "Delete token" })).toHaveFocus());
  });

  it("deletes with the CSRF token and an Idempotency-Key, then shows that nothing is stored", async () => {
    const user = userEvent.setup();
    const view = renderApp("/me/github-token", server({ stored: credential() }));
    await user.click(await screen.findByRole("button", { name: "Delete token" }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete token" }));
    expect(await screen.findByText("No GitHub token stored")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(screen.queryByText("Linked GitHub login")).toBeNull();
    const del = view.calls.find((c) => c.method === "DELETE");
    expect(del?.path).toBe("/api/v1/me/github-token");
    expect(del?.headers.get("x-csrf-token")).toBeTruthy();
    expect(del?.headers.get("idempotency-key")).toBeTruthy();
    expect(screen.getByText("Your GitHub token was deleted.")).toBeInTheDocument();
    // The button is gone, so the focus is on the page's h1 and not lost to the body.
    await waitFor(() => expect(screen.getByRole("heading", { level: 1, name: "My GitHub token" })).toHaveFocus());
  });

  it("stays in the dialog with the problem's title and the request id when the hub refuses, and the token stays", async () => {
    const user = userEvent.setup();
    renderApp("/me/github-token", server({ stored: credential(), del: () => problem(503, "unavailable") }));
    await user.click(await screen.findByRole("button", { name: "Delete token" }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete token" }));
    expect(await within(dialog).findByText("req-test-1")).toBeInTheDocument();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(screen.getByText("Linked GitHub login")).toBeInTheDocument();
  });
});
