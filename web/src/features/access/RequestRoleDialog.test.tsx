import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { apiCaches } from "@/api/client";
import { Role } from "@/api/generated/model";
import { ROLE_ORDER } from "@/auth/roles";
import { WriteGateProvider } from "@/components/app/write-gate";
import { createQueryClient } from "@/lib/query/client";
import { RequestRoleButton } from "./RequestRoleDialog";

// "Request a role change" (the permission state's one next step in the States index, and the contract's createAccessRequest): a
// person with a role asks an Admin for a higher one, with a reason. A write: nothing is shown as done until the hub answers, and
// the answer is shown in the dialog that asked (202 "request sent", 409 an open request, 429 too soon).

type Call = { method: string; path: string; body: string | undefined; headers: Headers };
let calls: Call[];
let answer: (call: Call) => Response | Promise<Response>;
let client: QueryClient;

const ok = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const problem = (status: number, type: string, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify({ type: `urn:lanekeeper:problem:${type}`, title: `Title of ${type}`, status, request_id: "req-role-1", detail: "A detail the screen must not show" }), {
    status,
    headers: { "content-type": "application/problem+json", ...headers },
  });
const ACCEPTED = { id: 7, user: "u", email: "ada@example.test", requested_role: "editor", reason: "to fix a typo", status: "pending", created_at: 1, decided_by: null, decided_at: null };

beforeEach(() => {
  apiCaches.clear();
  Object.defineProperty(document, "cookie", { configurable: true, get: () => "__Host-lk_csrf=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde", set: () => undefined });
  calls = [];
  answer = (call) => (call.method === "POST" ? ok(ACCEPTED, 202) : problem(404, "not_found"));
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      const call: Call = { method: (init.method ?? "GET").toUpperCase(), path: input, body: typeof init.body === "string" ? init.body : undefined, headers: new Headers(init.headers) };
      calls.push(call);
      return Promise.resolve(answer(call));
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

function mount(props: { have: Role | null; need?: Role; online?: boolean }) {
  client = createQueryClient();
  const { online = true, ...button } = props;
  return render(
    <QueryClientProvider client={client}>
      <WriteGateProvider value={{ roles: ROLE_ORDER, role: props.have, online }}>
        <RequestRoleButton {...button} />
      </WriteGateProvider>
    </QueryClientProvider>,
  );
}
const open = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(screen.getByRole("button", { name: "Request a role change" }));
  return screen.findByRole("alertdialog", { name: "Request a role change" });
};
const posts = () => calls.filter((c) => c.method === "POST");

describe("the button", () => {
  it("is offered to a person who has a role below Admin", () => {
    for (const have of [Role.viewer, Role.editor, Role.operator]) {
      const view = mount({ have });
      expect(screen.getByRole("button", { name: "Request a role change" })).toBeInTheDocument();
      view.unmount();
    }
  });

  it("is not offered to an Admin, who has no higher role, nor to a person with no role, who cannot ask", () => {
    mount({ have: Role.admin });
    expect(screen.queryByRole("button")).toBeNull();
    const view = render(<div />);
    view.unmount();
    mount({ have: null });
    expect(screen.queryByRole("button", { name: "Request a role change" })).toBeNull();
  });
});

describe("the dialog", () => {
  it("says what happens first, then asks for the role and the reason, and puts the focus on Cancel", async () => {
    const user = userEvent.setup();
    mount({ have: Role.viewer });
    const dialog = await open(user);
    expect(dialog).toHaveAccessibleDescription("An Admin sees your request and the reason, and decides. Your role does not change until they do.");
    expect(within(dialog).getByRole("radiogroup", { name: "Role you need" })).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Why you need it")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    expect(within(dialog).getByRole("button", { name: "Send request" })).toBeInTheDocument();
  });

  it("offers the roles above the one the person has, the lowest first, and not their own", async () => {
    const user = userEvent.setup();
    mount({ have: Role.editor });
    const dialog = await open(user);
    const radios = within(dialog).getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("value"))).toEqual(["operator", "admin"]);
    expect(radios[0]).toBeChecked();
  });

  it("starts on the role the screen needed, when it is one the person can ask for", async () => {
    const user = userEvent.setup();
    mount({ have: Role.viewer, need: Role.admin });
    const dialog = await open(user);
    expect(within(dialog).getByRole("radio", { name: /^Admin/ })).toBeChecked();
  });

  it("asks for a reason before it asks the hub", async () => {
    const user = userEvent.setup();
    mount({ have: Role.viewer });
    const dialog = await open(user);
    await user.click(within(dialog).getByRole("button", { name: "Send request" }));
    expect(await within(dialog).findByText("Say why you need it.")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Why you need it")).toHaveAttribute("aria-invalid", "true");
    expect(calls).toHaveLength(0);
  });

  it("limits the reason to what the hub accepts", async () => {
    const user = userEvent.setup();
    mount({ have: Role.viewer });
    const dialog = await open(user);
    expect(within(dialog).getByLabelText("Why you need it")).toHaveAttribute("maxlength", "256");
  });

  it("closes on Cancel without a request, and starts empty the next time", async () => {
    const user = userEvent.setup();
    mount({ have: Role.viewer });
    let dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Why you need it"), "half a thought");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(calls).toHaveLength(0);
    dialog = await open(user);
    expect(within(dialog).getByLabelText("Why you need it")).toHaveValue("");
  });
});

describe("while live updates are paused", () => {
  it("disables Send request with the reason, and sends nothing", async () => {
    const user = userEvent.setup();
    mount({ have: Role.viewer, online: false });
    const dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Why you need it"), "to fix a typo");
    const send = within(dialog).getByRole("button", { name: "Send request" });
    expect(send).toHaveAttribute("aria-disabled", "true");
    expect(send).toHaveAccessibleDescription("Unavailable offline");
    await user.click(send);
    expect(calls).toHaveLength(0);
  });
});

describe("sending the request", () => {
  it("sends the role and the reason with the CSRF token and an Idempotency-Key, and shows the hub's answer in the dialog", async () => {
    const user = userEvent.setup();
    mount({ have: Role.viewer });
    const dialog = await open(user);
    await user.click(within(dialog).getByRole("radio", { name: /^Editor/ }));
    await user.type(within(dialog).getByLabelText("Why you need it"), "to fix a typo");
    await user.click(within(dialog).getByRole("button", { name: "Send request" }));

    expect(await screen.findByText("Request sent")).toBeInTheDocument();
    expect(screen.getByRole("alertdialog")).toHaveTextContent("An Admin will decide on your request for the Editor role. You keep your current role until they do.");
    expect(posts()).toHaveLength(1);
    const post = posts()[0];
    expect(post?.path).toBe("/api/v1/me/access-request");
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ requested_role: "editor", reason: "to fix a typo" });
    expect(post?.headers.get("x-csrf-token")).toBeTruthy();
    expect(post?.headers.get("idempotency-key")).toBeTruthy();
    // The form is gone: the same request cannot be sent twice from here.
    expect(screen.queryByRole("button", { name: "Send request" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("shows nothing as done while the hub has not answered, and locks the dialog", async () => {
    const user = userEvent.setup();
    let release: (r: Response) => void = () => undefined;
    answer = () => new Promise<Response>((resolve) => (release = resolve));
    mount({ have: Role.viewer });
    const dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Why you need it"), "to fix a typo");
    await user.click(within(dialog).getByRole("button", { name: "Send request" }));
    const busy = await within(dialog).findByRole("button", { name: "Sending…" });
    expect(busy).toHaveAttribute("aria-busy", "true");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(screen.queryByText("Request sent")).toBeNull();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    release(ok(ACCEPTED, 202));
    expect(await screen.findByText("Request sent")).toBeInTheDocument();
  });

  it("asks the hub for the signed-in user again, so the rest of the app sees the request", async () => {
    const user = userEvent.setup();
    mount({ have: Role.viewer });
    client.setQueryData(["me"], { role: "viewer" });
    const dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Why you need it"), "to fix a typo");
    await user.click(within(dialog).getByRole("button", { name: "Send request" }));
    await screen.findByText("Request sent");
    expect(client.getQueryState(["me"])?.isInvalidated).toBe(true);
  });

  it.each([
    [409, "conflict", /already have an open request/],
    [429, "too-many-requests", /asked recently/],
    [400, "bad-request", /did not accept that request/],
    [500, "internal", null],
  ])("keeps the dialog and what was typed on %i, with the problem's title and the request id", async (status, type, words) => {
    const user = userEvent.setup();
    answer = () => problem(status, type);
    mount({ have: Role.viewer });
    const dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Why you need it"), "to fix a typo");
    await user.click(within(dialog).getByRole("button", { name: "Send request" }));
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(`Title of ${type}`);
    expect(alert).toHaveTextContent("req-role-1");
    expect(alert).not.toHaveTextContent("A detail the screen must not show");
    if (words) expect(alert).toHaveTextContent(words);
    expect(within(dialog).getByLabelText("Why you need it")).toHaveValue("to fix a typo");
    expect(within(dialog).getByRole("button", { name: "Send request" })).toBeInTheDocument();
    expect(posts()).toHaveLength(1);
  });

  it("says how long to wait when the hub says so", async () => {
    const user = userEvent.setup();
    answer = () => problem(429, "too-many-requests", { "retry-after": "120" });
    mount({ have: Role.viewer });
    const dialog = await open(user);
    await user.type(within(dialog).getByLabelText("Why you need it"), "to fix a typo");
    await user.click(within(dialog).getByRole("button", { name: "Send request" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Try again in 120 seconds.");
  });
});
