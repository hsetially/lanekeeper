import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { apiCaches } from "@/api/client";
import type { GithubCredential } from "@/api/generated/model";
import { ROLE_ORDER } from "@/auth/roles";
import { WriteGateProvider } from "@/components/app/write-gate";
import { createQueryClient } from "@/lib/query/client";
import { TokenSetup } from "./TokenSetup";

// The screen where a person pastes their GitHub token (S2, S7, S21 in the browser). What these tests prove, one per claim:
//  - the token is typed into a real masked field with a visible label, and goes to the hub once, with the CSRF token and an
//    Idempotency-Key, and is never retried;
//  - it is held by this component only: after success AND after every kind of failure it is in no query, no mutation, no
//    console line, and the field is empty again;
//  - each answer the hub can give to a token is one state of the screen (the code-to-state table), and "verifying" is one honest
//    line, not a progress bar.

const SECRET = "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const NOW = 1_760_000_000_000;
const DAY = 86_400_000;
const CREDENTIAL: GithubCredential = { github_login: "mdiaz", verified_at: NOW, scopes: ["push:acme/configuration-base-saas", "push:acme/csp-tenant-data"], expires_at: NOW + 90 * DAY, expires_soon: false };

type Call = { method: string; path: string; body: string | undefined; headers: Headers };
let calls: Call[];
let answer: (call: Call) => Response | Promise<Response>;
let client: QueryClient;
let onStored: ReturnType<typeof vi.fn<(credential: GithubCredential) => void>>;

const ok = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const problem = (status: number, type: string, extra: Record<string, unknown> = {}, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify({ type: `urn:lanekeeper:problem:${type}`, title: `Title of ${type}`, status, request_id: "req-tok-1", detail: "A detail the screen must not show", ...extra }), {
    status,
    headers: { "content-type": "application/problem+json", ...headers },
  });

const everythingTheClientHolds = (c: QueryClient): string =>
  JSON.stringify([c.getQueryCache().getAll().map((q) => [q.queryKey, q.state.data, q.state.error]), c.getMutationCache().getAll().map((m) => [m.options.mutationKey, m.state.variables, m.state.data, m.state.error])]);

function mount(props: { linkedLogin?: string | null; role?: string; online?: boolean } = {}) {
  client = createQueryClient();
  return render(
    <QueryClientProvider client={client}>
      <WriteGateProvider value={{ roles: ROLE_ORDER, role: props.role ?? "editor", online: props.online ?? true }}>
        <TokenSetup linkedLogin={props.linkedLogin === undefined ? "mdiaz" : props.linkedLogin} onStored={onStored} />
      </WriteGateProvider>
    </QueryClientProvider>,
  );
}

const field = () => screen.getByLabelText("2 · Paste the token");
async function paste(user: ReturnType<typeof userEvent.setup>, text = SECRET) {
  await user.click(field());
  await user.paste(text);
}
const verify = (user: ReturnType<typeof userEvent.setup>) => user.click(screen.getByRole("button", { name: "Verify and save" }));
const puts = () => calls.filter((c) => c.method === "PUT");

let consoleCalls: unknown[][];
beforeEach(() => {
  apiCaches.clear();
  Object.defineProperty(document, "cookie", { configurable: true, get: () => "__Host-lk_csrf=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde", set: () => undefined });
  calls = [];
  onStored = vi.fn<(credential: GithubCredential) => void>();
  answer = (call) => (call.method === "PUT" ? ok(CREDENTIAL) : problem(404, "not_found"));
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      const call: Call = { method: (init.method ?? "GET").toUpperCase(), path: input, body: typeof init.body === "string" ? init.body : undefined, headers: new Headers(init.headers) };
      calls.push(call);
      try {
        return Promise.resolve(answer(call));
      } catch (error) {
        return Promise.reject(error instanceof Error ? error : new Error("fetch failed"));
      }
    }),
  );
  consoleCalls = [];
  for (const name of ["log", "info", "warn", "error", "debug", "trace"] as const) vi.spyOn(console, name).mockImplementation((...args: unknown[]) => void consoleCalls.push(args));
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

describe("the instructions", () => {
  it("say what the token must be, in the design's words: fine-grained, the two repositories, the two permissions, an expiry", () => {
    mount();
    expect(screen.getByRole("heading", { level: 2, name: "1 · Create the token on GitHub with these settings" })).toBeInTheDocument();
    const dl = document.querySelector("dl");
    expect(dl).not.toBeNull();
    expect(dl).toHaveTextContent("Token type");
    expect(dl).toHaveTextContent("Fine-grained only. Classic tokens are rejected.");
    expect(dl).toHaveTextContent("configuration-base-saas");
    expect(dl).toHaveTextContent("csp-tenant-data");
    expect(dl).toHaveTextContent("Contents: Read and write");
    expect(dl).toHaveTextContent("Pull requests: Read and write");
    expect(dl).toHaveTextContent("Set an expiry date. Lanekeeper warns you 7 days before it ends.");
  });

  it("link to GitHub's token settings in a new tab that cannot reach back to this one", () => {
    mount();
    const link = screen.getByRole("link", { name: /Open token settings/ });
    expect(link).toHaveAttribute("href", "https://github.com/settings/personal-access-tokens/new");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link.getAttribute("rel")).toMatch(/noopener/);
    expect(link.getAttribute("rel")).toMatch(/noreferrer/);
  });
});

describe("the field", () => {
  it("is a real masked input with a visible label, that the browser does not remember, check or capitalise", () => {
    mount();
    const input = field();
    expect(input.tagName).toBe("INPUT");
    expect(input).toHaveAttribute("type", "password");
    expect(input).toHaveAttribute("autocomplete", "off");
    expect(input).toHaveAttribute("spellcheck", "false");
    expect(input).toHaveAttribute("placeholder", "github_pat_…");
    expect(input).not.toHaveAttribute("name");
    expect(screen.getByText("2 · Paste the token", { selector: "label" })).toBeVisible();
    expect(screen.getByText("The token is masked as you paste it. After saving, only your GitHub login and the expiry date are shown.")).toBeInTheDocument();
  });

  it("asks for a token before it asks the hub", async () => {
    const user = userEvent.setup();
    mount();
    await verify(user);
    expect(await screen.findByText("Paste the token first.")).toBeInTheDocument();
    expect(field()).toHaveAttribute("aria-invalid", "true");
    expect(calls).toHaveLength(0);
    await paste(user, "   ");
    await verify(user);
    expect(calls).toHaveLength(0);
  });

  it("refuses a value longer than any token before it is sent", async () => {
    const user = userEvent.setup();
    mount();
    await paste(user, "x".repeat(513));
    await verify(user);
    expect(await screen.findByText(/longer than a GitHub token can be/)).toBeInTheDocument();
    expect(calls).toHaveLength(0);
    expect(field()).toHaveValue("");
  });
});

describe("saving the token", () => {
  it("sends it once, with the CSRF token and an Idempotency-Key, and hands the answer to the page", async () => {
    const user = userEvent.setup();
    mount();
    await paste(user, `  ${SECRET}\n`);
    await verify(user);
    await waitFor(() => expect(onStored).toHaveBeenCalledWith(CREDENTIAL));
    expect(puts()).toHaveLength(1);
    const put = puts()[0];
    expect(put?.path).toBe("/api/v1/me/github-token");
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ token: SECRET });
    expect(put?.headers.get("x-csrf-token")).toBeTruthy();
    expect(put?.headers.get("idempotency-key")).toBeTruthy();
    expect(onStored).toHaveBeenCalledTimes(1);
  });

  it("leaves the token in no query, no mutation and no console line, and empties the field", async () => {
    const user = userEvent.setup();
    mount();
    await paste(user);
    await verify(user);
    await waitFor(() => expect(onStored).toHaveBeenCalled());
    expect(field()).toHaveValue("");
    expect(everythingTheClientHolds(client)).not.toContain(SECRET);
    expect(client.getMutationCache().getAll()).toHaveLength(0);
    expect(JSON.stringify(consoleCalls)).not.toContain(SECRET);
    expect(document.body.innerHTML).not.toContain(SECRET);
  });

  it("refreshes the user and the stored-token summary, so the rest of the app learns that a token exists", async () => {
    const user = userEvent.setup();
    mount();
    // What the shell and the page hold: the user and the credential.
    client.setQueryData(["me"], { needs_github_token: true });
    client.setQueryData(["me-github-credential"], undefined);
    await paste(user);
    await verify(user);
    await waitFor(() => expect(onStored).toHaveBeenCalled());
    expect(client.getQueryState(["me"])?.isInvalidated).toBe(true);
  });
});

describe("verifying", () => {
  it("does not fade the field: the state is the read-only background, because a faded token is under 4.5:1 (axe `color-contrast`)", async () => {
    const user = userEvent.setup();
    answer = () => new Promise<Response>(() => undefined);
    mount();
    await paste(user);
    await verify(user);
    await screen.findByRole("button", { name: "Verifying…" });
    expect(field().className).not.toMatch(/(^|\s)opacity-/);
    expect(field().className).toMatch(/(^|\s)bg-muted(\s|$)/);
  });

  it("is one honest line and a busy button, never a progress bar or a list of invented steps", async () => {
    const user = userEvent.setup();
    let release: (response: Response) => void = () => undefined;
    answer = () => new Promise<Response>((resolve) => (release = resolve));
    mount();
    await paste(user);
    await verify(user);

    const button = await screen.findByRole("button", { name: "Verifying…" });
    expect(button).toHaveAttribute("aria-busy", "true");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(field()).toHaveAttribute("readonly");
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("Verifying with GitHub");
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(within(status).queryAllByRole("listitem")).toHaveLength(0);
    expect(screen.getAllByRole("status")).toHaveLength(1);

    // A second press while it runs sends nothing more.
    await user.click(button);
    expect(puts()).toHaveLength(1);

    act(() => {
      release(ok(CREDENTIAL));
    });
    await waitFor(() => expect(onStored).toHaveBeenCalled());
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("what the hub can answer to a token (the code-to-state table)", () => {
  async function refused(response: Response) {
    const user = userEvent.setup();
    answer = () => response;
    mount();
    await paste(user);
    await verify(user);
    return user;
  }

  it.each(["classic_token", "not_fine_grained_token"])("shows %s as: classic tokens are not allowed by policy", async (type) => {
    await refused(problem(422, type));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Classic tokens aren't allowed by policy");
    expect(alert).toHaveTextContent("Classic tokens reach every repository you can. Create a fine-grained token with the settings above, then paste it here.");
    expect(onStored).not.toHaveBeenCalled();
  });

  it("shows token_missing_access with the repository the token cannot reach, and the one it can", async () => {
    await refused(problem(422, "token_missing_access", { base: true, tenant: false }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("This token can't reach csp-tenant-data");
    const rows = within(alert).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("configuration-base-saas");
    expect(rows[0]).toHaveTextContent("Read and write OK");
    expect(rows[1]).toHaveTextContent("csp-tenant-data");
    expect(rows[1]).toHaveTextContent("No access");
    expect(alert).toHaveTextContent("On GitHub, edit the token, add csp-tenant-data under Repository access, then verify again.");
  });

  it("names the base repository when that is the one missing, and both when neither is reachable", async () => {
    await refused(problem(422, "token_missing_access", { base: false, tenant: true }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This token can't reach configuration-base-saas");
  });

  it("says both when neither is reachable", async () => {
    await refused(problem(422, "token_missing_access", { base: false, tenant: false }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("This token can't reach configuration-base-saas or csp-tenant-data");
    expect(alert).toHaveTextContent("add configuration-base-saas and csp-tenant-data under Repository access");
  });

  it("does not claim a repository is reachable or not when the hub did not say which", async () => {
    await refused(problem(422, "token_missing_access"));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("This token can't push to both repositories");
    expect(alert).not.toHaveTextContent("Read and write OK");
    expect(alert).not.toHaveTextContent("No access");
  });

  it("shows github_account_mismatch with the login that is linked to the person, and does not invent the other one", async () => {
    await refused(problem(422, "github_account_mismatch"));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("This token belongs to a different GitHub account than the one linked to you");
    expect(alert).toHaveTextContent("Linked to you");
    expect(alert).toHaveTextContent("mdiaz");
    expect(alert).not.toHaveTextContent("Token belongs to");
    expect(alert).toHaveTextContent("Create the token while signed in to GitHub as mdiaz. If the linked account is wrong, ask an admin to change it.");
  });

  it("leaves the linked login out when the person has none", async () => {
    const user = userEvent.setup();
    answer = () => problem(422, "github_account_mismatch");
    mount({ linkedLogin: null });
    await paste(user);
    await verify(user);
    const alert = await screen.findByRole("alert");
    expect(alert).not.toHaveTextContent("Linked to you");
    expect(alert).toHaveTextContent("Create the token while signed in to the GitHub account that is linked to you.");
  });

  it("shows token_rejected as an error with the problem's title and the request id, and not what the server wrote in detail", async () => {
    await refused(problem(422, "token_rejected"));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't verify the token");
    expect(alert).toHaveTextContent("Title of token_rejected");
    expect(alert).toHaveTextContent("GitHub did not accept the token. It may be wrong, expired or revoked.");
    expect(alert).toHaveTextContent("req-tok-1");
    expect(alert).not.toHaveTextContent("A detail the screen must not show");
  });

  it.each([
    [503, "unavailable", /did not answer/],
    [400, "bad_request", /could not read that as a token/],
    [403, "forbidden", /needs the Editor role/],
  ])("shows %i as an error with the request id", async (status, type, words) => {
    await refused(problem(status, type, {}, status === 503 ? { "retry-after": "1" } : {}));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't verify the token");
    expect(alert).toHaveTextContent(words);
    expect(alert).toHaveTextContent("req-tok-1");
  });

  it("says how long to wait after 429", async () => {
    await refused(problem(429, "too_many_requests", {}, { "retry-after": "30" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Wait 30 seconds");
  });

  it("shows idempotency_conflict as a try again: nothing was saved, paste and verify again", async () => {
    await refused(problem(422, "idempotency_conflict"));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("That request clashed with an earlier one");
    expect(alert).toHaveTextContent("Nothing was saved. Paste the token and verify again.");
    expect(alert).toHaveTextContent("req-tok-1");
  });

  it("says when the server could not be reached, and does not send the token twice", async () => {
    const user = userEvent.setup();
    answer = () => {
      throw new TypeError("Failed to fetch ghp_NETWORKMESSAGE");
    };
    mount();
    await paste(user);
    await verify(user);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The server could not be reached");
    expect(alert).not.toHaveTextContent("NETWORKMESSAGE");
    // The fetch layer never resends a token on its own, so the hub saw it at most once.
    expect(puts()).toHaveLength(1);
  });

  it.each([
    ["classic_token", 422, {}],
    ["token_missing_access", 422, { base: true, tenant: false }],
    ["github_account_mismatch", 422, {}],
    ["token_rejected", 422, {}],
    ["unavailable", 503, {}],
    ["idempotency_conflict", 422, {}],
  ])("after %s the token is gone: empty field, no cache holds it, no console line, no second send", async (type, status, extra) => {
    const user = await refused(problem(status, type, extra));
    await screen.findByRole("alert");
    expect(field()).toHaveValue("");
    expect(everythingTheClientHolds(client)).not.toContain(SECRET);
    expect(client.getMutationCache().getAll()).toHaveLength(0);
    expect(JSON.stringify(consoleCalls)).not.toContain(SECRET);
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(puts()).toHaveLength(1);
    // The person is back in the field, ready to paste another.
    await waitFor(() => expect(field()).toHaveFocus());
    // And a second attempt works as a first (a new Idempotency-Key).
    answer = () => ok(CREDENTIAL);
    await paste(user, `${SECRET}2`);
    await verify(user);
    await waitFor(() => expect(onStored).toHaveBeenCalled());
    expect(puts()).toHaveLength(2);
    expect(puts()[0]?.headers.get("idempotency-key")).not.toBe(puts()[1]?.headers.get("idempotency-key"));
  });

  it("clears the earlier answer as soon as the person starts again", async () => {
    const user = await refused(problem(422, "classic_token"));
    await screen.findByRole("alert");
    await user.click(field());
    await user.paste("g");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(field()).not.toHaveAttribute("aria-invalid", "true");
  });
});

describe("when saving is not allowed", () => {
  it("disables Verify and save with the reason while live updates are paused, and sends nothing, not even on Enter", async () => {
    const user = userEvent.setup();
    mount({ online: false });
    await paste(user);
    const button = screen.getByRole("button", { name: "Verify and save" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("Unavailable offline");
    await user.click(button);
    await user.keyboard("{Enter}");
    expect(calls).toHaveLength(0);
    expect(field()).toHaveValue(SECRET);
  });

  it("names the role that is missing", async () => {
    const user = userEvent.setup();
    mount({ role: "viewer" });
    await paste(user);
    expect(screen.getByRole("button", { name: "Verify and save" })).toHaveAccessibleDescription("Needs the Editor role. Ask an Admin to grant it.");
    await user.keyboard("{Enter}");
    expect(calls).toHaveLength(0);
  });
});

describe("leaving the screen", () => {
  it("drops the token with the component, and an answer that comes after is not kept anywhere", async () => {
    const user = userEvent.setup();
    let release: (response: Response) => void = () => undefined;
    answer = () => new Promise<Response>((resolve) => (release = resolve));
    const view = mount();
    await paste(user);
    await verify(user);
    await screen.findByRole("button", { name: "Verifying…" });
    view.unmount();
    expect(document.body.innerHTML).not.toContain(SECRET);
    await act(async () => {
      release(ok(CREDENTIAL));
      await Promise.resolve();
    });
    await waitFor(() => expect(client.getMutationCache().getAll()).toHaveLength(0));
    expect(everythingTheClientHolds(client)).not.toContain(SECRET);
    // The page it belonged to is gone: nothing is told about the result.
    expect(onStored).not.toHaveBeenCalled();
  });

  it("empties the field when the page is put away", async () => {
    const user = userEvent.setup();
    mount();
    await paste(user);
    expect(field()).toHaveValue(SECRET);
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    expect(field()).toHaveValue("");
  });
});
