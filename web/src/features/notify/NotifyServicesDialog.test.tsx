import { useQuery } from "@tanstack/react-query";
import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Role, SwimlaneDetail } from "@/api/generated/model";
import { swimlaneQuery } from "@/features/swimlanes/queries";
import { TOKEN } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";
import { NotifyServicesDialog } from "./NotifyServicesDialog";
import { planForFile, planForServices, type NotifyPlan } from "./paths";
import { json, laneDetail, problem, renderNotify, routes, service, type Api, type Call } from "./test-utils";

// "Notify services": tell the config-server which files changed (an Operator's write, audited). The dialog says first what will be
// announced and that it is recorded; it sends nothing until the person confirms; and it shows what the server answered and no
// more: the count the hub accepted and the audit event, the request sent for approval, or the refusal with its request ID. Nothing
// is optimistic and nothing is sent again by the screen.

configure({ asyncUtilTimeout: 5000 });
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const FILES = ["application.properties", "tx-infinity-api/tx-infinity-core-sit7.yml"] as const;
const WAITING = laneDetail({
  services: [
    service("tx-infinity-core", { pending_files: [FILES[1], FILES[0]] }),
    service("tx-infinity-api", { pending_files: [FILES[0]], notifications_enabled: false }),
    service("document-service", { pickup: "live" }),
  ],
});

const notifyAnswer = (body: unknown = { accepted: 2, audit_id: 41 }, status = 200): Api =>
  routes({ "POST /api/v1/swimlanes/sitb/notify": () => (status === 200 ? json(body) : problem(status, "x")) });

type Options = { lane?: SwimlaneDetail; plan?: NotifyPlan; api?: Api; role?: Role; online?: boolean; requiresApproval?: boolean; onClose?: () => void; probe?: boolean };

function Probe({ id }: { id: string }) {
  useQuery(swimlaneQuery(id));
  return null;
}

function Harness({ lane, plan, onClose }: { lane: SwimlaneDetail; plan: NotifyPlan; onClose: () => void }) {
  const [open, setOpen] = useState(true);
  return (
    <NotifyServicesDialog
      lane={lane}
      plan={plan}
      open={open}
      onClose={() => {
        setOpen(false);
        onClose();
      }}
    />
  );
}

function show({ lane = WAITING, plan = planForServices(lane.services), api = notifyAnswer(), role = "operator", online = true, requiresApproval = false, onClose = () => undefined, probe = false }: Options = {}) {
  return renderNotify(
    <>
      {probe ? <Probe id={lane.id} /> : null}
      <Harness lane={lane} plan={plan} onClose={onClose} />
    </>,
    { api, role, online, user: { requires_approval: requiresApproval } },
  );
}

const confirm = () => screen.findByRole("button", { name: "Notify services" });
const posts = (calls: readonly Call[]) => calls.filter((c) => c.method === "POST");

describe("asking first", () => {
  it("says what will be announced and that it is recorded, and puts focus on Cancel", async () => {
    show();
    const dialog = await screen.findByRole("alertdialog", { name: "Notify services in sitb?" });
    expect(dialog).toHaveAccessibleDescription("Tell the config-server that these 2 files changed in sitb. The agent sends the request. It is recorded in the audit trail, and nothing is restarted.");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    const list = within(dialog).getByRole("region", { name: "2 files to announce" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual([FILES[0], FILES[1]]);
  });

  it("names the services that wait on those files, with the notification setting each reported and no more", async () => {
    show();
    const dialog = await screen.findByRole("alertdialog");
    const list = within(dialog).getByRole("region", { name: "2 services waiting on these files" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["sitb/tx-infinity-api notifications reported off", "sitb/tx-infinity-core notifications reported on"]);
    expect(within(list).queryByText(/document-service/)).toBeNull();
    // It reports the setting; it does not say what the setting does.
    expect(dialog.textContent).not.toMatch(/will pick|picks it up|will receive|will get/i);
  });

  it("sends nothing until the person confirms, and nothing when they cancel", async () => {
    const onClose = vi.fn();
    const { calls } = show({ onClose });
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(posts(calls)).toEqual([]);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("says when more files are waiting than one request can carry, and sends the 100 it can", async () => {
    const many = Array.from({ length: 130 }, (_, i) => `app/f${String(i).padStart(3, "0")}.yml`);
    const lane = laneDetail({ services: [service("big", { pending_files: many })] });
    const { calls } = show({ lane, api: notifyAnswer({ accepted: 100, audit_id: 41 }) });
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/30 more files are waiting and are not in this request/)).toBeInTheDocument();
    await userEvent.click(await confirm());
    await screen.findByRole("dialog", { name: "Notification sent" });
    expect((JSON.parse(posts(calls)[0]?.body ?? "{}") as { paths: string[] }).paths).toHaveLength(100);
  });

  it("says, for a file nobody is listed as waiting on, that the notification is still sent", async () => {
    show({ lane: laneDetail({ services: [service("tx-infinity-core")] }), plan: planForFile("new.yml", []) });
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveAccessibleDescription(expect.stringContaining("this file changed in sitb"));
    expect(within(dialog).getByText(/No service is listed as waiting on this file\. The notification is still sent\./)).toBeInTheDocument();
  });

  it("has no axe violations", async () => {
    show();
    await screen.findByRole("alertdialog");
    await expectNoAxeViolations();
  });
});

describe("sending", () => {
  it("sends the paths with the CSRF token and an Idempotency-Key, and shows what the hub answered", async () => {
    const { calls } = show();
    await userEvent.click(await confirm());
    const dialog = await screen.findByRole("dialog", { name: "Notification sent" });
    expect(dialog).toHaveAccessibleDescription("The hub accepted 2 paths for sitb.");
    expect(within(dialog).getByText("41")).toBeInTheDocument();
    const post = posts(calls)[0];
    expect(post?.path).toBe("/api/v1/swimlanes/sitb/notify");
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ paths: [FILES[0], FILES[1]] });
    expect(post?.headers.get("x-csrf-token")).toBe(TOKEN);
    expect(post?.headers.get("idempotency-key")).toMatch(/\S/);
    expect(posts(calls)).toHaveLength(1);
  });

  it("does not invent a result for each service: the hub's answer has none, and the dialog says so", async () => {
    show();
    await userEvent.click(await confirm());
    const dialog = await screen.findByRole("dialog", { name: "Notification sent" });
    expect(within(dialog).getByText(/does not say which services picked the change up/)).toBeInTheDocument();
    expect(within(dialog).queryByText(/tx-infinity-core/)).toBeNull();
    expect(within(dialog).queryByRole("list")).toBeNull();
  });

  it("says one path in the singular", async () => {
    show({ lane: laneDetail(), plan: planForFile("application.properties", []), api: notifyAnswer({ accepted: 1, audit_id: 7 }) });
    await userEvent.click(await confirm());
    expect(await screen.findByRole("dialog", { name: "Notification sent" })).toHaveAccessibleDescription("The hub accepted 1 path for sitb.");
  });

  it("says a count lower than what was sent is partial, without guessing which paths were left out", async () => {
    show({ api: notifyAnswer({ accepted: 1, audit_id: 42 }) });
    await userEvent.click(await confirm());
    const dialog = await screen.findByRole("dialog", { name: "Notification partly accepted" });
    expect(dialog).toHaveAccessibleDescription("The hub accepted 1 of 2 paths for sitb. Its answer does not say which paths it left out or why.");
    expect(within(dialog).getByText(/Nothing was sent again/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("list")).toBeNull();
  });

  it("says none accepted as none accepted", async () => {
    show({ api: notifyAnswer({ accepted: 0, audit_id: 43 }) });
    await userEvent.click(await confirm());
    const dialog = await screen.findByRole("dialog", { name: "Notification partly accepted" });
    expect(dialog).toHaveAccessibleDescription(expect.stringContaining("The hub accepted 0 of 2 paths"));
  });

  it("locks the dialog while the request is out, and does not show a result before the server answers", async () => {
    let answer: (response: Response) => void = () => undefined;
    const api: Api = () =>
      new Promise<Response>((resolve) => {
        answer = resolve;
      });
    show({ api });
    await userEvent.click(await confirm());
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByRole("button", { name: /Notifying/ })).toHaveAttribute("aria-disabled", "true");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled();
    await userEvent.keyboard("{Escape}");
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(screen.queryByText("Notification sent")).toBeNull();
    answer(json({ accepted: 2, audit_id: 1 }));
    expect(await screen.findByRole("dialog", { name: "Notification sent" })).toBeInTheDocument();
  });

  it("reads the swimlane again after the hub accepted it, so the pickup states on screen are the hub's", async () => {
    const detail = () => json(WAITING);
    const api = routes({ "GET /api/v1/swimlanes/sitb": detail, "POST /api/v1/swimlanes/sitb/notify": () => ({ accepted: 2, audit_id: 41 }) });
    const { calls } = show({ api, probe: true });
    await waitFor(() => expect(calls.filter((c) => c.method === "GET" && c.path === "/api/v1/swimlanes/sitb")).toHaveLength(1));
    await userEvent.click(await confirm());
    await screen.findByRole("dialog", { name: "Notification sent" });
    await waitFor(() => expect(calls.filter((c) => c.method === "GET" && c.path === "/api/v1/swimlanes/sitb").length).toBeGreaterThan(1));
  });

  it("closes from the result and tells the page", async () => {
    const onClose = vi.fn();
    show({ onClose });
    await userEvent.click(await confirm());
    await userEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("sent for approval", () => {
  const sent = { outcome: "proposal_created", id: 12 };

  it("says nothing was sent to the config-server, names the request, and offers it to an Admin", async () => {
    show({ role: "admin", api: notifyAnswer(sent) });
    await userEvent.click(await confirm());
    const dialog = await screen.findByRole("dialog", { name: "Notification sent for approval" });
    expect(dialog).toHaveAccessibleDescription("Nothing is sent to the config-server until an Admin approves request AP-12.");
    expect(within(dialog).getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/12");
    expect(screen.queryByText("Notification sent")).toBeNull();
  });

  it("offers no link to someone who cannot open the request", async () => {
    show({ role: "operator", api: notifyAnswer(sent) });
    await userEvent.click(await confirm());
    const dialog = await screen.findByRole("dialog", { name: "Notification sent for approval" });
    expect(within(dialog).queryByRole("link")).toBeNull();
  });

  it("is said before the confirmation to a person whose writes need approval, and the audit trail is not promised", async () => {
    show({ requiresApproval: true });
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveAccessibleDescription("Ask an Admin to tell the config-server that these 2 files changed in sitb. Nothing is sent until the request is approved.");
    expect(within(dialog).getByText(/Your changes need approval/)).toBeInTheDocument();
  });
});

describe("a refusal stays in the dialog", () => {
  it.each([
    [403, /Notifying services needs the Operator role\. Nothing was sent\./],
    [422, /The server did not accept this notification\. Nothing was sent\./],
    [429, /Wait a moment, then try again\./],
    [500, /may or may not have been sent/],
  ])("shows a %i with the request ID, keeps the paths, and sends once", async (status, words) => {
    const { calls } = show({ api: notifyAnswer(undefined, status) });
    await userEvent.click(await confirm());
    const dialog = await screen.findByRole("alertdialog");
    expect(await within(dialog).findByText(words)).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(within(dialog).getByRole("region", { name: "2 files to announce" })).toBeInTheDocument();
    expect(screen.queryByText("Notification sent")).toBeNull();
    expect(posts(calls)).toHaveLength(1);
  });

  it("derives the cause of a 423 from the baseline when the swimlane is not adopted", async () => {
    const lane = laneDetail({ baseline: { state: "unconfirmed", by: null, at: null }, services: WAITING.services });
    show({ lane, api: notifyAnswer(undefined, 423) });
    await userEvent.click(await confirm());
    expect(await screen.findByText("sitb is locked until it is adopted")).toBeInTheDocument();
  });

  it("says the hub changed first for a 409 and does not retry", async () => {
    const { calls } = show({ api: notifyAnswer(undefined, 409) });
    await userEvent.click(await confirm());
    expect(await screen.findByText(/Something changed before the notification was handled\. Nothing was sent\./)).toBeInTheDocument();
    expect(posts(calls)).toHaveLength(1);
  });

  it("never shows what the server wrote in its detail", async () => {
    const api: Api = () => problem(422, "unprocessable", { detail: "SELECT secret FROM vault" });
    show({ api });
    await userEvent.click(await confirm());
    await screen.findByText(/The server did not accept this notification/);
    expect(document.body.textContent).not.toContain("SELECT secret");
  });

  it("does not say the notification was not sent when nothing came back, and repeats the same key if it is sent again by the fetch layer", async () => {
    const api: Api = () => {
      throw new TypeError("Failed to fetch");
    };
    const { calls } = show({ api });
    await userEvent.click(await confirm());
    expect(await screen.findByText("Couldn't reach the server", {}, { timeout: 15_000 })).toBeInTheDocument();
    expect(screen.getByText(/may or may not have been sent/)).toBeInTheDocument();
    // The fetch layer repeats a write that got no answer, once, with the SAME key (the hub answers a repeated key with the first
    // answer); the screen sends nothing of its own.
    const keys = new Set(posts(calls).map((c) => c.headers.get("idempotency-key")));
    expect(keys.size).toBe(1);
    expect(posts(calls).length).toBeLessThanOrEqual(2);
  }, 20_000);

  it("lets the person try again from the same dialog, and a fresh dialog starts clean", async () => {
    let status = 423;
    const api: Api = () => (status === 423 ? problem(423, "locked", { until: null }) : json({ accepted: 2, audit_id: 9 }));
    const { calls } = show({ api });
    await userEvent.click(await confirm());
    await screen.findByText("sitb is locked");
    status = 200;
    await userEvent.click(screen.getByRole("button", { name: "Notify services" }));
    expect(await screen.findByRole("dialog", { name: "Notification sent" })).toBeInTheDocument();
    expect(posts(calls)).toHaveLength(2);
  });
});

describe("who may, and when", () => {
  it("is off for a person below Operator, with the role it needs, and sends nothing", async () => {
    const { calls } = show({ role: "editor" });
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Needs the Operator role. Ask an Admin to grant it.")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Notify services" }));
    expect(posts(calls)).toEqual([]);
  });

  it("is off while live updates are paused, and says why", async () => {
    const { calls } = show({ online: false });
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Unavailable offline")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Notify services" }));
    expect(posts(calls)).toEqual([]);
  });

  it("is off while the agent is offline, and says why", async () => {
    const { calls } = show({ lane: laneDetail({ agent: "disconnected", services: WAITING.services }) });
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Unavailable while the agent is offline")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Notify services" }));
    expect(posts(calls)).toEqual([]);
  });

  it("is off when there is nothing to announce", async () => {
    const { calls } = show({ lane: laneDetail(), plan: { paths: [], left: 0, services: [] } });
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Notify services" }));
    expect(posts(calls)).toEqual([]);
  });
});
