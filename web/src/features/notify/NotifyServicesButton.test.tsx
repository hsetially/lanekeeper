import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { Role, SwimlaneDetail } from "@/api/generated/model";
import { NotifyServicesButton } from "./NotifyServicesButton";
import { json, laneDetail, renderNotify, routes, service } from "./test-utils";

// The two places a person starts a notification: the Services tab (the files that services are waiting on) and the file page
// after a write (the one file). The button is on screen for everyone and disabled, with the reason, for whoever cannot use it.

configure({ asyncUtilTimeout: 5000 });
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const WAITING = laneDetail({
  services: [service("tx-infinity-core", { pending_files: ["application.properties", "tx-infinity-api/a.yml"] }), service("tx-infinity-api", { pending_files: ["application.properties"] })],
});
const api = routes({ "POST /api/v1/swimlanes/sitb/notify": () => json({ accepted: 2, audit_id: 41 }) });

type Options = { lane?: SwimlaneDetail; path?: string; role?: Role; online?: boolean };
const show = ({ lane = WAITING, path, role = "operator", online = true }: Options = {}) =>
  renderNotify(path === undefined ? <NotifyServicesButton lane={lane} /> : <NotifyServicesButton lane={lane} path={path} />, { api, role, online });
const button = () => screen.findByRole("button", { name: "Notify services" });

describe("on the Services tab", () => {
  it("opens the dialog with the files that services are waiting on", async () => {
    show();
    await userEvent.click(await button());
    const dialog = await screen.findByRole("alertdialog", { name: "Notify services in sitb?" });
    const list = within(dialog).getByRole("region", { name: "2 files to announce" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["application.properties", "tx-infinity-api/a.yml"]);
  });

  it("is on screen but off when no service is waiting on a file, and says why", async () => {
    show({ lane: laneDetail({ services: [service("quiet", { pickup: "live" })] }) });
    const b = await button();
    expect(b).toHaveAttribute("aria-disabled", "true");
    expect(b).toHaveAccessibleDescription("No service is waiting on a file.");
    await userEvent.click(b);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});

describe("on the file page", () => {
  it("opens the dialog for the one file, even when no service is listed as waiting on it", async () => {
    show({ lane: laneDetail({ services: [service("quiet", { pickup: "live" })] }), path: "tx-infinity-api/b.yml" });
    await userEvent.click(await button());
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveAccessibleDescription(expect.stringContaining("this file changed in sitb"));
    expect(within(dialog).getByRole("region", { name: "1 file to announce" })).toHaveTextContent("tx-infinity-api/b.yml");
  });
});

describe("who may, and when", () => {
  it.each<Role>(["viewer", "editor"])("keeps the button on screen for a %s, disabled, with the role it needs, and opens nothing", async (role) => {
    show({ role });
    const b = await button();
    expect(b).toHaveAttribute("aria-disabled", "true");
    expect(b).toHaveAccessibleDescription("Needs the Operator role. Ask an Admin to grant it.");
    await userEvent.click(b);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("is off while live updates are paused", async () => {
    show({ online: false });
    expect(await button()).toHaveAccessibleDescription("Unavailable offline");
  });

  it("is off while the agent is offline", async () => {
    show({ lane: laneDetail({ agent: "disconnected", services: WAITING.services }) });
    expect(await button()).toHaveAccessibleDescription("Unavailable while the agent is offline");
  });
});

describe("what the person read is what is sent", () => {
  function Moving() {
    const [lane, setLane] = useState(WAITING);
    return (
      <>
        <NotifyServicesButton lane={lane} />
        <button
          type="button"
          onClick={() => {
            setLane(laneDetail({ services: [service("newcomer", { pending_files: ["z/later.yml"] })] }));
          }}
        >
          A scan finds another file
        </button>
      </>
    );
  }

  it("keeps the plan it showed when the swimlane changes under an open dialog", async () => {
    const { calls } = renderNotify(<Moving />, { api });
    await userEvent.click(await button());
    const dialog = await screen.findByRole("alertdialog");
    // The page behind the dialog changes (a live event); the dialog is modal, so reach the button the way a script would.
    screen.getByRole("button", { name: "A scan finds another file", hidden: true }).click();
    await userEvent.click(await within(dialog).findByRole("button", { name: "Notify services" }));
    await screen.findByRole("dialog", { name: "Notification sent" });
    expect(JSON.parse(calls.find((c) => c.method === "POST")?.body ?? "{}")).toEqual({ paths: ["application.properties", "tx-infinity-api/a.yml"] });
  });
});

describe("focus", () => {
  it("goes into the result when the hub has answered, and back to the button when the person closes it", async () => {
    show();
    const b = await button();
    await userEvent.click(b);
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Notify services" }));
    const result = await screen.findByRole("dialog", { name: "Notification sent" });
    await waitFor(() => expect(result).toHaveFocus());
    await userEvent.click(within(result).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(b).toHaveFocus());
  });

  it("goes back to the button when the person cancels", async () => {
    show();
    const b = await button();
    await userEvent.click(b);
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(b).toHaveFocus());
  });
});

describe("focus after a request sent for approval", () => {
  it("goes back to the button when the person closes the message", async () => {
    renderNotify(<NotifyServicesButton lane={WAITING} />, { api: routes({ "POST /api/v1/swimlanes/sitb/notify": () => json({ outcome: "proposal_created", id: 12 }) }) });
    const b = await button();
    await userEvent.click(b);
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Notify services" }));
    await userEvent.click(within(await screen.findByRole("dialog", { name: "Notification sent for approval" })).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(b).toHaveFocus());
  });
});
