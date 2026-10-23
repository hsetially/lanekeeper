import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Role } from "@/api/generated/model";
import { liveByView } from "@/components/pickup";
import { prefs } from "@/lib/prefs";
import { lane, service } from "@/mocks/design/swimlanes/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/swimlanes/world";
import { json, problem, renderApp, TOKEN } from "@/test/app";
import { expectOneBlockInAlert } from "@/test/alert-sentence";

// The Services tab and the restart dialogs: what each service says about its config (D85), who may restart, and every answer
// the server can give to a restart: done, sent for approval (202), locked (423), refused (422), and no answer.

vi.mock("@/components/ui/resizable", async () => import("@/mocks/design/swimlanes/plain-resizable"));
configure({ asyncUtilTimeout: 5000 });

const NOW = Date.now();
const MIN = 60_000;

let world: World;
beforeEach(() => {
  prefs.clear();
  world = newWorld("operator", [lane("sitb", { tenants: ["sit7"], last_scan_at: NOW - 5_000 }), lane("presitc", { tier: "PRESIT", baseline: { state: "unconfirmed", by: null, at: null } })]);
  world.services["sitb"] = [
    service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: NOW - 12 * MIN, pending_files: ["tx-infinity-api/tx-infinity-core-sit7.yml"] }),
    service("tx-infinity-api", { pickup: "needs_notify_or_restart", pending_since: NOW - 3 * MIN, pending_files: ["application.properties", "tx-infinity-api/a.yml", "tx-infinity-api/b.yml"] }),
    service("tx-infinity-gateway"),
    service("remote-itm-teller", { pickup: "live_within_ttl", live_by: NOW + 22 * MIN }),
    service("config-server", { pickup: "needs_config_server_restart", pending_since: NOW - MIN, pending_files: ["channels.yml"] }),
  ];
  world.services["presitc"] = [service("tx-infinity-core", { pickup: "live" }, "presitc")];
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const open = (url: string, role: Role = "operator") => {
  world.role = role;
  return renderApp(url, worldApi(world));
};
const rowOf = async (name: string): Promise<HTMLElement> => {
  const cell = await screen.findByText(name, { selector: "bdi" });
  const row = cell.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no row for ${name}`);
  return row;
};
const restartButton = (name: string) => screen.findByRole("button", { name: `Restart ${name}` });

describe("the list of services", () => {
  it("lets 'since 12 min ago' give way with an ellipsis instead of running under the Restart button at 1024 wide (found by axe and the layout run)", async () => {
    open("/swimlanes/sitb/services");
    const since = within(await rowOf("tx-infinity-core")).getByText(/since 12 min ago/);
    expect(since.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(since.className).not.toMatch(/whitespace-nowrap/);
    expect(since).toHaveAttribute("title", "since 12 min ago");
    // The line that holds it can shrink, or the text has nothing to give way to.
    expect(since.parentElement?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
  });

  it("says for each service whether it has picked up its config, in the four states of D85 and never as a bare pending restart", async () => {
    open("/swimlanes/sitb/services");
    const core = await rowOf("tx-infinity-core");
    expect(within(core).getByText("Needs notify or restart")).toBeInTheDocument();
    expect(within(core).getByText(/since 12 min ago/)).toBeInTheDocument();
    expect(within(core).getByText("tx-infinity-core-sit7.yml")).toBeInTheDocument();
    expect(within(await rowOf("tx-infinity-gateway")).getByText("Live")).toBeInTheDocument();
    expect(within(await rowOf("remote-itm-teller")).getByText(`Live by ${liveByView(NOW + 22 * MIN, Date.now()).text}`)).toBeInTheDocument();
    expect(within(await rowOf("config-server")).getByText("Needs config-server restart")).toBeInTheDocument();
    expect(screen.queryByText(/pending restart/i)).toBeNull();
  });

  it("names the first file a service waits on and counts the rest, with all of them in the tooltip", async () => {
    open("/swimlanes/sitb/services");
    const api = await rowOf("tx-infinity-api");
    expect(within(api).getByText("application.properties")).toBeInTheDocument();
    expect(within(api).getByText(/and 2 more/)).toBeInTheDocument();
    expect(within(api).getByTitle(/application\.properties\s+tx-infinity-api\/a\.yml\s+tx-infinity-api\/b\.yml/)).toBeInTheDocument();
  });

  it("opens on the swimlane's own address too, and counts the services in the strip", async () => {
    open("/swimlanes/sitb");
    expect(await screen.findByText("5 services")).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Services" })).toBeInTheDocument();
  });

  it("filters by name as the person types, and keeps the text in the address", async () => {
    const app = open("/swimlanes/sitb/services");
    await rowOf("tx-infinity-core");
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter services" }), "gateway");
    await waitFor(() => expect(screen.queryByText("tx-infinity-core", { selector: "bdi" })).toBeNull());
    expect(screen.getByText("1 of 5 services")).toBeInTheDocument();
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitb/services?q=gateway"));
  });

  it("starts from the address", async () => {
    open("/swimlanes/sitb/services?q=teller");
    expect(await screen.findByText("remote-itm-teller", { selector: "bdi" })).toBeInTheDocument();
    expect(screen.queryByText("tx-infinity-core", { selector: "bdi" })).toBeNull();
    expect(screen.getByRole("searchbox", { name: "Filter services" })).toHaveValue("teller");
  });

  it("says no service matches, and takes a pattern character as itself", async () => {
    open("/swimlanes/sitb/services");
    await rowOf("tx-infinity-core");
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter services" }), ".*");
    expect(await screen.findByText("No service matches")).toBeInTheDocument();
  });

  it("focuses the filter on /", async () => {
    open("/swimlanes/sitb/services");
    await rowOf("tx-infinity-core");
    await userEvent.keyboard("/");
    expect(screen.getByRole("searchbox", { name: "Filter services" })).toHaveFocus();
  });

  it("says what is missing when the agent reported no services", async () => {
    world.services["sitb"] = [];
    open("/swimlanes/sitb/services");
    expect(await screen.findByText("No services reported")).toBeInTheDocument();
    expect(screen.getByText(/has not reported any services yet\. Check that it is running in its namespace\./)).toBeInTheDocument();
  });
});

describe("who may restart", () => {
  it("lets an Operator press Restart", async () => {
    open("/swimlanes/sitb/services", "operator");
    expect(await restartButton("tx-infinity-core")).not.toHaveAttribute("aria-disabled");
  });

  it.each<Role>(["viewer", "editor"])("keeps Restart on screen for a %s, disabled, with the role it needs, and opens nothing", async (role) => {
    open("/swimlanes/sitb/services", role);
    const button = await restartButton("tx-infinity-core");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("Needs the Operator role. Ask an Admin to grant it.");
    await userEvent.click(button);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("turns Restart off while the agent is offline, and says why", async () => {
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, agent: "disconnected", last_scan_at: NOW - 14 * MIN } : l));
    open("/swimlanes/sitb/services", "operator");
    const button = await restartButton("tx-infinity-core");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription("Unavailable while the agent is offline");
  });
});

describe("restarting a service", () => {
  it("asks first: the consequence, what the restart picks up, a warning for the others, and focus on Cancel", async () => {
    open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    const dialog = await screen.findByRole("alertdialog", { name: "Restart tx-infinity-core in sitb?" });
    expect(within(dialog).getByText("Restarting tx-infinity-core replaces its pods. Requests to it can fail while its pods come back.")).toBeInTheDocument();
    expect(within(dialog).getByText("tx-infinity-core-sit7.yml")).toBeInTheDocument();
    expect(within(dialog).getByText(/will be interrupted/)).toBeInTheDocument();
    // One sentence in its box: the Alert lays its children out as a column, so a name in the middle of it must not be a line of its own.
    expectOneBlockInAlert(within(dialog).getByText(/will be interrupted/));
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    expect(within(dialog).getByRole("button", { name: "Restart tx-infinity-core" })).toBeInTheDocument();
  });

  it("sends nothing when the person cancels", async () => {
    const app = open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(app.calls.filter((c) => c.method === "POST")).toEqual([]);
  });

  it("sends the service with the CSRF token and an Idempotency-Key, then says the request was accepted, and reads the swimlane again", async () => {
    const app = open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    expect(await screen.findByText("Restart requested for tx-infinity-core")).toBeInTheDocument();
    expect(screen.getByText(/Restart progress is not shown here/)).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const post = app.calls.find((c) => c.method === "POST");
    expect(post?.path).toBe("/api/v1/swimlanes/sitb/restart");
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ namespace: "sitb", name: "tx-infinity-core" });
    expect(post?.headers.get("x-csrf-token")).toBe(TOKEN);
    expect(post?.headers.get("idempotency-key")).toMatch(/\S/);
    expect(app.calls.filter((c) => c.method === "GET" && c.path === "/api/v1/swimlanes/sitb").length).toBeGreaterThan(1);
  });

  it("shows nothing restarting that it was not told: no pods, no progress, no service that is back", async () => {
    open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    await screen.findByText("Restart requested for tx-infinity-core");
    expect(screen.queryByText(/Restarting \d+ services/)).toBeNull();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("says a restart that was sent for approval is not done, names the request, and offers an Admin the request", async () => {
    world.restart = json({ outcome: "proposal_created", id: 12 });
    open("/swimlanes/sitb/services", "admin");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    const dialog = await screen.findByRole("dialog", { name: "Restart sent for approval" });
    expect(within(dialog).getByText("Nothing restarts until an Admin approves request AP-12.")).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/12");
    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(await screen.findByText("Restart of tx-infinity-core sent for approval")).toBeInTheDocument();
    expect(screen.queryByText("Restart requested for tx-infinity-core")).toBeNull();
  });

  it("offers no link to the request to someone who cannot open it", async () => {
    world.restart = json({ outcome: "proposal_created", id: 12 });
    open("/swimlanes/sitb/services", "operator");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    const dialog = await screen.findByRole("dialog", { name: "Restart sent for approval" });
    expect(within(dialog).queryByRole("link", { name: "View request" })).toBeNull();
  });

  it("keeps the dialog open on a 423, says what holds the lock as far as it knows, and shows the request ID", async () => {
    world.restart = problem(423, "locked", { until: null });
    open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(await within(dialog).findByText("sitb is locked")).toBeInTheDocument();
    expect(within(dialog).getByText(/Another operation is using it/)).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(screen.queryByText("Restart requested for tx-infinity-core")).toBeNull();
  });

  it("derives the cause of a 423 from the baseline when the swimlane is not adopted", async () => {
    world.restart = problem(423, "locked", { until: null });
    open("/swimlanes/presitc/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    expect(await screen.findByText("presitc is locked until it is adopted")).toBeInTheDocument();
  });

  it("shows a refusal inside the dialog with the request ID, and the server's detail nowhere", async () => {
    world.restart = problem(422, "unprocessable", { detail: "SELECT secret FROM vault" });
    open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(await within(dialog).findByText("The server did not accept this restart. Nothing was restarted.")).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("SELECT secret");
  });

  it("does not say the restart was not sent when nothing came back", async () => {
    world.restart = () => {
      throw new TypeError("Failed to fetch");
    };
    open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    expect(await screen.findByText("Couldn't reach the server", {}, { timeout: 15_000 })).toBeInTheDocument();
    expect(screen.getByText(/may or may not have been sent/)).toBeInTheDocument();
  }, 20_000);

  it("lets the person try again after a refusal, from the same dialog", async () => {
    world.restart = problem(423, "locked", { until: null });
    const app = open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    await screen.findByText("sitb is locked");
    world.restart = json({ outcome: "applied", hash: null, audit_id: 9 });
    await userEvent.click(screen.getByRole("button", { name: "Restart tx-infinity-core" }));
    expect(await screen.findByText("Restart requested for tx-infinity-core")).toBeInTheDocument();
    expect(app.calls.filter((c) => c.method === "POST")).toHaveLength(2);
  });

  it("does not leave an old error in the dialog when it is opened for another service", async () => {
    world.restart = problem(423, "locked", { until: null });
    open("/swimlanes/sitb/services");
    await userEvent.click(await restartButton("tx-infinity-core"));
    await userEvent.click(await screen.findByRole("button", { name: "Restart tx-infinity-core" }));
    await screen.findByText("sitb is locked");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await userEvent.click(await restartButton("tx-infinity-api"));
    const dialog = await screen.findByRole("alertdialog", { name: "Restart tx-infinity-api in sitb?" });
    expect(within(dialog).queryByText("sitb is locked")).toBeNull();
  });
});

describe("an Editor, who may edit but not restart (States P04)", () => {
  it("is told so on the tab, besides the disabled buttons", async () => {
    open("/swimlanes/sitb/services", "editor");
    expect(await screen.findByText("Restart needs Operator")).toBeInTheDocument();
    expect(screen.getByText("You can edit files here, but restarting services needs the Operator permission.")).toBeInTheDocument();
  });

  it("is not said to a Viewer, whose tab is read-only as a whole, nor to an Operator", async () => {
    const viewer = open("/swimlanes/sitb/services", "viewer");
    await rowOf("tx-infinity-core");
    expect(screen.queryByText("Restart needs Operator")).toBeNull();
    viewer.unmount();
    open("/swimlanes/sitb/services", "operator");
    await rowOf("tx-infinity-core");
    expect(screen.queryByText("Restart needs Operator")).toBeNull();
  });
});

describe("the tab bar of an agent that is offline (States O02)", () => {
  it("is stamped with when the data is from", async () => {
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, agent: "disconnected", last_scan_at: NOW - 14 * MIN } : l));
    open("/swimlanes/sitb/services");
    expect(await screen.findByText(/^as of \d\d:\d\d UTC$/)).toBeInTheDocument();
  });
});

describe("what the tab says about notifying and about the config-server (T6)", () => {
  it("has a Notify services button in the strip for an Operator when a service waits on a file", async () => {
    open("/swimlanes/sitb/services");
    await rowOf("tx-infinity-core");
    expect(await screen.findByRole("button", { name: "Notify services" })).toBeEnabled();
  });

  it("lets the strip wrap, so that the filter, the count and the button are never cut off in the narrow pane of a 1024 px window (found by the layout run)", async () => {
    open("/swimlanes/sitb/services");
    await rowOf("tx-infinity-core");
    const strip = (await screen.findByRole("button", { name: "Notify services" })).closest<HTMLElement>('[data-slot="services-strip"]');
    if (strip === null) throw new Error("the strip has no data-slot");
    // The pane beside the file tree is 360 px wide at 1024: the filter (224), the count and the button do not fit in one line, and a
    // strip of a fixed height cut the button off at the pane's edge. The count and the button go together to the next line instead.
    expect(strip.className).toMatch(/(^|\s)flex-wrap(\s|$)/);
    expect(strip.className).toMatch(/(^|\s)min-h-8(\s|$)/);
    expect(strip.className).not.toMatch(/(^|\s)h-8(\s|$)/);
    const count = within(strip).getByText("5 services");
    expect(count.parentElement).toBe(within(strip).getByRole("button", { name: "Notify services" }).parentElement);
    expect(count.parentElement?.className).toMatch(/(^|\s)ml-auto(\s|$)/);
  });

  it("disables it, with the reason, below Operator and when no service waits on a file", async () => {
    const viewer = open("/swimlanes/sitb/services", "viewer");
    await rowOf("tx-infinity-core");
    expect(await screen.findByRole("button", { name: "Notify services" })).toHaveAttribute("aria-disabled", "true");
    viewer.unmount();
    world.services["sitb"] = [service("tx-infinity-gateway")];
    open("/swimlanes/sitb/services");
    await rowOf("tx-infinity-gateway");
    expect(await screen.findByRole("button", { name: "Notify services" })).toHaveAttribute("aria-disabled", "true");
  });

  it("says whether auto-notify is on, from the swimlane's own answer, and who can change it", async () => {
    open("/swimlanes/sitb/services");
    await rowOf("tx-infinity-core");
    const off = screen.getByRole("group", { name: "Auto-notify" });
    expect(within(off).getByText("Auto-notify off")).toBeInTheDocument();
    expect(within(off).getByText("Only an Admin can change this.")).toBeInTheDocument();
  });

  it("says it is on, with a link to the Admin page for an Admin", async () => {
    world.auto_notify["sitb"] = true;
    open("/swimlanes/sitb/services", "admin");
    await rowOf("tx-infinity-core");
    const on = screen.getByRole("group", { name: "Auto-notify" });
    expect(within(on).getByText("Auto-notify on")).toBeInTheDocument();
    expect(within(on).getByRole("link", { name: "Change in Admin" })).toHaveAttribute("href", "/admin/swimlanes");
  });

  it("says a config-server restart is needed when a service reports it, with the button to ask for it", async () => {
    open("/swimlanes/sitb/services");
    await rowOf("config-server");
    const banner = (await screen.findByText("Config-server restart needed")).closest<HTMLElement>('[data-slot="alert"]');
    if (banner === null) throw new Error("the config-server banner is not an Alert");
    expect(within(banner).getByText(/1 service reports needing a config-server restart\./)).toBeInTheDocument();
    expect(within(banner).getByRole("button", { name: "Restart config-server (type its name to confirm)" })).toBeEnabled();
  });

  it("gives the banner's Restart config-server button a name of its own, apart from the Restart button of a service that is itself called config-server (found by the accessible-name check of the T6 accessibility run)", async () => {
    open("/swimlanes/sitb/services");
    const row = await rowOf("config-server");
    const banner = (await screen.findByText("Config-server restart needed")).closest<HTMLElement>('[data-slot="alert"]');
    if (banner === null) throw new Error("the config-server banner is not an Alert");
    const rowButton = within(row).getByRole("button", { name: /^Restart config-server/ });
    const bannerButton = within(banner).getByRole("button", { name: /^Restart config-server/ });
    expect(bannerButton).not.toBe(rowButton);
    // The row's restarts the one service and keeps the name it always had; the banner's is the typed-confirmation restart of the
    // config-server Deployment the hub offers, and says so. A screen reader's list of buttons must tell them apart, and so must a test.
    expect(screen.getAllByRole("button", { name: "Restart config-server" })).toEqual([rowButton]);
    expect(bannerButton).toHaveAccessibleName("Restart config-server (type its name to confirm)");
    // The visible words are still in the name (WCAG 2.5.3, Label in Name).
    expect(bannerButton).toHaveTextContent("Restart config-server");
  });

  it("does not say it when nothing needs the config-server to restart", async () => {
    world.services["sitb"] = [service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: NOW - MIN, pending_files: ["application.properties"] })];
    open("/swimlanes/sitb/services");
    await rowOf("tx-infinity-core");
    expect(screen.queryByText("Config-server restart needed")).toBeNull();
  });
});
