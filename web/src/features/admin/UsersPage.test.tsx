import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Role } from "@/api/generated/model";
import { IDS, NOW, person, samRequest } from "@/mocks/design/admin/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/admin/world";
import { json, problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Users and access requests (route /admin/users, Admin only): who has signed in and waits for a role, who the people are, and the
// four things an Admin does to them: approve or reject a request, change a role, switch "requires approval", enable or disable.
// Nothing is shown as done until the hub answers (these writes are not optimistic), and a write names its consequence first.

const FIRST = { timeout: 8000 };
const posts = <T extends { method: string }>(calls: readonly T[]): T[] => calls.filter((c) => c.method !== "GET");

// The clock the design's relative times ("2 h ago") are drawn against; only `Date` is faked, so timers and promises run for real.
beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
});
afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(document, "cookie");
});

async function open(world: World, url = "/admin/users") {
  const app = renderApp(url, worldApi(world));
  await screen.findByRole("heading", { level: 1, name: "Admin" }, FIRST);
  return app;
}
const usersTable = () => screen.findByRole("grid", { name: "Users" });
async function rowOf(name: string): Promise<HTMLElement> {
  const grid = await usersTable();
  const cell = await within(grid).findByText(name);
  const row = cell.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no row for ${name}`);
  return row;
}
const holdUntil = () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { gate, release };
};

describe("the Admin area", () => {
  it("is one h1 over the section's own heading and the area's navigation, with the current section marked", async () => {
    await open(newWorld());
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(await screen.findByRole("heading", { level: 2, name: "Users and access requests" })).toBeInTheDocument();
    const nav = screen.getByRole("navigation", { name: "Admin sections" });
    for (const group of ["Access", "Infrastructure", "Mapping and rules", "Data and alerts"]) expect(within(nav).getByText(group)).toBeInTheDocument();
    expect(within(nav).getByRole("link", { name: /^Users and access requests/ })).toHaveAttribute("aria-current", "page");
    expect(within(nav).getByRole("link", { name: "Swimlanes and agents" })).toHaveAttribute("href", "/admin/swimlanes");
    expect(within(nav).getByRole("link", { name: "Path mappings" })).toHaveAttribute("href", "/admin/paths");
    expect(screen.getByText("Admin only")).toBeInTheDocument();
    expect(screen.getByText("Every change on these pages is written to the audit log.")).toBeInTheDocument();
  });

  it("counts the waiting requests beside Users and access requests, from the same request as the list", async () => {
    const app = await open(newWorld());
    const nav = screen.getByRole("navigation", { name: "Admin sections" });
    expect(await within(nav).findByLabelText("1 request waiting")).toHaveTextContent("1");
    await screen.findByRole("group", { name: "Access request from sam.okafor@corp.example" });
    expect(app.calls.filter((c) => c.path.startsWith("/api/v1/admin/access-requests"))).toHaveLength(1);
  });
});

describe("the people", () => {
  it("lists them with role, approval, status, last seen and GitHub login, and says who is not linked", async () => {
    await open(newWorld());
    expect(await screen.findByText("7 users")).toBeInTheDocument();
    const dana = await rowOf("Dana Okoye");
    expect(within(dana).getByText("dana.okoye@corp.example")).toBeInTheDocument();
    expect(within(dana).getByLabelText("Role for Dana Okoye")).toHaveValue("editor");
    expect(within(dana).getByRole("switch", { name: "Requires approval for Dana Okoye" })).toBeChecked();
    expect(within(dana).getByText("Active")).toBeInTheDocument();
    expect(within(dana).getByText("@dokoye")).toBeInTheDocument();
    const tomas = await rowOf("Tomás Reyes");
    expect(within(tomas).getByText("Not linked")).toBeInTheDocument();
    expect(within(tomas).queryByRole("switch")).toBeNull();
    expect(within(tomas).getByText("Not needed")).toBeInTheDocument();
    const ines = await rowOf("Ines Costa");
    expect(within(ines).getByText("Disabled")).toBeInTheDocument();
    expect(within(ines).getByRole("button", { name: "Enable Ines Costa" })).toBeInTheDocument();
  });

  it("shows a name with an invisible or direction-changing character with the character made visible (SafeText)", async () => {
    const world = newWorld();
    world.users = [...world.users, person(IDS.sam, "Eve‮evil")];
    await open(world);
    // The name and the address made from it both carry the character, and both show it.
    expect((await screen.findAllByRole("img", { name: /U\+202E/ })).length).toBeGreaterThanOrEqual(1);
  });

  it("marks your own row: your role cannot be changed, and you cannot disable yourself", async () => {
    await open(newWorld());
    const you = await rowOf("Priya Shah");
    expect(within(you).getByText("You")).toBeInTheDocument();
    expect(within(you).queryByLabelText("Role for Priya Shah")).toBeNull();
    expect(within(you).getByText(/you can't change your own role/i)).toBeInTheDocument();
    const disable = within(you).getByRole("button", { name: "Disable Priya Shah" });
    expect(disable).toHaveAttribute("aria-disabled", "true");
    expect(disable).toHaveAccessibleDescription("You can't disable your own account");
  });

  it("states the three safeguards and how many active admins there are", async () => {
    await open(newWorld());
    expect(await screen.findByText(/At least one active admin is required/)).toHaveTextContent("2 active admins");
    expect(screen.getByText(/Editors with .Requires approval. on need an Admin to approve each change/)).toBeInTheDocument();
  });
});

describe("changing a role", () => {
  it("names the consequence first, sends only the role once confirmed, and shows the hub's answer", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const app = await open(world);
    await user.selectOptions((await within(await rowOf("Lena Brandt")).findByLabelText("Role for Lena Brandt")), "admin");
    const dialog = await screen.findByRole("alertdialog", { name: "Change Lena Brandt's role" });
    expect(within(dialog).getByText(/changes from Editor to Admin/)).toBeInTheDocument();
    expect(within(dialog).getByText(/Admin can decide proposals, change roles and edit every swimlane/)).toBeInTheDocument();
    expect(posts(app.calls)).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Change role" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const patch = posts(app.calls)[0];
    expect(patch?.method).toBe("PATCH");
    expect(patch?.path).toBe(`/api/v1/admin/users/${IDS.lena}`);
    expect(JSON.parse(patch?.body ?? "{}")).toEqual({ role: "admin" });
    expect(patch?.headers.get("X-CSRF-Token")).toBeTruthy();
    expect(patch?.headers.get("Idempotency-Key")).toBeTruthy();
    await waitFor(async () => {
      expect(within(await rowOf("Lena Brandt")).getByLabelText("Role for Lena Brandt")).toHaveValue("admin");
    });
  });

  it("does not show the new role while the hub has not answered, and locks the dialog while it works", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const { gate, release } = holdUntil();
    world.overrides.push({ match: (c) => c.method === "PATCH", respond: async () => (await gate, json({ ...world.users[2], role: "operator" })), once: true });
    await open(world);
    await user.selectOptions((await within(await rowOf("Lena Brandt")).findByLabelText("Role for Lena Brandt")), "operator");
    const dialog = await screen.findByRole("alertdialog", { name: "Change Lena Brandt's role" });
    await user.click(within(dialog).getByRole("button", { name: "Change role" }));
    expect(await within(dialog).findByRole("button", { name: "Changing role…" })).toHaveAttribute("aria-busy", "true");
    // The page behind the dialog is out of the accessibility tree, so it is asked for by its label.
    expect(screen.getByLabelText("Role for Lena Brandt")).toHaveValue("editor");
    release();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("explains a 409 as the person having changed while you were editing, and offers to reload them (state C03)", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PATCH", respond: () => problem(409, "conflict"), once: true });
    const app = await open(world);
    await user.selectOptions((await within(await rowOf("Lena Brandt")).findByLabelText("Role for Lena Brandt")), "admin");
    const dialog = await screen.findByRole("alertdialog", { name: "Change Lena Brandt's role" });
    const listCalls = () => app.calls.filter((c) => c.path.startsWith("/api/v1/admin/users") && c.method === "GET").length;
    const before = listCalls();
    await user.click(within(dialog).getByRole("button", { name: "Change role" }));
    expect(await within(dialog).findByText("This user changed while you were editing")).toBeInTheDocument();
    expect(within(dialog).getByText(/Nothing was applied/)).toBeInTheDocument();
    expect(within(dialog).getByText("req-test-1")).toBeInTheDocument();
    // The people are asked for again: what the hub holds now is what the Admin must see before changing them again.
    await waitFor(() => expect(listCalls()).toBeGreaterThan(before));
    await user.click(within(dialog).getByRole("button", { name: "Reload user" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("says the hub refused (403) in the dialog and keeps the dialog open with the choice intact", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({ match: (c) => c.method === "PATCH", respond: () => problem(403, "insufficient-role"), once: true });
    await open(world);
    await user.selectOptions((await within(await rowOf("Lena Brandt")).findByLabelText("Role for Lena Brandt")), "viewer");
    const dialog = await screen.findByRole("alertdialog", { name: "Change Lena Brandt's role" });
    await user.click(within(dialog).getByRole("button", { name: "Change role" }));
    expect(await within(dialog).findByText(/role may have changed/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/changes from Editor to Viewer/)).toBeInTheDocument();
  });

  it("refuses, before it sends, to take the Admin role from the last active admin", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    // The session is an Admin whose own row says otherwise, so Amir is the only active admin the list shows.
    world.users = world.users.map((u) => (u.id === IDS.priya ? { ...u, role: "editor" satisfies Role } : u));
    const app = await open(world);
    await user.selectOptions(await within(await rowOf("Amir Haddad")).findByLabelText("Role for Amir Haddad"), "editor");
    expect(await screen.findByText("At least one active admin is required")).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(posts(app.calls)).toHaveLength(0);
  });
});

describe("enabling, disabling and requiring approval", () => {
  it("disables a person after a dialog says what it does, and then offers to enable them", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const app = await open(world);
    await user.click(within(await rowOf("Lena Brandt")).getByRole("button", { name: "Disable Lena Brandt" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Disable Lena Brandt" });
    expect(within(dialog).getByText(/can no longer sign in or use Lanekeeper until an Admin enables them again/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Disable user" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(JSON.parse(posts(app.calls)[0]?.body ?? "{}")).toEqual({ status: "disabled" });
    expect(await within(await rowOf("Lena Brandt")).findByRole("button", { name: "Enable Lena Brandt" })).toBeInTheDocument();
  });

  it("enables a disabled person at once, with no dialog", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    await user.click(within(await rowOf("Ines Costa")).getByRole("button", { name: "Enable Ines Costa" }));
    await waitFor(() => expect(JSON.parse(posts(app.calls)[0]?.body ?? "{}")).toEqual({ status: "active" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("turns Requires approval on at once and asks first to turn it off", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    await user.click(within(await rowOf("Lena Brandt")).getByRole("switch", { name: "Requires approval for Lena Brandt" }));
    await waitFor(() => expect(JSON.parse(posts(app.calls)[0]?.body ?? "{}")).toEqual({ requires_approval: true }));
    await user.click(await within(await rowOf("Dana Okoye")).findByRole("switch", { name: "Requires approval for Dana Okoye" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Stop requiring approval for Dana Okoye" });
    expect(within(dialog).getByText(/changes take effect without an Admin's approval/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Stop requiring approval" }));
    await waitFor(() => expect(posts(app.calls).some((c) => c.body === JSON.stringify({ requires_approval: false }))).toBe(true));
  });
});

describe("the columns", () => {
  it("gives Status the room of a badge and a button with their words 40% longer (216px), so that Enable and Disable are not cut off (the layout run, German-length labels)", async () => {
    await open(newWorld());
    const grid = await usersTable();
    const template = grid.querySelector<HTMLElement>("[data-slot='virtual-list']")?.style.getPropertyValue("--lk-cols") ?? grid.style.getPropertyValue("--lk-cols");
    const tracks = template.split(/\s+(?![^(]*\))/);
    const headers = within(grid)
      .getAllByRole("columnheader")
      .map((h) => (h.textContent ?? "").trim());
    expect(Number.parseInt(tracks[headers.indexOf("Status")] ?? "0", 10)).toBeGreaterThanOrEqual(216);
  });
});

describe("access requests", () => {
  it("shows who asked, for which role and why, with the role chosen as asked", async () => {
    await open(newWorld());
    const card = await screen.findByRole("group", { name: "Access request from sam.okafor@corp.example" });
    expect(within(card).getByText(/Asked for Editor/)).toBeInTheDocument();
    expect(within(card).getByText(/requested 2 h ago/)).toBeInTheDocument();
    expect(within(card).getByText(/Joining the sit swimlane team/)).toBeInTheDocument();
    expect(within(card).getByLabelText("Approve sam.okafor@corp.example as")).toHaveValue("editor");
    expect(screen.getByText("1 waiting")).toBeInTheDocument();
  });

  it("puts the decision below the request, in a row that wraps, when the card is short of room (at 1024 the Admin menu leaves it 490px, and a two-column grid gave the request 57px, so that its text ran under the buttons)", async () => {
    await open(newWorld());
    const card = await screen.findByRole("group", { name: "Access request from sam.okafor@corp.example" });
    expect(card.className).toMatch(/(^|\s)flex(\s|$)/);
    expect(card.className).toMatch(/(^|\s)flex-wrap(\s|$)/);
    expect(card.className).not.toMatch(/grid-cols-/);
    const [who, decision] = Array.from(card.children) as HTMLElement[];
    // The request keeps room to read (a basis it wraps below), grows into spare room, and does not shrink under the decision.
    expect(who?.className).toMatch(/(^|\s)basis-\[\d+px\](\s|$)/);
    expect(who?.className).toMatch(/(^|\s)flex-1(\s|$)/);
    expect(decision?.className).toMatch(/(^|\s)flex-none(\s|$)/);
  });

  it("approves after a dialog that names the role's consequence, then shows the person in the table and the request gone", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    await user.click(await screen.findByRole("button", { name: "Approve request from sam.okafor@corp.example" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Approve sam.okafor@corp.example as Editor" });
    expect(within(dialog).getByText(/can edit config files/i)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Approve request" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const post = posts(app.calls)[0];
    expect(post?.path).toBe("/api/v1/admin/access-requests/41/approve");
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ role: "editor" });
    expect(await screen.findByText("sam.okafor@corp.example approved as Editor")).toBeInTheDocument();
    expect(await screen.findByText("No pending requests")).toBeInTheDocument();
    expect(await within(await usersTable()).findByText("Sam Okafor")).toBeInTheDocument();
  });

  it("names the Admin role's consequence when the Admin role is chosen", async () => {
    const user = userEvent.setup();
    await open(newWorld());
    await user.selectOptions(await screen.findByLabelText("Approve sam.okafor@corp.example as"), "admin");
    await user.click(screen.getByRole("button", { name: "Approve request from sam.okafor@corp.example" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Approve sam.okafor@corp.example as Admin" });
    expect(within(dialog).getByText(/Admin can decide proposals, change roles and edit every swimlane/)).toBeInTheDocument();
  });

  it("will not approve without a role, and says so next to the choice", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.requests = [samRequest({ requested_role: null })];
    const app = await open(world);
    const select = await screen.findByLabelText("Approve sam.okafor@corp.example as");
    expect(select).toHaveValue("");
    await user.click(screen.getByRole("button", { name: "Approve request from sam.okafor@corp.example" }));
    expect(await screen.findByText("Choose a role before approving")).toBeInTheDocument();
    expect(select).toHaveAttribute("aria-invalid", "true");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(posts(app.calls)).toHaveLength(0);
  });

  it("rejects with a reason the person is told nothing of, and shows the outcome", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    await user.click(await screen.findByRole("button", { name: "Reject request from sam.okafor@corp.example" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Reject sam.okafor@corp.example's request" });
    expect(within(dialog).getByText(/stay signed out and can request access again/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Reject request" }));
    expect(await within(dialog).findByText("Give a reason")).toBeInTheDocument();
    expect(posts(app.calls)).toHaveLength(0);
    await user.type(within(dialog).getByLabelText("Reason"), "Not on the sit team");
    await user.click(within(dialog).getByRole("button", { name: "Reject request" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const post = posts(app.calls)[0];
    expect(post?.path).toBe("/api/v1/admin/access-requests/41/reject");
    expect(JSON.parse(post?.body ?? "{}")).toEqual({ reason: "Not on the sit team" });
    expect(await screen.findByText("Request from sam.okafor@corp.example rejected")).toBeInTheDocument();
  });

  it("says a request that was decided meanwhile is decided (409), and refreshes the list", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    world.overrides.push({
      match: (c) => c.method === "POST",
      respond: () => {
        world.requests = world.requests.map((r) => ({ ...r, status: "approved" as const }));
        return problem(409, "conflict");
      },
      once: true,
    });
    await open(world);
    await user.click(await screen.findByRole("button", { name: "Approve request from sam.okafor@corp.example" }));
    const dialog = await screen.findByRole("alertdialog", { name: /^Approve sam.okafor@corp.example as/ });
    await user.click(within(dialog).getByRole("button", { name: "Approve request" }));
    expect(await within(dialog).findByText(/This request was already decided/)).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("group", { name: "Access request from sam.okafor@corp.example" })).toBeNull());
  });

  it("is an empty state when nobody waits, saying where new sign-ins appear", async () => {
    const world = newWorld();
    world.requests = [];
    await open(world);
    expect(await screen.findByText("No pending requests")).toBeInTheDocument();
    expect(screen.getByText(/New sign-ins that need approval appear here/)).toBeInTheDocument();
    expect(screen.getByText("None waiting")).toBeInTheDocument();
  });
});

describe("loading, errors and the filter", () => {
  it("shows a skeleton for the people while they load, under a heading that is already there", async () => {
    const world = newWorld();
    world.hold.add("/api/v1/admin/users");
    await open(world);
    expect(await screen.findByLabelText("Loading users")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("heading", { level: 1, name: "Admin" })).toBeInTheDocument();
  });

  it("says the people could not be loaded, with the request id and a way to retry, while the requests still show", async () => {
    const world = newWorld();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/users"), respond: () => problem(500, "internal"), once: true });
    await open(world);
    expect(await screen.findByText("Couldn't load users")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(await screen.findByRole("group", { name: "Access request from sam.okafor@corp.example" })).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry" }));
    expect(await within(await usersTable()).findByText("Dana Okoye")).toBeInTheDocument();
  });

  it("asks the hub for the people matching a search, and keeps the search in the address", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    await within(await usersTable()).findByText("Dana Okoye");
    await user.type(screen.getByRole("searchbox", { name: "Search users" }), "okoye");
    await waitFor(() => expect(app.calls.some((c) => c.path.includes("/api/v1/admin/users") && c.path.includes("q=okoye"))).toBe(true), { timeout: 3000 });
    await waitFor(() => expect(app.location()).toBe("/admin/users?q=okoye"));
    await waitFor(async () => {
      const grid = await usersTable();
      expect(within(grid).queryByText("Lena Brandt")).toBeNull();
      expect(within(grid).getByText("Dana Okoye")).toBeInTheDocument();
    });
  });

  it("starts from the role in the address, and filters among the people loaded without asking the hub for a role", async () => {
    const app = await open(newWorld(), "/admin/users?role=viewer");
    await waitFor(async () => {
      const grid = await usersTable();
      expect(within(grid).getByText("Tomás Reyes")).toBeInTheDocument();
      expect(within(grid).queryByText("Dana Okoye")).toBeNull();
    });
    expect(screen.getByRole("combobox", { name: "Role" })).toHaveValue("viewer");
    expect(app.calls.some((c) => c.path.includes("role="))).toBe(false);
  });
});

describe("many people", () => {
  it("asks for a hundred at a time, says there are more, and loads the next hundred on request", async () => {
    const user = userEvent.setup();
    const world = newWorld();
    const everyone = world.users;
    world.overrides.push(
      {
        match: (c) => c.method === "GET" && c.path.startsWith("/api/v1/admin/users") && !c.path.includes("cursor="),
        respond: () => json({ items: everyone.slice(0, 4), next: "c1" }),
      },
      { match: (c) => c.method === "GET" && c.path.includes("cursor=c1"), respond: () => json({ items: everyone.slice(4), next: null }) },
    );
    const app = await open(world);
    const grid = await usersTable();
    await within(grid).findByText("Dana Okoye");
    expect(await screen.findByText("4+ users")).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.includes("/api/v1/admin/users") && c.path.includes("limit=100"))).toBe(true);
    // The last active admin cannot be judged from part of the list, so the page does not say it did.
    expect(screen.getByText(/2\+ active admins/)).toBeInTheDocument();
    expect(within(grid).queryByText("Ines Costa")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Load 100 more" }));
    expect(await within(await usersTable()).findByText("Ines Costa")).toBeInTheDocument();
    expect(await screen.findByText("7 users")).toBeInTheDocument();
  });
});

describe("keyboard and focus", () => {
  it("closes a dialog with Escape without sending anything, and gives focus back to the control that opened it", async () => {
    const user = userEvent.setup();
    const app = await open(newWorld());
    const select = await within(await rowOf("Lena Brandt")).findByLabelText("Role for Lena Brandt");
    await user.selectOptions(select, "admin");
    await screen.findByRole("alertdialog", { name: "Change Lena Brandt's role" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(posts(app.calls)).toHaveLength(0);
    await waitFor(() => expect(screen.getByLabelText("Role for Lena Brandt")).toHaveFocus());
    expect(screen.getByLabelText("Role for Lena Brandt")).toHaveValue("editor");
  });

  it("gives focus back to the Disable button after the dialog is cancelled", async () => {
    const user = userEvent.setup();
    await open(newWorld());
    const button = within(await rowOf("Lena Brandt")).getByRole("button", { name: "Disable Lena Brandt" });
    await user.click(button);
    const dialog = await screen.findByRole("alertdialog", { name: "Disable Lena Brandt" });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Disable Lena Brandt" })).toHaveFocus());
  });
});

describe("who may open it", () => {
  it.each(["viewer", "editor", "operator"] as const)("shows the permission state to a %s and asks the hub for nothing of the Admin area", async (role) => {
    const app = renderApp("/admin/users", worldApi(newWorld(role)));
    expect(await screen.findByRole("heading", { level: 1, name: "Admin needs the Admin role" }, FIRST)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin"))).toBe(false);
    expect(screen.queryByRole("navigation", { name: "Admin sections" })).toBeNull();
  });
});

describe("accessibility", () => {
  it("has no structural violations with the people and a request on screen", async () => {
    await open(newWorld());
    await within(await usersTable()).findByText("Dana Okoye");
    await screen.findByRole("group", { name: "Access request from sam.okafor@corp.example" });
    await expectNoAxeViolations();
  });
});
