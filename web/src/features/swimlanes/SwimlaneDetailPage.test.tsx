import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Role, SwimlaneSummary } from "@/api/generated/model";
import { prefs } from "@/lib/prefs";
import { lane, service } from "@/mocks/design/swimlanes/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/swimlanes/world";
import { json, problem, renderApp } from "@/test/app";

// jsdom cannot lay out a split pane (see the stand-in): the tests use a plain one, the browser tests use the real one.
vi.mock("@/components/ui/resizable", async () => import("@/mocks/design/swimlanes/plain-resizable"));

// The frame every screen of a swimlane shares: its title at once, the banners that say why something is off, the head, the tabs
// that are routes, and the states of the States index for "Swimlane detail" (loading, empty, error, Editor without Operator,
// agent offline, live updates paused). The tabs' own content is in the tests next to each tab.

// The first screen of a run loads two lazy chunks, and the machine may be busy: waiting is cheap, a flaky test is not.
configure({ asyncUtilTimeout: 5000 });

const NOW = Date.now();
const MIN = 60_000;
const COMMIT = "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c";

const SITB: SwimlaneSummary = lane("sitb", {
  tenants: ["sit7", "sit8", "sit9"],
  drift_counts: [["conflict", 1], ["nfs_ahead", 2], ["git_ahead", 1]],
  finding_count: 2,
  last_scan_at: NOW - 5_000,
  base_version: { tag: "v2.3.1", distance: 4, commit: COMMIT },
});

let world: World;
beforeEach(() => {
  prefs.clear();
  world = newWorld("admin", [{ ...SITB }, lane("sit"), lane("presitc", { tier: "PRESIT", tenants: [], baseline: { state: "unconfirmed", by: null, at: null }, drift_counts: [["unknown", 412]] })]);
  world.services["sitb"] = [service("tx-infinity-core"), service("tx-infinity-api")];
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const open = (url: string, role: Role = "admin") => {
  world.role = role;
  return renderApp(url, worldApi(world));
};

describe("the head", () => {
  it("has the swimlane's name as its one title, and it is there before the data is", async () => {
    let release: (r: Response) => void = () => undefined;
    const slow = new Promise<Response>((resolve) => {
      release = resolve;
    });
    const api = worldApi(world);
    renderApp("/swimlanes/sitb", (call) => (new URL(call.path, "http://x").pathname === "/api/v1/swimlanes/sitb" ? slow : api(call)));
    const title = await screen.findByRole("heading", { level: 1 });
    expect(title).toHaveTextContent("sitb");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByLabelText("Loading the swimlane")).toHaveAttribute("aria-busy", "true");
    release(json({ ...SITB, services: [], auto_notify: false }));
    expect(await screen.findByText("Baseline confirmed")).toBeInTheDocument();
  });

  it("lets the name be copied from a button beside the title, which does not rename the title", async () => {
    const user = userEvent.setup();
    open("/swimlanes/sitb");
    const title = await screen.findByRole("heading", { level: 1 });
    expect(title).toHaveAccessibleName("sitb");
    expect(title).not.toContainElement(screen.getByRole("button", { name: "Copy swimlane name" }));
    await user.click(screen.getByRole("button", { name: "Copy swimlane name" }));
    expect(await navigator.clipboard.readText()).toBe("sitb");
  });

  it("shows the tier, the set of tenants, the version, the agent, the baseline and what is not in sync", async () => {
    open("/swimlanes/sitb");
    expect(await screen.findByText("Baseline confirmed")).toBeInTheDocument();
    expect(screen.getAllByText("SIT").length).toBeGreaterThan(0);
    // One branch is not enough to say where a swimlane deploys: the first tenant, and the whole set for whoever asks.
    expect(screen.getAllByText("sit7").length).toBeGreaterThan(0);
    // The shell's switcher shows the set too, so there is more than one of them on screen.
    expect(screen.getAllByText("+2").length).toBeGreaterThan(0);
    expect(screen.getAllByText(/more tenants: sit7, sit8, sit9/).length).toBeGreaterThan(0);
    expect(screen.getByText("v2.3.1 + 4")).toBeInTheDocument();
    expect(screen.getByText("Online")).toBeInTheDocument();
    for (const chip of ["Conflict", "NFS ahead", "Git ahead"]) expect(screen.getAllByText(chip).length).toBeGreaterThan(0);
  });

  it("says how many services need a notify or a restart in the words of D85, never as a pending restart", async () => {
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, pickup_counts: [["needs_notify_or_restart", 2], ["needs_config_server_restart", 1]] } : l));
    open("/swimlanes/sitb/drift");
    await screen.findByText("Baseline confirmed");
    expect(screen.getByText("Needs config-server restart", { selector: "span" })).toBeInTheDocument();
    expect(screen.queryByText(/pending restart/i)).toBeNull();
  });

  it("copes with a swimlane that has no tenants yet", async () => {
    open("/swimlanes/presitc");
    expect(await screen.findByText("No tenants")).toBeInTheDocument();
    expect(screen.getByText("unconfirmed")).toBeInTheDocument();
  });

  it("lets the swimlane's tab bar grow past 36px, so that its tabs can wrap when they do not fit (the layout run, labels 40% longer, at 1024 wide)", async () => {
    open("/swimlanes/sitb");
    const tabs = await screen.findByRole("tablist", { name: "Swimlane" });
    expect(tabs.className).toMatch(/(^|\s)min-h-9(\s|$)/);
    expect(tabs.className).not.toMatch(/(^|\s)h-9(\s|$)/);
  });

  it("holds a swimlane name with no break in it to the header: the title may break anywhere, and the name in the Search button ends in an ellipsis (the layout run: a 3,000px name made the page scroll sideways)", async () => {
    open("/swimlanes/sitb");
    await screen.findByText("Baseline confirmed");
    const title = screen.getByRole("heading", { level: 1 });
    expect(title.className).toMatch(/\[overflow-wrap:anywhere\]/);
    expect(title.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(title.parentElement?.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    const search = screen.getByRole("link", { name: /Search in/ });
    expect(search.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    const name = search.querySelector("[data-slot='safe-text']");
    expect(name?.className).toMatch(/(^|\s)truncate(\s|$)/);
  });

  it("links to the comparison with each of the other swimlanes, and to the search scoped to this one", async () => {
    open("/swimlanes/sitb");
    await screen.findByText("Baseline confirmed");
    expect(screen.getByRole("link", { name: /Search in/ })).toHaveAttribute("href", "/search?lanes=sitb&type=text");
    await userEvent.click(screen.getByRole("button", { name: /Compare with/ }));
    expect(await screen.findByRole("menuitem", { name: "sit" })).toHaveAttribute("href", "/compare?lanes=sitb%2Csit");
    expect(screen.queryByRole("menuitem", { name: "sitb" })).toBeNull();
  });
});

describe("the tabs", () => {
  it("are four routes, with the counts of the swimlane, and the current one is selected", async () => {
    open("/swimlanes/sitb/drift");
    const tabs = await screen.findByRole("tablist", { name: "Swimlane" });
    await within(tabs).findByRole("tab", { name: "Services 2" });
    expect(within(tabs).getByRole("tab", { name: "Services 2" })).toHaveAttribute("href", "/swimlanes/sitb/services");
    expect(within(tabs).getByRole("tab", { name: "Drift 4" })).toHaveAttribute("href", "/swimlanes/sitb/drift");
    expect(within(tabs).getByRole("tab", { name: "Drift 4" })).toHaveAttribute("aria-selected", "true");
    expect(within(tabs).getByRole("tab", { name: "Findings 2" })).toHaveAttribute("href", "/swimlanes/sitb/findings");
    expect(within(tabs).getByRole("tab", { name: "Services 2" })).toHaveAttribute("aria-selected", "false");
  });

  it("opens with Services on the swimlane's own address", async () => {
    open("/swimlanes/sitb");
    const tabs = await screen.findByRole("tablist", { name: "Swimlane" });
    expect(await within(tabs).findByRole("tab", { name: "Services 2" })).toHaveAttribute("aria-selected", "true");
  });

  it("changes the route, not the page, when a tab is chosen", async () => {
    const app = open("/swimlanes/sitb/services");
    await userEvent.click(await screen.findByRole("tab", { name: /^Findings/ }));
    await app.settled();
    expect(app.location()).toBe("/swimlanes/sitb/findings");
    expect(await screen.findByRole("tab", { name: /^Findings/ })).toHaveAttribute("aria-selected", "true");
  });

  it("gives an Admin the Activity tab as a link", async () => {
    open("/swimlanes/sitb", "admin");
    expect(await screen.findByRole("tab", { name: "Activity" })).toHaveAttribute("href", "/swimlanes/sitb/activity");
  });

  it("leaves the Activity tab in place for anyone else, disabled, with the role it needs", async () => {
    const app = open("/swimlanes/sitb", "operator");
    const tab = await screen.findByRole("tab", { name: "Activity" });
    expect(tab).toHaveAttribute("aria-disabled", "true");
    expect(tab).not.toHaveAttribute("href");
    await userEvent.click(tab);
    expect(app.location()).toBe("/swimlanes/sitb");
    await userEvent.hover(tab);
    expect((await screen.findAllByText("Needs the Admin role. Ask an Admin to grant it.")).length).toBeGreaterThan(0);
  });
});

describe("the banners", () => {
  it("says editing is locked when the baseline is not confirmed, and gives an Admin the way to adopt", async () => {
    open("/swimlanes/presitc", "admin");
    expect(await screen.findByText("Editing locked")).toBeInTheDocument();
    expect(screen.getByText("presitc has no confirmed baseline. An admin must finish adoption first.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Start adoption" })).toHaveAttribute("href", "/swimlanes/presitc/adopt");
    expect(screen.getAllByText("Adoption needed").length).toBeGreaterThan(0);
  });

  it("tells anyone else to ask an admin, and offers nothing it cannot do", async () => {
    open("/swimlanes/presitc", "operator");
    expect(await screen.findByText("Editing locked")).toBeInTheDocument();
    expect(screen.getByText("Ask an admin to finish adoption.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Start adoption" })).toBeNull();
  });

  it("says an adoption in progress is that, and offers an Admin the way back into it", async () => {
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, baseline: { state: "in_progress", by: null, at: null } } : l));
    open("/swimlanes/sitb", "admin");
    expect(await screen.findByText("sitb has an adoption in progress. Editing stays locked until an admin confirms the baseline.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Continue adoption" })).toHaveAttribute("href", "/swimlanes/sitb/adopt");
  });

  it("shows no lock banner for a confirmed baseline", async () => {
    open("/swimlanes/sitb");
    await screen.findByText("Baseline confirmed");
    expect(screen.queryByText("Editing locked")).toBeNull();
  });

  it("says when the agent went offline, stamps the data with that time, and Check agent reads the swimlane again", async () => {
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, agent: "disconnected", last_scan_at: NOW - 14 * MIN } : l));
    const app = open("/swimlanes/sitb");
    expect(await screen.findByText(/^Agent offline since \d\d:\d\d UTC$/)).toBeInTheDocument();
    expect(screen.getByText(/^Showing data as of \d\d:\d\d UTC\. Restarts are off until the agent reconnects\.$/)).toBeInTheDocument();
    const before = app.calls.filter((c) => c.path === "/api/v1/swimlanes/sitb").length;
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, agent: "connected" } : l));
    await userEvent.click(screen.getByRole("button", { name: "Check agent" }));
    await waitFor(() => expect(screen.queryByText(/^Agent offline since/)).toBeNull());
    expect(app.calls.filter((c) => c.path === "/api/v1/swimlanes/sitb").length).toBeGreaterThan(before);
  });

  it("says an agent that never reported has not, rather than that it went offline", async () => {
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, agent: "never_seen", last_scan_at: null } : l));
    open("/swimlanes/sitb");
    expect(await screen.findByText("Agent has not reported yet")).toBeInTheDocument();
    expect(screen.getByText("Restarts are off until the agent connects.")).toBeInTheDocument();
  });

  it("says a sync is in progress and names the job", async () => {
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, syncing: { job: { name: "csp-tenant-data-sit7", uid: "run-42" }, since: NOW - MIN } } : l));
    open("/swimlanes/sitb");
    expect(await screen.findByText("Sync in progress")).toBeInTheDocument();
    expect(screen.getByText("Alerts are held until the sync completes.")).toBeInTheDocument();
    expect(screen.getByText("csp-tenant-data-sit7")).toBeInTheDocument();
  });
});

describe("the states of the States index", () => {
  it("shows the problem's title and the request ID when the swimlane cannot be read, and keeps the title of the page", async () => {
    const app = open("/swimlanes/nope");
    expect(await screen.findByText("Couldn't load services")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("nope");
    app.unmount();
  });

  it("shows the error with Retry for a failure that trying again can change", async () => {
    const api = worldApi(world);
    let failing = true;
    renderApp("/swimlanes/sitb", (call) => (failing && new URL(call.path, "http://x").pathname === "/api/v1/swimlanes/sitb" ? problem(500, "internal") : api(call)));
    expect(await screen.findByText("Couldn't load services")).toBeInTheDocument();
    failing = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Baseline confirmed")).toBeInTheDocument();
  });
});
