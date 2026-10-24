import { configure, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { Role, SwimlaneDetail } from "@/api/generated/model";
import { scenarios } from "@/mocks/design/notify";
import { contextFor, resolveBody, type MockContext, type MockResponse, type Scenario } from "@/mocks/scenario";
import { AutoNotifyStatus } from "./AutoNotifyStatus";
import { ConfigServerRestartBanner } from "./ConfigServerRestartBanner";
import { NotifyServicesButton } from "./NotifyServicesButton";
import { json, problem, renderNotify, type Api } from "./test-utils";

// The scenarios the e2e harness serves drive these pieces as they are: the data in `src/mocks/design/notify` is what the screens
// need, so a visual or journey test built on it shows the pieces and not an artefact of the mock.

configure({ asyncUtilTimeout: 5000 });
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

/** The path parameters of `path` if it is an instance of the OpenAPI template of `response`, else null. */
function match(response: MockResponse, method: string, path: string): Record<string, string> | null {
  if (response.method !== method) return null;
  const names: string[] = [];
  const source = response.path.replace(/[.*+?^$()|[\]\\]/g, "\\$&").replace(/\{([A-Za-z_]+)\}/g, (_whole, name: string) => {
    names.push(name);
    return "([^/]+)";
  });
  const found = new RegExp(`^${source}$`).exec(path);
  return found === null ? null : Object.fromEntries(names.map((name, i) => [name, decodeURIComponent(found[i + 1] ?? "")]));
}

/** `fetch` answered from a scenario, the way the harness looks a response up: by method and OpenAPI template. */
function apiOf(scenario: Scenario): Api {
  return (call) => {
    const url = new URL(call.path, "http://hub.test");
    for (const response of scenario.responses) {
      const params = match(response, call.method, url.pathname);
      if (params === null) continue;
      const context: MockContext = { params, query: url.searchParams, headers: call.headers, body: call.body === undefined ? undefined : (JSON.parse(call.body) as unknown) };
      const body = resolveBody(response, context);
      const status = response.status ?? 200;
      return status >= 400 ? problem(status, "locked", body as Record<string, unknown>) : json(body, status);
    }
    return problem(404, "not_found");
  };
}

const scenario = (id: string): Scenario => {
  const found = scenarios.find((s) => s.id === id);
  if (!found) throw new Error(`no scenario ${id}`);
  return found;
};

function laneOf(scenarioId: string): SwimlaneDetail {
  const response = scenario(scenarioId).responses.find((r) => r.method === "GET" && r.path === "/api/v1/swimlanes/{id}");
  if (!response) throw new Error("no swimlane detail");
  return resolveBody(response, { ...contextFor(response), params: { id: "sitb" } }) as SwimlaneDetail;
}

const show = (id: string, ui: (lane: SwimlaneDetail) => React.ReactElement, role: Role = "admin") => {
  const lane = laneOf(id);
  return renderNotify(ui(lane), { api: apiOf(scenario(id)), role });
};

describe("design-notify-services-tab", () => {
  it("offers to notify the two files the two waiting services are waiting on, and the hub accepts them", async () => {
    const { calls } = show("design-notify-services-tab", (lane) => <NotifyServicesButton lane={lane} />);
    await userEvent.click(await screen.findByRole("button", { name: "Notify services" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Notify services in sitb?" });
    const files = within(dialog).getByRole("region", { name: "2 files to announce" });
    expect(within(files).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["application.properties", "tx-infinity-api/tx-infinity-core-sit7.yml"]);
    const services = within(dialog).getByRole("region", { name: "2 services waiting on these files" });
    expect(within(services).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["sitb/tx-infinity-core notifications reported on", "sitb/ui-service notifications reported off"]);
    await userEvent.click(within(dialog).getByRole("button", { name: "Notify services" }));
    expect(await screen.findByRole("dialog", { name: "Notification sent" })).toHaveAccessibleDescription("The hub accepted 2 paths for sitb.");
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
  });

  it("shows auto-notify on", async () => {
    show("design-notify-services-tab", (lane) => <AutoNotifyStatus lane={lane} />);
    expect(await screen.findByText("Auto-notify on")).toBeInTheDocument();
  });
});

describe("the other answers to a notify", () => {
  it.each([
    ["design-notify-approval", "Notification sent for approval"],
    ["design-notify-partial", "Notification partly accepted"],
  ])("%s ends in %s", async (id, title) => {
    show(id, (lane) => <NotifyServicesButton lane={lane} />);
    await userEvent.click(await screen.findByRole("button", { name: "Notify services" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Notify services" }));
    expect(await screen.findByRole("dialog", { name: title })).toBeInTheDocument();
  });

  it("design-notify-locked keeps the refusal in the dialog", async () => {
    show("design-notify-locked", (lane) => <NotifyServicesButton lane={lane} />);
    await userEvent.click(await screen.findByRole("button", { name: "Notify services" }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Notify services" }));
    expect(await within(dialog).findByText("sitb is locked")).toBeInTheDocument();
  });
});

describe("design-notify-auto-notify-off and design-notify-viewer", () => {
  it("shows auto-notify off", async () => {
    show("design-notify-auto-notify-off", (lane) => <AutoNotifyStatus lane={lane} />);
    expect(await screen.findByText("Auto-notify off")).toBeInTheDocument();
  });

  it("gives a Viewer a disabled button and a read-only line with no link", async () => {
    show(
      "design-notify-viewer",
      (lane) => (
        <>
          <NotifyServicesButton lane={lane} />
          <AutoNotifyStatus lane={lane} />
        </>
      ),
      "viewer",
    );
    expect(await screen.findByRole("button", { name: "Notify services" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("Only an Admin can change this.")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });
});

describe("the config-server restart scenarios", () => {
  const open = async () => {
    await userEvent.click(await screen.findByRole("button", { name: "Restart config-server (type its name to confirm)" }));
  };

  it("design-notify-config-server: the banner, the offer, and four services that are affected", async () => {
    show("design-notify-config-server", (lane) => <ConfigServerRestartBanner lane={lane} />);
    expect(await screen.findByText(/Check C11 is open/)).toBeInTheDocument();
    await open();
    const dialog = await screen.findByRole("alertdialog", { name: "Restart csp-configuration-server in sitb?" });
    expect(within(dialog).getByRole("region", { name: "4 other services affected" })).toBeInTheDocument();
  });

  it("design-notify-config-server-truncated: says the list is not the whole of them", async () => {
    show("design-notify-config-server-truncated", (lane) => <ConfigServerRestartBanner lane={lane} />);
    await open();
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/the list is not the whole of the services affected/)).toBeInTheDocument();
  });

  it("design-notify-config-server-none: says the hub offers none", async () => {
    show("design-notify-config-server-none", (lane) => <ConfigServerRestartBanner lane={lane} />);
    await open();
    expect(await screen.findByRole("dialog", { name: "No config-server restart is offered" })).toBeInTheDocument();
  });

  it("design-notify-config-server-approval: ends in a request for approval", async () => {
    show("design-notify-config-server-approval", (lane) => <ConfigServerRestartBanner lane={lane} />);
    await open();
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.type(within(dialog).getByRole("textbox"), "csp-configuration-server");
    await userEvent.click(within(dialog).getByRole("button", { name: "Restart csp-configuration-server" }));
    expect(await screen.findByRole("dialog", { name: "Config-server restart sent for approval" })).toHaveAccessibleDescription("Nothing restarts until an Admin approves request AP-2044.");
  });

  it("design-notify-services-tab: shows no banner, because C11 is not open and no service waits for the config-server", async () => {
    const { container } = show("design-notify-services-tab", (lane) => <ConfigServerRestartBanner lane={lane} />);
    await new Promise((resolve) => setTimeout(resolve, 80));
    expect(container.querySelector("[data-slot=alert]")).toBeNull();
  });
});
