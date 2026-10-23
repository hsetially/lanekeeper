import { configure, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { Role, SwimlaneDetail } from "@/api/generated/model";
import { finding } from "@/mocks/design/swimlanes/fixtures";
import { expectNoAxeViolations } from "@/test/axe";
import { ConfigServerRestartBanner } from "./ConfigServerRestartBanner";
import { json, laneDetail, problem, renderNotify, routes, service, type Api } from "./test-utils";

// The line on the Services tab that says a config-server restart is needed: when finding C11 is open for the swimlane, or when a
// service reports that it waits for one. It says which; it offers the restart to everyone who can see it as a disabled button for
// whoever may not press it; and it reads the offer only when the button is pressed (that is the dialog's, RestartConfigServer.test).

configure({ asyncUtilTimeout: 5000 });
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const C11 = finding(7, { kind: "config_server_restart_required", severity: "high", path: "channels.yml", message: "channels.yml changed after the config-server started." });
const waitingLane = (count: number): SwimlaneDetail =>
  laneDetail({ services: Array.from({ length: count }, (_, i) => service(`svc-${i}`, { pickup: "needs_config_server_restart" })).concat([service("fine", { pickup: "live" })]) });

function server(items: readonly unknown[] | Response): Api {
  return routes({ "GET /api/v1/findings": () => (items instanceof Response ? items.clone() : { items, next: null }) });
}
const show = (lane: SwimlaneDetail, items: readonly unknown[] | Response = [], role: Role = "operator") => renderNotify(<ConfigServerRestartBanner lane={lane} />, { api: server(items), role });

describe("when it shows", () => {
  it("shows nothing when C11 is not open and no service waits for a config-server restart, and asks for nothing else", async () => {
    const { calls, container } = show(laneDetail({ services: [service("fine", { pickup: "live" })] }), []);
    await expectNoRequestsPending(calls);
    expect(container.querySelector("[data-slot=alert]")).toBeNull();
    expect(screen.queryByRole("button", { name: "Restart config-server (type its name to confirm)" })).toBeNull();
  });

  it("shows when finding C11 is open, and says so", async () => {
    show(laneDetail(), [C11]);
    const banner = await screen.findByText("Config-server restart needed");
    const alert = banner.closest("[data-slot=alert]") as HTMLElement;
    expect(within(alert).getByText(/Check C11 is open: a new folder or a change to channels\.yml is not served until the config-server restarts\./)).toBeInTheDocument();
    expect(within(alert).getByRole("button", { name: "Restart config-server (type its name to confirm)" })).toBeInTheDocument();
  });

  it("asks for the findings of this swimlane that are C11, one is enough", async () => {
    const { calls } = show(laneDetail(), [C11]);
    await screen.findByText("Config-server restart needed");
    const asked = calls.find((c) => c.path.startsWith("/api/v1/findings")) as (typeof calls)[number];
    const url = new URL(asked.path, "http://hub.test");
    expect(url.searchParams.get("swimlane")).toBe("sitb");
    expect(url.searchParams.get("kinds")).toBe("config_server_restart_required");
    expect(url.searchParams.get("limit")).toBe("1");
  });

  it("shows when a service reports that it waits for a config-server restart, and does not claim C11 is open", async () => {
    show(waitingLane(2), []);
    const banner = await screen.findByText("Config-server restart needed");
    const alert = banner.closest("[data-slot=alert]") as HTMLElement;
    expect(within(alert).getByText("2 services report needing a config-server restart.")).toBeInTheDocument();
    expect(within(alert).queryByText(/Check C11 is open/)).toBeNull();
  });

  it("says one service in the singular", async () => {
    show(waitingLane(1), []);
    expect(await screen.findByText("1 service reports needing a config-server restart.")).toBeInTheDocument();
  });

  it("says both when both are true", async () => {
    show(waitingLane(3), [C11]);
    const banner = await screen.findByText("Config-server restart needed");
    const alert = banner.closest("[data-slot=alert]") as HTMLElement;
    expect(await within(alert).findByText(/Check C11 is open/)).toBeInTheDocument();
    expect(within(alert).getByText("3 services report needing a config-server restart.")).toBeInTheDocument();
  });

  it("does not take another finding, or another swimlane's, for C11, whatever the server sent back", async () => {
    show(laneDetail(), [finding(8, { kind: "duplicate_keys" }), finding(9, { kind: "config_server_restart_required", swimlane: "sit" })]);
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(screen.queryByText("Config-server restart needed")).toBeNull();
  });

  it("stays on what the services report when the findings could not be read, and says nothing about the failure", async () => {
    show(waitingLane(1), problem(403, "forbidden"));
    expect(await screen.findByText("Config-server restart needed")).toBeInTheDocument();
    expect(screen.queryByText(/Couldn't/)).toBeNull();
  });
});

describe("who may press it", () => {
  it.each<Role>(["viewer", "editor"])("shows a %s that a restart is needed, with the button disabled and the role it needs", async (role) => {
    show(laneDetail(), [C11], role);
    const button = await screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("Needs the Operator role. Ask an Admin to grant it.");
    await userEvent.click(button);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("reads no offer until the button is pressed", async () => {
    const { calls } = renderNotify(<ConfigServerRestartBanner lane={laneDetail()} />, { api: routes({ "GET /api/v1/findings": () => ({ items: [C11], next: null }), "GET /api/v1/swimlanes/sitb/config-server-restart": () => json({ offer: null }) }) });
    await screen.findByText("Config-server restart needed");
    expect(calls.filter((c) => c.path.endsWith("/config-server-restart"))).toEqual([]);
    await userEvent.click(screen.getByRole("button", { name: "Restart config-server (type its name to confirm)" }));
    expect(await screen.findByRole("dialog", { name: "No config-server restart is offered" })).toBeInTheDocument();
    expect(calls.filter((c) => c.path.endsWith("/config-server-restart"))).toHaveLength(1);
  });
});

it("has no axe violations", async () => {
  show(laneDetail(), [C11]);
  await screen.findByText("Config-server restart needed");
  await expectNoAxeViolations();
});

/** Let the findings read finish before saying that nothing was drawn. */
async function expectNoRequestsPending(calls: readonly { path: string }[]): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(calls.filter((c) => !c.path.startsWith("/api/v1/findings"))).toEqual([]);
}
