import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toasts } from "@/features/shell/toast/store";
import { NOW } from "@/mocks/design/admin/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/admin/world";
import { json, problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Swimlanes and agents (route /admin/swimlanes, Admin only): the swimlanes the hub knows, the agent registered for each, and per
// swimlane its tenants, its auto-notify switch, the service account its agent runs as, and the fallback join token.
//
// What is NOT here is registering a swimlane (its name, tier, project, cluster): the contract has no operation for it (CG15), so the
// page says so and offers nothing that would only look like it works.

const FIRST = { timeout: 8000 };
const writes = <T extends { method: string }>(calls: readonly T[]): T[] => calls.filter((c) => c.method !== "GET");

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  Reflect.deleteProperty(document, "cookie");
});

async function open(world: World, url = "/admin/swimlanes") {
  const app = renderApp(url, worldApi(world));
  await screen.findByRole("heading", { level: 1, name: "Admin" }, FIRST);
  return app;
}
const lanesTable = () => screen.findByRole("grid", { name: "Swimlanes" });
const agentsTable = () => screen.findByRole("grid", { name: "Agents" });
async function rowIn(table: HTMLElement, text: string): Promise<HTMLElement> {
  const cell = await within(table).findByText(text);
  const row = cell.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no row for ${text}`);
  return row;
}
async function manage(user: ReturnType<typeof userEvent.setup>, id: string) {
  await user.click(await screen.findByRole("button", { name: `Manage ${id}` }));
  return screen.findByRole("region", { name: `Settings for ${id}` });
}

describe("with words 40% longer (the layout run, German-length labels)", () => {
  it("ends the agent's state in an ellipsis in both lists, and gives Manage the room for its longer word", async () => {
    await open(newWorld());
    for (const table of [await lanesTable(), await agentsTable()]) {
      const label = within(table).getAllByText(/^(Online|Offline|Never seen)/, { selector: "span.truncate" })[0];
      expect(label).toBeDefined();
      expect(label?.parentElement?.className).toMatch(/(^|\s)flex(\s|$)/);
      expect(label?.parentElement?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
      expect(label?.parentElement?.className).not.toMatch(/inline-flex/);
    }
    const grid = await lanesTable();
    const template = grid.querySelector<HTMLElement>("[data-slot='virtual-list']")?.style.getPropertyValue("--lk-cols") ?? grid.style.getPropertyValue("--lk-cols");
    const tracks = template.split(/\s+(?![^(]*\))/);
    expect(Number.parseInt(tracks[tracks.length - 1] ?? "0", 10)).toBeGreaterThanOrEqual(112);
  });
});

describe("the lists", () => {
  it("is the swimlanes section of the Admin area, and it offers no way to register a swimlane", async () => {
    await open(newWorld());
    expect(await screen.findByRole("heading", { level: 2, name: "Swimlanes and agents" })).toBeInTheDocument();
    expect(within(screen.getByRole("navigation", { name: "Admin sections" })).getByRole("link", { name: "Swimlanes and agents" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("Registering a swimlane from this page is not available yet")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /register swimlane/i })).toBeNull();
    expect(screen.queryByLabelText(/GCP project/i)).toBeNull();
  });

  it("lists the swimlanes with project, tier, tenants and agent", async () => {
    await open(newWorld());
    expect(await screen.findByText("6 registered")).toBeInTheDocument();
    const sitb = await rowIn(await lanesTable(), "sitb");
    expect(within(sitb).getByText("ops-sit")).toBeInTheDocument();
    expect(within(sitb).getByText("SIT")).toBeInTheDocument();
    expect(within(sitb).getByText("sit7")).toBeInTheDocument();
    expect(within(sitb).getByRole("img", { name: "Agent online" })).toBeInTheDocument();
    const presitb = await rowIn(await lanesTable(), "presitb");
    expect(within(presitb).getByText("PRESIT")).toBeInTheDocument();
    expect(within(presitb).getByRole("img", { name: "Agent offline" })).toBeInTheDocument();
  });

  it("lists the agents with service account, status, replica, last heartbeat and certificate expiry", async () => {
    await open(newWorld());
    expect(await screen.findByText("6 registered · 1 offline · 1 never seen")).toBeInTheDocument();
    const sit = await rowIn(await agentsTable(), "sit");
    expect(within(sit).getByText("lanekeeper-agent@dfb-nonprod-sit.iam.gserviceaccount.com")).toBeInTheDocument();
    expect(within(sit).getByText("Online")).toBeInTheDocument();
    expect(within(sit).getByText("lk-agent-sit-x2kqz")).toBeInTheDocument();
    expect(within(sit).getByText("just now")).toBeInTheDocument();
    expect(within(sit).getByText("in 42 d")).toBeInTheDocument();
    const offline = await rowIn(await agentsTable(), "presitb");
    expect(within(offline).getByText("Offline")).toBeInTheDocument();
    expect(within(offline).getByText("2 h ago")).toBeInTheDocument();
    const never = await rowIn(await agentsTable(), "presitc");
    expect(within(never).getByText("Never seen")).toBeInTheDocument();
    expect(within(never).getByText("Never")).toBeInTheDocument();
  });

  it("warns, in words as well as colour, when a certificate ends within two weeks", async () => {
    await open(newWorld());
    const soon = await rowIn(await agentsTable(), "presita");
    expect(within(soon).getByText(/expires soon/i)).toBeInTheDocument();
    const fine = await rowIn(await agentsTable(), "sit");
    expect(within(fine).queryByText(/expires soon/i)).toBeNull();
  });

  it("says no swimlane is registered when there is none, and still offers nothing to register", async () => {
    const world = newWorld();
    world.lanes = [];
    world.agents = [];
    await open(world);
    expect(await screen.findByText("No swimlanes registered")).toBeInTheDocument();
    expect(screen.getByText(/can't be registered from this page yet/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /register/i })).toBeNull();
  });

  it("shows skeletons while both lists load, under a heading that is already there", async () => {
    const world = newWorld();
    world.hold.add("/api/v1/swimlanes");
    world.hold.add("/api/v1/admin/agents");
    await open(world);
    expect(await screen.findByLabelText("Loading swimlanes")).toHaveAttribute("aria-busy", "true");
    expect(await screen.findByLabelText("Loading agents")).toHaveAttribute("aria-busy", "true");
  });

  it("says which list could not be loaded, with the request id, and the other still shows", async () => {
    const world = newWorld();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/agents"), respond: () => problem(500, "internal"), once: true });
    await open(world);
    expect(await screen.findByText("Couldn't load agents")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(await rowIn(await lanesTable(), "sitb")).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(await rowIn(await agentsTable(), "sitb")).toBeInTheDocument();
  });

  it.each(["viewer", "editor", "operator"] as const)("shows the permission state to a %s", async (role) => {
    const app = renderApp("/admin/swimlanes", worldApi(newWorld(role)));
    expect(await screen.findByRole("heading", { level: 1, name: "Admin needs the Admin role" }, FIRST)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin"))).toBe(false);
  });
});

describe("one swimlane's settings", () => {
  it("opens the settings of the swimlane chosen, and names it", async () => {
    const user = userEvent.setup();
    await open(newWorld());
    const panel = await manage(user, "sitb");
    expect(within(panel).getByRole("heading", { level: 3, name: "Settings for sitb" })).toBeInTheDocument();
    expect(await within(panel).findByText("sit7")).toBeInTheDocument();
    expect(await within(panel).findByRole("switch", { name: "Auto-notify for sitb" })).toBeChecked();
  });

  it("asks the hub for the settings of one swimlane only, not of every row", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    await manage(user, "sitb");
    await screen.findByRole("switch", { name: "Auto-notify for sitb" });
    const settingCalls = app.calls.filter((c) => /\/swimlanes\/[^/]+\/(tenants|auto-notify)/.test(c.path));
    expect(new Set(settingCalls.map((c) => c.path.split("/")[5]))).toEqual(new Set(["sitb"]));
  });
});

describe("auto-notify", () => {
  it("is the one setting shown at once: the switch moves before the hub answers, then the hub's answer is read", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    world.overrides.push({
      match: (c) => c.method === "PUT" && c.path.endsWith("/auto-notify"),
      respond: async () => {
        await gate;
        world.autoNotify["sitb"] = false;
        return json({ enabled: false });
      },
      once: true,
    });
    const app = await open(world);
    const panel = await manage(user, "sitb");
    const toggle = await within(panel).findByRole("switch", { name: "Auto-notify for sitb" });
    await user.click(toggle);
    await waitFor(() => expect(toggle).not.toBeChecked());
    const put = writes(app.calls)[0];
    expect(put?.path).toBe("/api/v1/admin/swimlanes/sitb/auto-notify");
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ enabled: false });
    expect(put?.headers.get("X-CSRF-Token")).toBeTruthy();
    expect(put?.headers.get("Idempotency-Key")).toBeTruthy();
    const before = app.calls.filter((c) => c.method === "GET" && c.path.endsWith("/auto-notify")).length;
    release();
    await waitFor(() => expect(app.calls.filter((c) => c.method === "GET" && c.path.endsWith("/auto-notify")).length).toBeGreaterThan(before));
    expect(toggle).not.toBeChecked();
  });

  it("goes back to what the hub holds, says why, and reads the setting again when the hub refuses", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT" && c.path.endsWith("/auto-notify"), respond: () => problem(403, "insufficient-role"), once: true });
    const app = await open(world);
    const panel = await manage(user, "sitb");
    const toggle = await within(panel).findByRole("switch", { name: "Auto-notify for sitb" });
    const before = app.calls.filter((c) => c.method === "GET" && c.path.endsWith("/auto-notify")).length;
    await user.click(toggle);
    expect(await within(panel).findByText(/role may have changed/i)).toBeInTheDocument();
    await waitFor(() => expect(toggle).toBeChecked());
    await waitFor(() => expect(app.calls.filter((c) => c.method === "GET" && c.path.endsWith("/auto-notify")).length).toBeGreaterThan(before));
  });

  it("turns it on for a swimlane where it is off, with no dialog", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    const panel = await manage(user, "sit");
    const toggle = await within(panel).findByRole("switch", { name: "Auto-notify for sit" });
    expect(toggle).not.toBeChecked();
    await user.click(toggle);
    await waitFor(() => expect(JSON.parse(writes(app.calls)[0]?.body ?? "{}")).toEqual({ enabled: true }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await waitFor(() => expect(toggle).toBeChecked());
  });
});

describe("tenants", () => {
  it("shows the set, and changes it after a dialog that says what that does", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const app = await open(world);
    const panel = await manage(user, "sitb");
    await within(panel).findByText("sit7");
    await user.click(within(panel).getByRole("button", { name: "Edit tenants of sitb" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Change the tenants of sitb" });
    expect(within(dialog).getByText(/which tenant branches this swimlane reads/i)).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Add a tenant"), "sit8");
    await user.click(within(dialog).getByRole("button", { name: "Add tenant" }));
    expect(within(dialog).getByText("sit8")).toBeInTheDocument();
    expect(writes(app.calls)).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Save tenants" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writes(app.calls)[0];
    expect(put?.path).toBe("/api/v1/admin/swimlanes/sitb/tenants");
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ tenants: ["sit7", "sit8"] });
    expect(await within(panel).findByText("sit8")).toBeInTheDocument();
    expect(world.tenants["sitb"]).toEqual(["sit7", "sit8"]);
  });

  it("adds a tenant with Enter without sending the dialog, and takes one away", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    const panel = await manage(user, "sitb");
    await within(panel).findByText("sit7");
    await user.click(within(panel).getByRole("button", { name: "Edit tenants of sitb" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Change the tenants of sitb" });
    await user.type(within(dialog).getByLabelText("Add a tenant"), "sit9{Enter}");
    expect(within(dialog).getByText("sit9")).toBeInTheDocument();
    expect(writes(app.calls)).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Remove sit9" }));
    expect(within(dialog).queryByText("sit9")).toBeNull();
  });

  it("refuses a tenant that is not a branch name, a tenant twice, and an empty set, saying so next to the field", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    const panel = await manage(user, "sitb");
    await within(panel).findByText("sit7");
    await user.click(within(panel).getByRole("button", { name: "Edit tenants of sitb" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Change the tenants of sitb" });
    const field = within(dialog).getByLabelText("Add a tenant");
    await user.type(field, "SIT 8");
    await user.click(within(dialog).getByRole("button", { name: "Add tenant" }));
    expect(await within(dialog).findByText(/lower case letters, digits/i)).toBeInTheDocument();
    await user.clear(field);
    await user.type(field, "sit7");
    await user.click(within(dialog).getByRole("button", { name: "Add tenant" }));
    expect(await within(dialog).findByText("sit7 is already in the list")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Remove sit7" }));
    await user.click(within(dialog).getByRole("button", { name: "Save tenants" }));
    expect(await within(dialog).findByText("A swimlane needs at least one tenant")).toBeInTheDocument();
    expect(writes(app.calls)).toHaveLength(0);
  });

  it("keeps the dialog and the choice when the hub does not accept the set (422)", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PUT" && c.path.endsWith("/tenants"), respond: () => problem(422, "unprocessable"), once: true });
    await open(world);
    const panel = await manage(user, "sitb");
    await within(panel).findByText("sit7");
    await user.click(within(panel).getByRole("button", { name: "Edit tenants of sitb" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Change the tenants of sitb" });
    await user.type(within(dialog).getByLabelText("Add a tenant"), "sit8{Enter}");
    await user.click(within(dialog).getByRole("button", { name: "Save tenants" }));
    expect(await within(dialog).findByText(/did not accept these values/i)).toBeInTheDocument();
    expect(within(dialog).getByText("sit8")).toBeInTheDocument();
  });
});

describe("the agent of a swimlane", () => {
  it("shows the service account it runs as and changes it after a dialog that names the grant needed", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    const panel = await manage(user, "sitb");
    const field = await within(panel).findByLabelText("Service account");
    expect(field).toHaveValue("lanekeeper-agent@dfb-nonprod-sit.iam.gserviceaccount.com");
    await user.clear(field);
    await user.type(field, "agent-two@dfb-nonprod-sit.iam.gserviceaccount.com");
    await user.click(within(panel).getByRole("button", { name: "Save service account" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Change the agent of sitb" });
    expect(within(dialog).getByText(/roles\/iam.workloadIdentityUser on that service account/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Save service account" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const put = writes(app.calls)[0];
    expect(put?.path).toBe("/api/v1/admin/agents/sitb");
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ service_account_email: "agent-two@dfb-nonprod-sit.iam.gserviceaccount.com" });
    expect(await within(await agentsTable()).findByText("agent-two@dfb-nonprod-sit.iam.gserviceaccount.com")).toBeInTheDocument();
  });

  it("refuses an address that is not a Google service account, before it sends", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    const panel = await manage(user, "sitb");
    const field = await within(panel).findByLabelText("Service account");
    await user.clear(field);
    await user.type(field, "agent@example.com");
    await user.click(within(panel).getByRole("button", { name: "Save service account" }));
    expect(await within(panel).findByText("Must look like name@project.iam.gserviceaccount.com")).toBeInTheDocument();
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(writes(app.calls)).toHaveLength(0);
  });

  it("registers an agent for a swimlane that has none", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.agents = world.agents.filter((a) => a.swimlane !== "sitc");
    const app = await open(world);
    const panel = await manage(user, "sitc");
    expect(await within(panel).findByText(/No agent is registered for sitc/)).toBeInTheDocument();
    await user.type(within(panel).getByLabelText("Service account"), "lanekeeper-agent@dfb-nonprod-sit.iam.gserviceaccount.com");
    await user.click(within(panel).getByRole("button", { name: "Register agent" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Register the agent of sitc" });
    await user.click(within(dialog).getByRole("button", { name: "Register agent" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writes(app.calls)[0]?.path).toBe("/api/v1/admin/agents/sitc");
    expect(await within(panel).findByRole("button", { name: "Remove registration of sitc" })).toBeInTheDocument();
  });

  it("removes a registration only after the swimlane's name is typed", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    const panel = await manage(user, "sitb");
    await user.click(await within(panel).findByRole("button", { name: "Remove registration of sitb" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove the agent registration of sitb" });
    expect(within(dialog).getByText(/can no longer connect to the hub until a service account is registered again/)).toBeInTheDocument();
    const remove = within(dialog).getByRole("button", { name: "Remove registration" });
    expect(remove).toHaveAttribute("aria-disabled", "true");
    await user.click(remove);
    expect(writes(app.calls)).toHaveLength(0);
    await user.type(within(dialog).getByLabelText(/to confirm/), "sitb");
    await user.click(remove);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(writes(app.calls)[0]).toMatchObject({ method: "DELETE", path: "/api/v1/admin/agents/sitb" });
    expect(await screen.findByText("5 registered · 1 offline · 1 never seen")).toBeInTheDocument();
  });
});

describe("the fallback join token", () => {
  const TOKEN = "lkjt_7f3a9c21e8b04d5a96c1f2e07b38d4a5";
  const held = (app: ReturnType<typeof renderApp>): string =>
    JSON.stringify([
      app.queryClient.getQueryCache().getAll().map((q) => [q.queryKey, q.state.data, q.state.error]),
      app.queryClient.getMutationCache().getAll().map((m) => [m.options.mutationKey, m.state.variables, m.state.data, m.state.error]),
    ]);

  it("is shown once, in a field that selects itself, with a copy button, and the answer is kept out of every cache", async () => {
    const user = userEvent.setup();
    const logged: unknown[][] = [];
    for (const method of ["log", "info", "warn", "error", "debug"] as const) vi.spyOn(console, method).mockImplementation((...args: unknown[]) => void logged.push(args));
    const app = await open(newWorld());
    const panel = await manage(user, "sitb");
    await user.click(await within(panel).findByRole("button", { name: "Create join token" }));
    const field = await within(panel).findByLabelText("Your join token");
    expect(field).toHaveValue(TOKEN);
    expect(field).toHaveAttribute("readonly");
    expect(within(panel).getByText("Join token created · shown once")).toBeInTheDocument();
    expect(within(panel).getByText(/works for one agent in/)).toBeInTheDocument();
    expect(within(panel).getByText(/expires 2025-10-10 08:53 UTC/)).toBeInTheDocument();
    const post = writes(app.calls)[0];
    expect(post).toMatchObject({ method: "POST", path: "/api/v1/admin/agents/sitb/join-token" });
    expect(post?.headers.get("Idempotency-Key")).toBeTruthy();
    expect(held(app)).not.toContain(TOKEN);
    expect(JSON.stringify(logged)).not.toContain(TOKEN);
    // The human's decision of 2026-10-17: a one-time secret has a copy button. Nothing was copied until the click.
    const copy = within(panel).getByRole("button", { name: "Copy join token" });
    expect(await navigator.clipboard.readText()).toBe("");
    await user.click(copy);
    expect(await navigator.clipboard.readText()).toBe(TOKEN);
    expect(panel).toHaveTextContent("Copied. It will not be shown again.");
    expect(held(app)).not.toContain(TOKEN);
    expect(JSON.stringify(logged)).not.toContain(TOKEN);
    expect(toasts.getSnapshot()).toEqual([]);
    await user.click(within(panel).getByRole("button", { name: "I have stored it" }));
    await waitFor(() => expect(screen.queryByDisplayValue(TOKEN)).toBeNull());
    expect(within(panel).queryByRole("button", { name: /^Copy join token|^Copied/ })).toBeNull();
    expect(document.body.innerHTML).not.toContain(TOKEN);
    expect(panel).not.toHaveTextContent("will not be shown again");
    expect(held(app)).not.toContain(TOKEN);
  });

  it("is gone when another swimlane is chosen, and when the page is left", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    const panel = await manage(user, "sitb");
    await user.click(await within(panel).findByRole("button", { name: "Create join token" }));
    await screen.findByDisplayValue(TOKEN);
    await user.click(screen.getByRole("button", { name: "Manage sit" }));
    await waitFor(() => expect(screen.queryByDisplayValue(TOKEN)).toBeNull());
    const again = await screen.findByRole("region", { name: "Settings for sit" });
    await user.click(await within(again).findByRole("button", { name: "Create join token" }));
    await screen.findByDisplayValue(TOKEN);
    app.unmount();
    expect(document.body.innerHTML).not.toContain(TOKEN);
    expect(held(app)).not.toContain(TOKEN);
  });

  it("says so when the hub refuses, and shows no token", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.path.endsWith("/join-token"), respond: () => problem(403, "insufficient-role"), once: true });
    const app = await open(world);
    const panel = await manage(user, "sitb");
    await user.click(await within(panel).findByRole("button", { name: "Create join token" }));
    expect(await within(panel).findByText(/role may have changed/i)).toBeInTheDocument();
    expect(within(panel).queryByLabelText("Your join token")).toBeNull();
    expect(held(app)).not.toContain(TOKEN);
  });
});

describe("accessibility", () => {
  it("has no structural violations with the lists and one swimlane's settings open", async () => {
    const user = userEvent.setup();
    await open(newWorld());
    const panel = await manage(user, "sitb");
    await within(panel).findByRole("switch", { name: "Auto-notify for sitb" });
    await within(panel).findByText("sit7");
    await expectNoAxeViolations();
  });
});
