import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { ConfigServerRestartOfferResult, Role, SwimlaneDetail } from "@/api/generated/model";
import { TOKEN } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";
import { RestartConfigServerButton } from "./RestartConfigServerButton";
import { RestartConfigServerDialog } from "./RestartConfigServerDialog";
import { json, laneDetail, problem, renderNotify, routes, service, type Api } from "./test-utils";
import { expectOneBlockInAlert } from "@/test/alert-sentence";

// "Restart config-server": the restart that finding C11 asks for (a folder created after the config-server started, or a change
// to channels.yml). It is offered by the hub only while C11 is open and the cluster report lists exactly one Deployment with the
// configured name; the dialog shows the offer exactly as the hub gave it, and says plainly when there is none. A restart takes the
// whole swimlane's config away for a while, so it is confirmed by typing the Deployment's name. Nothing is optimistic, nothing is
// restarted until the person confirms, and the screen sends one request.

configure({ asyncUtilTimeout: 5000 });
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const NAME = "csp-configuration-server";
const OFFER = {
  service: { namespace: "sitb", name: NAME },
  affected: [
    { namespace: "sitb", name: "tx-infinity-core" },
    { namespace: "sitb", name: "tx-infinity-api" },
  ],
  truncated: false,
};
const LANE = laneDetail({ services: [service("tx-infinity-core"), service(NAME, { pickup: "needs_config_server_restart" })] });

type Offer = ConfigServerRestartOfferResult | Response;
type Options = { offer?: Offer; restart?: Response | (() => Response); role?: Role; online?: boolean; lane?: SwimlaneDetail; requiresApproval?: boolean };

function api(offer: Offer, restart: Response | (() => Response) = json({ outcome: "applied", hash: null, audit_id: 77 })): Api {
  return routes({
    "GET /api/v1/swimlanes/sitb/config-server-restart": () => (offer instanceof Response ? offer.clone() : offer),
    "POST /api/v1/swimlanes/sitb/restart": () => (typeof restart === "function" ? restart() : restart.clone()),
  });
}

function show({ offer = { offer: OFFER }, restart, role = "operator", online = true, lane = LANE, requiresApproval = false }: Options = {}) {
  return renderNotify(<RestartConfigServerButton lane={lane} />, { api: api(offer, restart), role, online, user: { requires_approval: requiresApproval } });
}
const open = async () => {
  await userEvent.click(await screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" }));
};
const offerReads = (calls: readonly { method: string; path: string }[]) => calls.filter((c) => c.method === "GET" && c.path === "/api/v1/swimlanes/sitb/config-server-restart");
const restarts = (calls: readonly { method: string; path: string }[]) => calls.filter((c) => c.method === "POST");
const confirmDialog = () => screen.findByRole("alertdialog", { name: `Restart ${NAME} in sitb?` });
const typeName = async (dialog: HTMLElement) => {
  await userEvent.type(within(dialog).getByRole("textbox"), NAME);
};

describe("the button", () => {
  it("reads nothing until it is pressed, then reads the offer once", async () => {
    const { calls } = show();
    await screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" });
    expect(offerReads(calls)).toEqual([]);
    await open();
    await confirmDialog();
    expect(offerReads(calls)).toHaveLength(1);
  });

  it.each<Role>(["viewer", "editor"])("is on screen for a %s, disabled, with the role it needs, and neither opens nor reads anything", async (role) => {
    const { calls } = show({ role });
    const button = await screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("Needs the Operator role. Ask an Admin to grant it.");
    await userEvent.click(button);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(offerReads(calls)).toEqual([]);
  });

  it("is off while live updates are paused, and while the agent is offline, and says why", async () => {
    const paused = show({ online: false });
    expect(await screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" })).toHaveAccessibleDescription("Unavailable offline");
    paused.unmount();
    show({ lane: laneDetail({ agent: "disconnected", services: LANE.services }) });
    expect(await screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" })).toHaveAccessibleDescription("Unavailable while the agent is offline");
  });
});

describe("reading the offer", () => {
  it("says it is loading while the hub has not answered, and offers nothing to confirm", async () => {
    const pending: Api = () => new Promise<Response>(() => undefined);
    renderNotify(<RestartConfigServerButton lane={LANE} />, { api: pending });
    await open();
    const dialog = await screen.findByRole("dialog", { name: "Restart the config-server in sitb?" });
    expect(within(dialog).getByRole("status", { name: "Loading the config-server restart offer" })).toHaveAttribute("aria-busy", "true");
    expect(within(dialog).queryByRole("button", { name: /^Restart/ })).toBeNull();
  });

  it("says plainly that there is no offer, why the hub offers none, and that it does not say which reason applies", async () => {
    const { calls } = show({ offer: { offer: null } });
    await open();
    const dialog = await screen.findByRole("dialog", { name: "No config-server restart is offered" });
    expect(dialog).toHaveAccessibleDescription("The hub offers one for sitb only while check C11 is open for it and the cluster report lists exactly one config-server Deployment.");
    expect(within(dialog).getByText(/It offers none now\. Its answer does not say which of the two is missing/)).toBeInTheDocument();
    expect(within(dialog).getByText(/does not guess which one is meant/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("textbox")).toBeNull();
    expect(within(dialog).queryByRole("button", { name: /^Restart/ })).toBeNull();
    expect(restarts(calls)).toEqual([]);
  });

  it("says a read the server refused cannot be retried, with the request ID", async () => {
    show({ offer: problem(403, "forbidden") });
    await open();
    const dialog = await screen.findByRole("dialog", { name: "Couldn't read the config-server restart offer" });
    expect(dialog).toHaveAccessibleDescription("Reading it needs the Operator role.");
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("offers Retry for a server error, and reads the offer again when it is pressed", async () => {
    let answer: Response = problem(500, "internal");
    const server: Api = routes({ "GET /api/v1/swimlanes/sitb/config-server-restart": () => answer.clone() });
    const { calls } = renderNotify(<RestartConfigServerButton lane={LANE} />, { api: server });
    await open();
    const dialog = await screen.findByRole("dialog", { name: "Couldn't read the config-server restart offer" });
    answer = json({ offer: OFFER });
    await userEvent.click(within(dialog).getByRole("button", { name: "Retry" }));
    await confirmDialog();
    expect(offerReads(calls)).toHaveLength(2);
  });
});

describe("the confirmation", () => {
  it("says what the restart takes away, names the Deployment, and lists the services it affects", async () => {
    show();
    await open();
    const dialog = await confirmDialog();
    expect(dialog).toHaveAccessibleDescription(
      "Restarting the config-server replaces its pods. While it restarts, the other services of sitb cannot be served their config, because they all ask it for their files. It is recorded in the audit trail.",
    );
    const list = within(dialog).getByRole("region", { name: "2 other services affected" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["sitb/tx-infinity-core", "sitb/tx-infinity-api"]);
    expect(within(dialog).getByText(/Other people testing in/)).toHaveTextContent("Other people testing in sitb will be interrupted.");
  });

  it("says the warning about the other people as one sentence in its box, not as pieces on lines of their own", async () => {
    show();
    await open();
    const dialog = await confirmDialog();
    expectOneBlockInAlert(within(dialog).getByText(/Other people testing in/));
  });

  it("starts on Cancel, and cannot be confirmed until the Deployment's name is typed exactly", async () => {
    const { calls } = show();
    await open();
    const dialog = await confirmDialog();
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    const action = within(dialog).getByRole("button", { name: `Restart ${NAME}` });
    expect(action).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(within(dialog).getByRole("textbox"), NAME.slice(0, -1));
    await userEvent.click(action);
    expect(restarts(calls)).toEqual([]);
    await userEvent.type(within(dialog).getByRole("textbox"), NAME.slice(-1));
    expect(action).not.toHaveAttribute("aria-disabled");
  });

  it("is never confirmed by Enter", async () => {
    const { calls } = show();
    await open();
    const dialog = await confirmDialog();
    await userEvent.type(within(dialog).getByRole("textbox"), `${NAME}{Enter}`);
    expect(restarts(calls)).toEqual([]);
    within(dialog).getByRole("button", { name: `Restart ${NAME}` }).focus();
    await userEvent.keyboard("{Enter}");
    expect(restarts(calls)).toEqual([]);
  });

  it("says when the list of affected services is not the whole of them, as the hub does", async () => {
    show({ offer: { offer: { ...OFFER, truncated: true } } });
    await open();
    const dialog = await confirmDialog();
    expect(within(dialog).getByText("The swimlane has more services than this list shows, so the list is not the whole of the services affected.")).toBeInTheDocument();
    expect(within(dialog).getByRole("region", { name: "2 other services affected (not the whole list)" })).toBeInTheDocument();
  });

  it("says so when the hub lists no other service", async () => {
    show({ offer: { offer: { ...OFFER, affected: [] } } });
    await open();
    const dialog = await confirmDialog();
    expect(within(dialog).getByText("The hub lists no other service in sitb.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("region")).toBeNull();
  });

  it("shows an invisible character in a service name as a visible marker", async () => {
    show({ offer: { offer: { ...OFFER, affected: [{ namespace: "sitb", name: "pay‮roll" }] } } });
    await open();
    const dialog = await confirmDialog();
    expect(within(dialog).getByRole("img", { name: /U\+202E/ })).toBeInTheDocument();
  });

  it("sends nothing when the person cancels", async () => {
    const { calls } = show();
    await open();
    await userEvent.click(within(await confirmDialog()).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(restarts(calls)).toEqual([]);
  });

  it("reads the offer again when it is opened again, and does not show the old one", async () => {
    const { calls } = show();
    await open();
    await userEvent.click(within(await confirmDialog()).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await open();
    await confirmDialog();
    expect(offerReads(calls)).toHaveLength(2);
  });

  it("has no axe violations", async () => {
    show();
    await open();
    await confirmDialog();
    await expectNoAxeViolations();
  });
});

describe("restarting", () => {
  it("restarts the Deployment the offer names, with the CSRF token and an Idempotency-Key, and says the request was accepted and no more", async () => {
    const { calls } = show();
    await open();
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    const done = await screen.findByRole("dialog", { name: "Restart requested" });
    expect(done).toHaveAccessibleDescription(`The hub accepted the restart of ${NAME} in sitb.`);
    expect(within(done).getByText("77")).toBeInTheDocument();
    expect(within(done).getByText(/The hub reports no restart progress/)).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).toBeNull();
    const post = restarts(calls)[0] as (typeof calls)[number];
    expect(post.path).toBe("/api/v1/swimlanes/sitb/restart");
    expect(JSON.parse(post.body ?? "{}")).toEqual({ namespace: "sitb", name: NAME });
    expect(post.headers.get("x-csrf-token")).toBe(TOKEN);
    expect(post.headers.get("idempotency-key")).toMatch(/\S/);
    expect(restarts(calls)).toHaveLength(1);
  });

  it("does not read the offer again while the restart is out, so what the person confirmed is what is restarted", async () => {
    let answer: (response: Response) => void = () => undefined;
    const server: Api = (call) =>
      call.method === "POST"
        ? new Promise<Response>((resolve) => {
            answer = resolve;
          })
        : json({ offer: OFFER });
    const { calls } = renderNotify(<RestartConfigServerButton lane={LANE} />, { api: server });
    await open();
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    expect(await within(dialog).findByRole("button", { name: /Restarting/ })).toHaveAttribute("aria-disabled", "true");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled();
    answer(json({ outcome: "applied", hash: null, audit_id: 5 }));
    await screen.findByRole("dialog", { name: "Restart requested" });
    expect(offerReads(calls)).toHaveLength(1);
  });

  it("says a restart that was sent for approval is not done, names the request, and offers it to an Admin only", async () => {
    show({ role: "admin", restart: json({ outcome: "proposal_created", id: 12 }) });
    await open();
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    const sent = await screen.findByRole("dialog", { name: "Config-server restart sent for approval" });
    expect(sent).toHaveAccessibleDescription("Nothing restarts until an Admin approves request AP-12.");
    expect(within(sent).getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/12");
    expect(screen.queryByText("Restart requested")).toBeNull();
  });

  it("offers an Operator no link to the request", async () => {
    show({ role: "operator", restart: json({ outcome: "proposal_created", id: 12 }) });
    await open();
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    expect(within(await screen.findByRole("dialog", { name: "Config-server restart sent for approval" })).queryByRole("link")).toBeNull();
  });

  it.each([
    [423, "sitb is locked", /Another operation is using it/],
    [422, "unprocessable", /The server did not accept this restart\. Nothing was restarted\./],
    [403, "You may not restart services here", /needs the Operator role/],
    [429, "Too many requests", /Wait a moment/],
    [500, "internal", /Check the service before you try again/],
  ])("keeps the dialog open on a %i, with the request ID, the name still typed, and one request sent", async (status, title, words) => {
    const { calls } = show({ restart: problem(status, status === 423 ? "locked" : status === 422 ? "unprocessable" : status === 403 ? "forbidden" : status === 429 ? "too_many_requests" : "internal") });
    await open();
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    expect(await within(dialog).findByText(title)).toBeInTheDocument();
    expect(within(dialog).getByText(words)).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(within(dialog).getByRole("textbox")).toHaveValue(NAME);
    expect(screen.queryByText("Restart requested")).toBeNull();
    expect(restarts(calls)).toHaveLength(1);
  });

  it("derives the cause of a 423 from the baseline when the swimlane is not adopted", async () => {
    show({ lane: laneDetail({ baseline: { state: "unconfirmed", by: null, at: null }, services: LANE.services }), restart: problem(423, "locked") });
    await open();
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    expect(await within(dialog).findByText("sitb is locked until it is adopted")).toBeInTheDocument();
  });

  it("never shows what the server wrote in its detail", async () => {
    show({ restart: problem(422, "unprocessable", { detail: "SELECT secret FROM vault" }) });
    await open();
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    await within(dialog).findByText(/The server did not accept this restart/);
    expect(document.body.textContent).not.toContain("SELECT secret");
  });

  it("lets the person try again from the same dialog after a refusal", async () => {
    let answer: Response = problem(423, "locked");
    const { calls } = show({ restart: () => answer.clone() });
    await open();
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    await within(dialog).findByText("sitb is locked");
    answer = json({ outcome: "applied", hash: null, audit_id: 9 });
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    expect(await screen.findByRole("dialog", { name: "Restart requested" })).toBeInTheDocument();
    expect(restarts(calls)).toHaveLength(2);
  });
});

describe("the dialog's own gate", () => {
  function Direct({ lane }: { lane: SwimlaneDetail }) {
    const [isOpen, setOpen] = useState(true);
    return (
      <RestartConfigServerDialog
        lane={lane}
        open={isOpen}
        onClose={() => {
          setOpen(false);
        }}
      />
    );
  }

  it("cannot confirm while live updates are paused, says why, and sends nothing (a read is still open)", async () => {
    const { calls } = renderNotify(<Direct lane={LANE} />, { api: api({ offer: OFFER }), online: false });
    const dialog = await confirmDialog();
    await typeName(dialog);
    expect(within(dialog).getByText("Unavailable offline")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    expect(restarts(calls)).toEqual([]);
  });

  it("cannot confirm while the agent is offline, and says why", async () => {
    const { calls } = renderNotify(<Direct lane={laneDetail({ agent: "disconnected", services: LANE.services })} />, { api: api({ offer: OFFER }) });
    const dialog = await confirmDialog();
    await typeName(dialog);
    expect(within(dialog).getByText("Unavailable while the agent is offline")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    expect(restarts(calls)).toEqual([]);
  });

  it("reads nothing for a person below Operator, and says that reading needs the role", async () => {
    const { calls } = renderNotify(<Direct lane={LANE} />, { api: api({ offer: OFFER }), role: "editor" });
    const dialog = await screen.findByRole("dialog", { name: "Couldn't read the config-server restart offer" });
    expect(dialog).toHaveAccessibleDescription("Reading it needs the Operator role.");
    expect(calls).toEqual([]);
  });
});

describe("a person whose writes need approval", () => {
  it("is told before the confirmation that this goes to an Admin, and the audit trail is not promised", async () => {
    show({ requiresApproval: true });
    await open();
    const dialog = await confirmDialog();
    expect(within(dialog).getByText(/Your changes need approval/)).toBeInTheDocument();
    expect(dialog).toHaveAccessibleDescription(expect.not.stringContaining("audit trail"));
  });
});


describe("focus", () => {
  const trigger = () => screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" });

  it("goes back to the button when the restart was requested and the person closes the result", async () => {
    show();
    const button = await trigger();
    await userEvent.click(button);
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    const done = await screen.findByRole("dialog", { name: "Restart requested" });
    await waitFor(() => expect(done).toHaveFocus());
    await userEvent.click(within(done).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(button).toHaveFocus());
  });

  it("goes back to the button when the person closes the message that there is no offer", async () => {
    show({ offer: { offer: null } });
    const button = await trigger();
    await userEvent.click(button);
    await userEvent.click(within(await screen.findByRole("dialog", { name: "No config-server restart is offered" })).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(button).toHaveFocus());
  });

  it("goes back to the button when the person closes the message that the restart was sent for approval", async () => {
    show({ restart: json({ outcome: "proposal_created", id: 12 }) });
    const button = await trigger();
    await userEvent.click(button);
    const dialog = await confirmDialog();
    await typeName(dialog);
    await userEvent.click(within(dialog).getByRole("button", { name: `Restart ${NAME}` }));
    await userEvent.click(within(await screen.findByRole("dialog", { name: "Config-server restart sent for approval" })).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(button).toHaveFocus());
  });

  it("goes back to the button when the person cancels the confirmation", async () => {
    show();
    const button = await trigger();
    await userEvent.click(button);
    await userEvent.click(within(await confirmDialog()).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(button).toHaveFocus());
  });
});
