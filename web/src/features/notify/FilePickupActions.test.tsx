import { configure, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { PickupState, Role } from "@/api/generated/model";
import { FilePickupActions } from "./FilePickupActions";
import { json, laneDetail, renderNotify, routes, service } from "./test-utils";

// What the file page offers after a write, by the file's pickup state (D85): a file that waits for a notify or a restart gets "Notify
// services" and "Restart services"; one that waits for the config-server to restart gets "Restart config-server"; a file that is live,
// live within its cache time or not stated gets nothing, because there is nothing for a person to do.

configure({ asyncUtilTimeout: 5000 });
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const LANE = laneDetail({ services: [service("tx-infinity-core", { pending_files: [PATH] })] });
const api = routes({
  "POST /api/v1/swimlanes/sitb/notify": () => json({ accepted: 1, audit_id: 8 }),
  "GET /api/v1/swimlanes/sitb/restart-offer": () => json({ services: [{ namespace: "sitb", name: "tx-infinity-core" }], truncated: false }),
  "GET /api/v1/swimlanes/sitb/config-server-restart": () => json({ offer: null }),
});

const show = (pickup: PickupState | null | undefined, role: Role = "operator") => renderNotify(<FilePickupActions lane={LANE} path={PATH} pickup={pickup} />, { api, role });
const names = () => screen.queryAllByRole("button").map((b) => b.textContent);

describe("by pickup state", () => {
  it("offers a notify and a restart for a file that waits for either", async () => {
    show("needs_notify_or_restart");
    await screen.findByRole("button", { name: "Notify services" });
    expect(names()).toEqual(["Notify services", "Restart services"]);
  });

  it("offers the config-server restart, and nothing else, for a file that waits for it", async () => {
    show("needs_config_server_restart");
    await screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" });
    expect(names()).toEqual(["Restart config-server"]);
  });

  it.each<PickupState | null | undefined>(["live", "live_within_ttl", null, undefined])("offers nothing when the pickup state is %s", async (pickup) => {
    const { container } = show(pickup);
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(container.querySelectorAll("button")).toHaveLength(0);
  });
});

describe("the buttons", () => {
  it("announce the file that was written, and no other", async () => {
    show("needs_notify_or_restart");
    await userEvent.click(await screen.findByRole("button", { name: "Notify services" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByRole("region", { name: "1 file to announce" })).toHaveTextContent(PATH);
  });

  it("ask for the services that read the file that was written", async () => {
    const { calls } = show("needs_notify_or_restart");
    await userEvent.click(await screen.findByRole("button", { name: "Restart services" }));
    await screen.findByRole("alertdialog");
    expect(new URL(calls.find((c) => c.path.startsWith("/api/v1/swimlanes/sitb/restart-offer"))?.path ?? "", "http://hub.test").searchParams.get("path")).toBe(PATH);
  });

  it.each<Role>(["viewer", "editor"])("are disabled for a %s, with the role they need", async (role) => {
    show("needs_notify_or_restart", role);
    await screen.findByRole("button", { name: "Notify services" });
    for (const button of screen.getAllByRole("button")) {
      expect(button).toHaveAttribute("aria-disabled", "true");
      expect(button).toHaveAccessibleDescription("Needs the Operator role. Ask an Admin to grant it.");
    }
  });
});
