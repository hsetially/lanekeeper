import { cleanup, configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { RestartOffer, Role, SwimlaneDetail } from "@/api/generated/model";
import { TOKEN } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";
import { RestartForFileButton } from "./RestartForFileButton";
import { RestartForFileDialog } from "./RestartForFileDialog";
import { json, laneDetail, problem, renderNotify, routes, service, type Api, type Call } from "./test-utils";
import { expectOneBlockInAlert } from "@/test/alert-sentence";

// "Restart services" for a file that was just written (D49, "restart all"): the hub lists the services that read the file (every
// service for a file in the root folder, one application's Deployments for a file in its folder), and the screen restarts them one
// request at a time with `restartService`. The confirmation lists exactly what the hub listed and says when the list is not the
// whole of it. A run stops at the first request the hub refuses and says which were never sent. Nothing is optimistic and nothing
// is sent again by the screen. Nothing says a service restarted: the contract has no signal for it.

configure({ asyncUtilTimeout: 5000 });
afterEach(() => {
  cleanup();
  Reflect.deleteProperty(document, "cookie");
});

const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const SERVICES = [
  { namespace: "sitb", name: "tx-infinity-api" },
  { namespace: "sitb", name: "tx-infinity-core" },
  { namespace: "sitb", name: "tx-infinity-gateway" },
];
const OFFER: RestartOffer = { services: SERVICES, truncated: false };
const LANE = laneDetail({ services: [service("tx-infinity-core")] });

type Answer = Response | ((name: string) => Response);
const applied = (id = 1) => json({ outcome: "applied", hash: null, audit_id: id });

function server(offer: RestartOffer | Response, restart: Answer = () => applied()): Api {
  return routes({
    "GET /api/v1/swimlanes/sitb/restart-offer": () => (offer instanceof Response ? offer.clone() : offer),
    "POST /api/v1/swimlanes/sitb/restart": (call) => {
      const { name } = JSON.parse(call.body ?? "{}") as { name: string };
      return typeof restart === "function" ? restart(name) : restart.clone();
    },
  });
}

type Options = { offer?: RestartOffer | Response; restart?: Answer; role?: Role; online?: boolean; lane?: SwimlaneDetail; requiresApproval?: boolean };
const show = ({ offer = OFFER, restart, role = "operator", online = true, lane = LANE, requiresApproval = false }: Options = {}) =>
  renderNotify(<RestartForFileButton lane={lane} path={PATH} />, { api: server(offer, restart), role, online, user: { requires_approval: requiresApproval } });

const open = async () => {
  await userEvent.click(await screen.findByRole("button", { name: "Restart services" }));
};
const reads = (calls: readonly Call[]) => calls.filter((c) => c.method === "GET" && c.path.startsWith("/api/v1/swimlanes/sitb/restart-offer"));
const posts = (calls: readonly Call[]) => calls.filter((c) => c.method === "POST");
const bodies = (calls: readonly Call[]) => posts(calls).map((c) => (JSON.parse(c.body ?? "{}") as { name: string }).name);
const confirmDialog = () => screen.findByRole("alertdialog", { name: "Restart 3 services in sitb?" });
const run = async (dialog: HTMLElement, n = 3) => {
  await userEvent.type(within(dialog).getByRole("textbox"), "sitb");
  await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${n} services` }));
};

describe("the button", () => {
  it("reads nothing until it is pressed, then asks the hub for the services that read this file", async () => {
    const { calls } = show();
    await screen.findByRole("button", { name: "Restart services" });
    expect(reads(calls)).toEqual([]);
    await open();
    await confirmDialog();
    expect(reads(calls)).toHaveLength(1);
    expect(new URL(reads(calls)[0]?.path ?? "", "http://hub.test").searchParams.get("path")).toBe(PATH);
  });

  it.each<Role>(["viewer", "editor"])("is on screen for a %s, disabled, with the role it needs, and reads nothing", async (role) => {
    const { calls } = show({ role });
    const button = await screen.findByRole("button", { name: "Restart services" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("Needs the Operator role. Ask an Admin to grant it.");
    await userEvent.click(button);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(reads(calls)).toEqual([]);
  });

  it("is off while live updates are paused, and while the agent is offline, and says why", async () => {
    const paused = show({ online: false });
    expect(await screen.findByRole("button", { name: "Restart services" })).toHaveAccessibleDescription("Unavailable offline");
    paused.unmount();
    show({ lane: laneDetail({ agent: "disconnected", services: LANE.services }) });
    expect(await screen.findByRole("button", { name: "Restart services" })).toHaveAccessibleDescription("Unavailable while the agent is offline");
  });
});

describe("reading the offer", () => {
  it("says it is loading and offers nothing to confirm", async () => {
    renderNotify(<RestartForFileButton lane={LANE} path={PATH} />, { api: () => new Promise<Response>(() => undefined) });
    await open();
    const dialog = await screen.findByRole("dialog", { name: "Restart services for tx-infinity-core-sit7.yml in sitb?" });
    expect(within(dialog).getByRole("status", { name: "Loading the services to restart" })).toHaveAttribute("aria-busy", "true");
  });

  it("says when the hub lists no service, without guessing why", async () => {
    show({ offer: { services: [], truncated: false } });
    await open();
    const dialog = await screen.findByRole("dialog", { name: "No services to restart" });
    expect(dialog).toHaveAccessibleDescription(`The hub lists no service to restart for a change to ${PATH} in sitb.`);
    expect(within(dialog).getByText(/builds the list from the services it holds for the swimlane\. It lists none when it holds none/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("textbox")).toBeNull();
  });

  it("says a read the server refused cannot be retried, with the request ID", async () => {
    show({ offer: problem(403, "forbidden") });
    await open();
    const dialog = await screen.findByRole("dialog", { name: "Couldn't read the services to restart" });
    expect(dialog).toHaveAccessibleDescription("Reading it needs the Operator role.");
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("offers Retry for a server error and reads again when it is pressed", async () => {
    let answer: Response = problem(500, "internal");
    const { calls } = renderNotify(<RestartForFileButton lane={LANE} path={PATH} />, { api: routes({ "GET /api/v1/swimlanes/sitb/restart-offer": () => answer.clone() }) });
    await open();
    const dialog = await screen.findByRole("dialog", { name: "Couldn't read the services to restart" });
    answer = json(OFFER);
    await userEvent.click(within(dialog).getByRole("button", { name: "Retry" }));
    await confirmDialog();
    expect(reads(calls)).toHaveLength(2);
  });
});

describe("the confirmation", () => {
  it("says what restarting does, lists the services as the hub listed them, and starts on Cancel", async () => {
    show();
    await open();
    const dialog = await confirmDialog();
    expect(dialog).toHaveAccessibleDescription(
      `Restarting replaces the pods of these 3 services, which read ${PATH}. Requests to them can fail while their pods come back. The requests are sent one at a time, and each is recorded in the audit trail.`,
    );
    const list = within(dialog).getByRole("region", { name: "3 services to restart" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["sitb/tx-infinity-api", "sitb/tx-infinity-core", "sitb/tx-infinity-gateway"]);
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    expect(within(dialog).getByText(/Other people testing in/)).toHaveTextContent("Other people testing in sitb will be interrupted.");
  });

  it("says the warning about the other people as one sentence in its box, not as pieces on lines of their own", async () => {
    show();
    await open();
    const dialog = await confirmDialog();
    expectOneBlockInAlert(within(dialog).getByText(/Other people testing in/));
  });

  it("says one service in the singular", async () => {
    show({ offer: { services: [SERVICES[0] as (typeof SERVICES)[number]], truncated: false } });
    await open();
    const dialog = await screen.findByRole("alertdialog", { name: "Restart 1 service in sitb?" });
    expect(dialog).toHaveAccessibleDescription(expect.stringContaining(`Restarting replaces the pods of this service, which read ${PATH}. Requests to it can fail`));
    expect(within(dialog).getByRole("button", { name: "Restart 1 service" })).toBeInTheDocument();
  });

  it("is confirmed by typing the swimlane's name, and never by Enter", async () => {
    const { calls } = show();
    await open();
    const dialog = await confirmDialog();
    const action = within(dialog).getByRole("button", { name: "Restart 3 services" });
    expect(action).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(within(dialog).getByRole("textbox"), "sit{Enter}");
    await userEvent.click(action);
    expect(posts(calls)).toEqual([]);
    await userEvent.type(within(dialog).getByRole("textbox"), "b{Enter}");
    expect(posts(calls)).toEqual([]);
    expect(action).not.toHaveAttribute("aria-disabled");
  });

  it("says when the list is not the whole of what the swimlane has, as the hub does", async () => {
    show({ offer: { services: SERVICES, truncated: true } });
    await open();
    const dialog = await confirmDialog();
    expect(within(dialog).getByText("The swimlane has more services than this list holds. Restarting these leaves the others as they are.")).toBeInTheDocument();
    expect(within(dialog).getByRole("region", { name: "3 services to restart (not the whole list)" })).toBeInTheDocument();
  });

  it("sends nothing when the person cancels", async () => {
    const { calls } = show();
    await open();
    await userEvent.click(within(await confirmDialog()).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(posts(calls)).toEqual([]);
  });

  it("has no axe violations", async () => {
    show();
    await open();
    await confirmDialog();
    await expectNoAxeViolations();
  });
});

describe("restarting, one request at a time", () => {
  it("sends one request per service in the hub's order, each with its own key, and says what the hub accepted and no more", async () => {
    const { calls } = show();
    await open();
    await run(await confirmDialog());
    const done = await screen.findByRole("dialog", { name: "Restart requested for 3 services" });
    expect(done).toHaveAccessibleDescription("The hub accepted 3 restart requests, one at a time. It reports no restart progress.");
    expect(bodies(calls)).toEqual(["tx-infinity-api", "tx-infinity-core", "tx-infinity-gateway"]);
    for (const post of posts(calls)) {
      expect(post.path).toBe("/api/v1/swimlanes/sitb/restart");
      expect(post.headers.get("x-csrf-token")).toBe(TOKEN);
      expect(JSON.parse(post.body ?? "{}")).toMatchObject({ namespace: "sitb" });
    }
    expect(new Set(posts(calls).map((c) => c.headers.get("idempotency-key"))).size).toBe(3);
    const list = within(done).getByRole("region", { name: "Restart requests" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["sitb/tx-infinity-api accepted", "sitb/tx-infinity-core accepted", "sitb/tx-infinity-gateway accepted"]);
    expect(within(done).queryByText(/restarted|is back/i)).toBeNull();
  });

  it("does not send the next request until the hub has answered the one before", async () => {
    const pending: Array<(response: Response) => void> = [];
    const calls: Call[] = [];
    const api: Api = (call) => {
      if (call.method === "GET") return json(OFFER);
      calls.push(call);
      return new Promise<Response>((resolve) => {
        pending.push(resolve);
      });
    };
    renderNotify(<RestartForFileButton lane={LANE} path={PATH} />, { api });
    await open();
    const dialog = await confirmDialog();
    await run(dialog);
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(within(dialog).getByText("Request 1 of 3 sent.")).toHaveAttribute("role", "status");
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(calls).toHaveLength(1);
    pending[0]?.(applied());
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(within(dialog).getByText("Request 2 of 3 sent.")).toBeInTheDocument();
    pending[1]?.(applied());
    await waitFor(() => expect(calls).toHaveLength(3));
    pending[2]?.(applied());
    await screen.findByRole("dialog", { name: "Restart requested for 3 services" });
  });

  it("locks the dialog while requests are out: no Cancel, and Escape does not close it", async () => {
    const api: Api = (call) => (call.method === "GET" ? json(OFFER) : new Promise<Response>(() => undefined));
    renderNotify(<RestartForFileButton lane={LANE} path={PATH} />, { api });
    await open();
    const dialog = await confirmDialog();
    await run(dialog);
    expect(await within(dialog).findByRole("button", { name: /Restarting/ })).toHaveAttribute("aria-disabled", "true");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled();
    await userEvent.keyboard("{Escape}");
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });

  it("does not read the offer again once the person has confirmed, so what they read is what is restarted", async () => {
    const { calls } = show();
    await open();
    await run(await confirmDialog());
    await screen.findByRole("dialog", { name: "Restart requested for 3 services" });
    expect(reads(calls)).toHaveLength(1);
  });

  it("stops at the first request the hub refuses, says which were never sent, and sends nothing again", async () => {
    const { calls } = show({ restart: (name) => (name === "tx-infinity-core" ? problem(423, "locked") : applied()) });
    await open();
    await run(await confirmDialog());
    const stopped = await screen.findByRole("dialog", { name: "Restart stopped early" });
    expect(stopped).toHaveAccessibleDescription("The hub refused the request for sitb/tx-infinity-core. 1 earlier request was accepted and 1 was not sent.");
    expect(within(stopped).getByText("sitb is locked")).toBeInTheDocument();
    expect(within(stopped).getByText("req-test-1")).toBeInTheDocument();
    const list = within(stopped).getByRole("region", { name: "Restart requests" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["sitb/tx-infinity-api accepted", "sitb/tx-infinity-core not accepted", "sitb/tx-infinity-gateway not attempted"]);
    expect(within(stopped).getByText(/Nothing was sent again\. Open this again to restart the rest\./)).toBeInTheDocument();
    expect(bodies(calls)).toEqual(["tx-infinity-api", "tx-infinity-core"]);
  });

  it("stops on a server error without saying the service was not restarted, and never shows the server's detail", async () => {
    const { calls } = show({ restart: (name) => (name === "tx-infinity-api" ? problem(500, "internal", { detail: "SELECT secret FROM vault" }) : applied()) });
    await open();
    await run(await confirmDialog());
    const stopped = await screen.findByRole("dialog", { name: "Restart stopped early" });
    expect(within(stopped).getByText(/Check the service before you try again/)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("SELECT secret");
    expect(bodies(calls)).toEqual(["tx-infinity-api"]);
  });

  it("says a request that was sent for approval is not done, names it, and goes on with the next", async () => {
    const { calls } = show({ role: "admin", restart: (name) => json({ outcome: "proposal_created", id: name === "tx-infinity-api" ? 12 : 13 }) });
    await open();
    await run(await confirmDialog());
    const sent = await screen.findByRole("dialog", { name: "Restart sent for approval" });
    expect(sent).toHaveAccessibleDescription("Nothing restarts for these services until an Admin approves their requests.");
    const list = within(sent).getByRole("region", { name: "Restart requests" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "sitb/tx-infinity-api sent for approval, request AP-12",
      "sitb/tx-infinity-core sent for approval, request AP-13",
      "sitb/tx-infinity-gateway sent for approval, request AP-13",
    ]);
    expect(within(sent).getByRole("link", { name: "View requests" })).toHaveAttribute("href", "/approvals");
    expect(bodies(calls)).toHaveLength(3);
    expect(screen.queryByText(/Restart requested for/)).toBeNull();
  });

  it("says a mix of accepted and sent-for-approval requests as that, without calling it all done", async () => {
    show({ restart: (name) => (name === "tx-infinity-core" ? json({ outcome: "proposal_created", id: 21 }) : applied()) });
    await open();
    await run(await confirmDialog());
    const mixed = await screen.findByRole("dialog", { name: "Restart requested" });
    expect(mixed).toHaveAccessibleDescription("The hub accepted 2 restart requests and sent 1 for approval. Nothing restarts for that one until an Admin approves it.");
  });

  it("offers an Operator no link to the requests", async () => {
    show({ role: "operator", restart: () => json({ outcome: "proposal_created", id: 12 }) });
    await open();
    await run(await confirmDialog());
    const sent = await screen.findByRole("dialog", { name: "Restart sent for approval" });
    expect(within(sent).queryByRole("link")).toBeNull();
  });

  it("sends no further request when the person leaves while one is out", async () => {
    const pending: Array<(response: Response) => void> = [];
    const calls: Call[] = [];
    const api: Api = (call) => {
      if (call.method === "GET") return json(OFFER);
      calls.push(call);
      return new Promise<Response>((resolve) => {
        pending.push(resolve);
      });
    };
    const view = renderNotify(<RestartForFileButton lane={LANE} path={PATH} />, { api });
    await open();
    await run(await confirmDialog());
    await waitFor(() => expect(calls).toHaveLength(1));
    view.unmount();
    pending[0]?.(applied());
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(calls).toHaveLength(1);
  });
});

describe("a person whose writes need approval", () => {
  it("is told before the confirmation that each restart goes to an Admin as its own request, and the audit trail is not promised", async () => {
    show({ requiresApproval: true });
    await open();
    const dialog = await confirmDialog();
    expect(within(dialog).getByText("Your changes need approval, so each restart goes to an Admin as its own request (3 requests). Nothing restarts until they are approved.")).toBeInTheDocument();
    expect(dialog).toHaveAccessibleDescription(expect.not.stringContaining("audit trail"));
  });
});

describe("the dialog's own gate", () => {
  function Direct({ lane }: { lane: SwimlaneDetail }) {
    const [isOpen, setOpen] = useState(true);
    return (
      <RestartForFileDialog
        lane={lane}
        path={PATH}
        open={isOpen}
        onClose={() => {
          setOpen(false);
        }}
      />
    );
  }

  it("cannot confirm while live updates are paused, and says why", async () => {
    const { calls } = renderNotify(<Direct lane={LANE} />, { api: server(OFFER), online: false });
    const dialog = await confirmDialog();
    await userEvent.type(within(dialog).getByRole("textbox"), "sitb");
    expect(within(dialog).getByText("Unavailable offline")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Restart 3 services" }));
    expect(posts(calls)).toEqual([]);
  });

  it("reads nothing for a person below Operator, and says that reading needs the role", async () => {
    const { calls } = renderNotify(<Direct lane={LANE} />, { api: server(OFFER), role: "editor" });
    const dialog = await screen.findByRole("dialog", { name: "Couldn't read the services to restart" });
    expect(dialog).toHaveAccessibleDescription("Reading it needs the Operator role.");
    expect(calls).toEqual([]);
  });
});

describe("focus", () => {
  const trigger = () => screen.findByRole("button", { name: "Restart services" });

  it("goes back to the button when the person closes the result of a run", async () => {
    show();
    const button = await trigger();
    await userEvent.click(button);
    await run(await confirmDialog());
    const done = await screen.findByRole("dialog", { name: "Restart requested for 3 services" });
    await userEvent.click(within(done).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(button).toHaveFocus());
  });

  it("goes back to the button when the person closes the message of a run that stopped early", async () => {
    show({ restart: () => problem(423, "locked") });
    const button = await trigger();
    await userEvent.click(button);
    await run(await confirmDialog());
    await userEvent.click(within(await screen.findByRole("dialog", { name: "Restart stopped early" })).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(button).toHaveFocus());
  });

  it("goes back to the button when the person cancels", async () => {
    show();
    const button = await trigger();
    await userEvent.click(button);
    await userEvent.click(within(await confirmDialog()).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(button).toHaveFocus());
  });
});
