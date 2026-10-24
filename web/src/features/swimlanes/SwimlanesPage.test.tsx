import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Role, SwimlaneSummary } from "@/api/generated/model";
import { prefs } from "@/lib/prefs";
import { lane } from "@/mocks/design/swimlanes/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/swimlanes/world";
import { json, problem, renderApp, type Api } from "@/test/app";

// The swimlane list as a person meets it: the real router, session and shell, and a fake hub that answers for a world the test
// sets up. What each cell says, the filters, the selection and Compare, sorting, the column chooser, every state of the States
// index for this screen, and the keys. The live behaviour is in SwimlanesPage.live.test.tsx.

const NOW = Date.now();
const MIN = 60_000;

const LANES: SwimlaneSummary[] = [
  lane("sit", { tenants: ["sit1"], last_scan_at: NOW - 2 * 60 * MIN }),
  lane("sitb", {
    tenants: ["sit7", "sit8", "sit9"],
    drift_counts: [["conflict", 1], ["nfs_ahead", 2], ["git_ahead", 1]],
    pickup_counts: [["needs_notify_or_restart", 2]],
    finding_count: 2,
    last_scan_at: NOW - 5_000,
    base_version: { tag: "v2.3.1", distance: 4, commit: "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c" },
  }),
  lane("presitb", { tier: "PRESIT", project: "ops-presit", tenants: ["sit12"], agent: "disconnected", drift_counts: [["nfs_ahead", 3]], finding_count: 2, last_scan_at: NOW - 14 * MIN }),
  lane("presitc", { tier: "PRESIT", project: "ops-presit", tenants: [], drift_counts: [["unknown", 412]], baseline: { state: "unconfirmed", by: null, at: null }, last_scan_at: NOW - 6 * 24 * 60 * MIN }),
];

let world: World;
beforeEach(() => {
  prefs.clear();
  world = newWorld("admin", LANES.map((l) => ({ ...l })));
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const open = (role: Role = "admin", url = "/swimlanes") => {
  world.role = role;
  return renderApp(url, worldApi(world));
};
const rowOf = async (name: string): Promise<HTMLElement> => {
  const link = await screen.findByRole("link", { name });
  const row = link.closest<HTMLElement>('[role="row"]');
  if (row === null) throw new Error(`no row for ${name}`);
  return row;
};

describe("the screen", () => {
  it("shows its title, then the facts and one row for every swimlane", async () => {
    open();
    expect(await screen.findByRole("heading", { level: 1, name: "Swimlanes" })).toBeInTheDocument();
    expect(await screen.findByText("4 swimlanes · 3 of 4 agents online · 1 needs adoption")).toBeInTheDocument();
    for (const id of ["sit", "sitb", "presitb", "presitc"]) expect(await screen.findByRole("link", { name: id })).toHaveAttribute("href", `/swimlanes/${id}`);
  });

  it("says in each cell what the design says: tier, project, tenants, version, agent, drift, pickup, findings, baseline, last scan", async () => {
    open();
    const sitb = await rowOf("sitb");
    expect(within(sitb).getByText("SIT")).toBeInTheDocument();
    expect(within(sitb).getByText("ops-sit")).toBeInTheDocument();
    expect(within(sitb).getByText("sit7")).toBeInTheDocument();
    // The branch column is a SET of tenants: the first one, and the whole set for the one who asks.
    expect(within(sitb).getByText("+2")).toBeInTheDocument();
    expect(within(sitb).getByText(/more tenants: sit7, sit8, sit9/)).toBeInTheDocument();
    expect(within(sitb).getByText("v2.3.1 + 4")).toBeInTheDocument();
    expect(within(sitb).getByText("Online")).toBeInTheDocument();
    expect(within(sitb).getByRole("img", { name: "1 Conflict" })).toBeInTheDocument();
    expect(within(sitb).getByRole("img", { name: "2 NFS ahead" })).toBeInTheDocument();
    expect(within(sitb).getByRole("img", { name: "1 Git ahead" })).toBeInTheDocument();
    expect(within(sitb).getByRole("img", { name: "2 Needs notify or restart" })).toBeInTheDocument();
    expect(within(sitb).getByText("Confirmed")).toBeInTheDocument();
    expect(within(sitb).getByText("just now")).toBeInTheDocument();
  });

  it("gives the Agent and Baseline columns the room their longest cell needs, so nothing is cut off (the layout run at 1440 and 2560 found 'Offline · 14 min ago' and the Adopt button clipped)", async () => {
    open();
    await rowOf("sitb");
    const grid = screen.getByRole("grid", { name: "Swimlanes" });
    const template = grid.querySelector<HTMLElement>("[data-slot='virtual-list']")?.style.getPropertyValue("--lk-cols") ?? grid.style.getPropertyValue("--lk-cols");
    const tracks = template.split(/\s+(?![^(]*\))/);
    const headers = within(grid)
      .getAllByRole("columnheader")
      .map((h) => (h.textContent ?? "").trim());
    const widthOf = (name: string): number => Number.parseInt(tracks[headers.indexOf(name)] ?? "0", 10);
    // "Offline · 14 min ago" with its dot is about 126px; "Adoption in progress" with its button, about 200px.
    expect(widthOf("Agent")).toBeGreaterThanOrEqual(136);
    expect(widthOf("Baseline")).toBeGreaterThanOrEqual(208);
  });

  it("lets the Adoption badge give way with an ellipsis while the Adopt button keeps its place, instead of the button being cut off by the cell (labels 40% longer at 2560 wide: found by the T6 layout run)", async () => {
    open();
    const presitc = await rowOf("presitc");
    const badge = within(presitc).getByText("Adoption needed").closest<HTMLElement>("[data-slot='status-badge']");
    if (badge === null) throw new Error("no badge");
    // A badge does not shrink unless it says so (`shrink-0` in its base), and the cell cuts what runs past it: the button, after the badge,
    // went. The badge's own label is `truncate`, so once the badge may shrink the words end in an ellipsis and the button stays.
    expect(badge.className).toMatch(/(^|\s)shrink(\s|$)/);
    expect(badge.className).not.toMatch(/(^|\s)shrink-0(\s|$)/);
    expect(badge.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(within(presitc).getByRole("link", { name: "Adopt" })).toBeInTheDocument();
  });

  it("lets the Agent cell end in an ellipsis in the room it has, instead of being cut off by the cell (labels 40% longer, at 1024 and 1280 wide)", async () => {
    open();
    const sitb = await rowOf("sitb");
    const text = within(sitb).getByText("Online");
    expect(text.className).toMatch(/(^|\s)truncate(\s|$)/);
    const box = text.parentElement;
    expect(box?.className).toMatch(/(^|\s)flex(\s|$)/);
    expect(box?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(box?.className).not.toMatch(/inline-flex/);
  });

  it("wraps the toolbar onto a second line when its words do not fit (labels 40% longer, at 1024 wide: Compare selected was cut off), instead of fixing its height at 40px", async () => {
    open();
    const field = await screen.findByRole("searchbox", { name: "Filter swimlanes" });
    const bar = field.closest("[data-slot='toolbar']");
    expect(bar?.className).toMatch(/(^|\s)flex-wrap(\s|$)/);
    expect(bar?.className).toMatch(/(^|\s)min-h-10(\s|$)/);
    expect(bar?.className).not.toMatch(/(^|\s)h-10(\s|$)/);
  });

  it("lets the filter field give way when the toolbar is narrow, so that Compare selected is not cut off at 1024 wide", async () => {
    open();
    const field = await screen.findByRole("searchbox", { name: "Filter swimlanes" });
    const box = field.closest("[data-slot='search-input']");
    expect(box?.className).not.toMatch(/(^|\s)flex-none(\s|$)/);
    expect(box?.className).toMatch(/(^|\s)shrink(\s|$)/);
    // At 1024 the bar is 750px; with two rows selected, the buttons, "2 selected" and Compare selected need 629px, and the field is
    // left with 96px (the layout run measured the bar 23px too wide with the field at 144px).
    expect(box?.className).toMatch(/(^|\s)min-w-24(\s|$)/);
  });

  it("truncates what a stale row last knew, with the whole sentence in its title, instead of cutting it off", async () => {
    open();
    const presitb = await rowOf("presitb");
    const stale = within(presitb).getByText(/Stale · last known/);
    expect(stale.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(stale).toHaveAttribute("title", expect.stringMatching(/Stale · last known/));
  });

  it("shows a swimlane that is in sync as quiet text, not a badge", async () => {
    open();
    const sit = await rowOf("sit");
    expect(within(sit).getByText("In sync")).toBeInTheDocument();
    expect(within(sit).queryByRole("img", { name: /ahead|Conflict|Unknown|Needs|Untracked|Intentional/ })).toBeNull();
    expect(within(sit).getByText("2 h ago")).toBeInTheDocument();
  });

  it("shows an agent that is offline with when it last reported, and the drift as stale", async () => {
    open();
    const presitb = await rowOf("presitb");
    expect(within(presitb).getByText(/Offline · 14 min ago/)).toBeInTheDocument();
    expect(within(presitb).getByText("Stale · last known 3 NFS ahead")).toBeInTheDocument();
    expect(within(presitb).queryByRole("img", { name: /NFS ahead/ })).toBeNull();
  });

  it("copes with a swimlane that has no tenants, which is what the registry sends for one that is not adopted", async () => {
    open();
    const presitc = await rowOf("presitc");
    expect(within(presitc).getByText("No tenants")).toBeInTheDocument();
    expect(within(presitc).getByText("unconfirmed")).toBeInTheDocument();
    expect(within(presitc).getByText("Adoption needed")).toBeInTheDocument();
    expect(within(presitc).getByRole("img", { name: "412 Unknown" })).toBeInTheDocument();
    expect(within(presitc).getByText("6 d ago")).toBeInTheDocument();
  });

  it("gives an Admin the Adopt button and the Add a swimlane link, as links to the places they lead to", async () => {
    open("admin");
    const presitc = await rowOf("presitc");
    expect(within(presitc).getByRole("link", { name: "Adopt" })).toHaveAttribute("href", "/swimlanes/presitc/adopt");
    expect(await screen.findByRole("link", { name: "Add a swimlane" })).toHaveAttribute("href", "/admin/swimlanes");
  });

  it("gives a Viewer neither: the badge says adoption is needed and nothing offers to do it", async () => {
    open("viewer");
    const presitc = await rowOf("presitc");
    expect(within(presitc).getByText("Adoption needed")).toBeInTheDocument();
    expect(within(presitc).queryByRole("link", { name: "Adopt" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Add a swimlane" })).toBeNull();
  });

  it("says an adoption in progress is in progress, and offers an Admin the way back into it", async () => {
    world.lanes = [lane("sitc", { baseline: { state: "in_progress", by: null, at: null } })];
    open("admin");
    const row = await rowOf("sitc");
    expect(within(row).getByText("Adoption in progress")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "Continue" })).toHaveAttribute("href", "/swimlanes/sitc/adopt");
  });

  it("says nothing about the baseline when the server did not state it, rather than call it confirmed", async () => {
    const { baseline: _omitted, ...bare } = lane("old");
    world.lanes = [bare];
    open("viewer");
    const row = await rowOf("old");
    expect(within(row).queryByText("Confirmed")).toBeNull();
    expect(within(row).queryByText("Adoption needed")).toBeNull();
  });

  it("does not take a swimlane's name for markup", async () => {
    world.lanes = [lane("evil", { display_name: "<img src=x onerror=alert(1)>‮gnp.exe" })];
    open("viewer");
    await screen.findByRole("link", { name: /onerror/ });
    expect(document.querySelector("img[src='x']")).toBeNull();
    expect(screen.getByRole("img", { name: /U\+202E/ })).toBeInTheDocument();
  });
});

describe("the states of the States index", () => {
  it("shows a skeleton with the table's own columns while the list loads, and names what is loading", async () => {
    let release: (r: Response) => void = () => undefined;
    const slow = new Promise<Response>((resolve) => {
      release = resolve;
    });
    const api = worldApi(world);
    renderApp("/swimlanes", (call) => (new URL(call.path, "http://x").pathname === "/api/v1/swimlanes" ? slow : api(call)));
    expect(await screen.findByLabelText("Loading swimlanes")).toHaveAttribute("aria-busy", "true");
    // The title is on screen before the data is.
    expect(screen.getByRole("heading", { level: 1, name: "Swimlanes" })).toBeInTheDocument();
    expect(screen.getByText("Loading swimlanes…")).toBeInTheDocument();
    release(json({ items: LANES, next: null }));
    expect(await screen.findByRole("link", { name: "sitb" })).toBeInTheDocument();
  });

  it("shows the problem's title and its request ID when the list cannot be read, and Retry reads it again", async () => {
    world.listProblem = problem(500, "internal");
    const app = open();
    expect(await screen.findByText("Couldn't load swimlanes")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    // The design's words (States R01).
    expect(screen.getByText("The server returned an error while reading agent state. Retry, or send the request ID to the platform team.")).toBeInTheDocument();
    world.listProblem = null;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("link", { name: "sitb" })).toBeInTheDocument();
    app.unmount();
  }, 15_000);

  it("tells an Admin to add a swimlane when there is none", async () => {
    world.lanes = [];
    open("admin");
    expect(await screen.findByText("No swimlanes registered yet")).toBeInTheDocument();
    expect(screen.getByText("A swimlane appears here once an agent registers for its Kubernetes namespace.")).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: "Add a swimlane" }).length).toBeGreaterThan(0);
  });

  it("draws the empty state as the design does: centred in the table's area, with a primary button, and a toolbar that is not dimmed", async () => {
    world.lanes = [];
    open("admin");
    const state = (await screen.findByText("No swimlanes registered yet")).closest("[data-slot='empty-state']");
    expect(state).not.toBeNull();
    // It fills what is left of the table's area, so that its own centring puts it in the middle of it (the design's frame 1i).
    expect(state).toHaveClass("flex-1");
    // The one next step is the primary button; the link in the page header stays the secondary one.
    const inState = within(state as HTMLElement).getByRole("link", { name: "Add a swimlane" });
    expect(inState).toHaveClass("bg-primary");
    // An empty list is not a loading one: only "Compare selected" is off (nothing to select), the filters are as they always are.
    const toolbar = document.querySelector("[data-slot='toolbar']");
    expect(toolbar).not.toHaveClass("opacity-50");
    expect(screen.getByRole("searchbox", { name: "Filter swimlanes" })).toBeEnabled();
  });

  it("draws the toolbar's four buttons as the design does: 8 px of padding and 12 px icons", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    for (const name of ["Tier", "Agent status", "Has drift", "Columns"]) {
      expect(screen.getByRole("button", { name }), name).toHaveClass("px-2", "[&_svg:not([class*='size-'])]:size-3");
    }
  });

  it("tells a Viewer to ask an admin when there is none", async () => {
    world.lanes = [];
    open("viewer");
    expect(await screen.findByText("No swimlanes registered yet")).toBeInTheDocument();
    expect(screen.getByText("Ask an admin to add one.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Add a swimlane" })).toBeNull();
  });

  it("says no swimlane matches when the filters leave none, and clearing them brings the list back", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter swimlanes" }), "zzz");
    expect(await screen.findByText("No swimlane matches")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(await screen.findByRole("link", { name: "sitb" })).toBeInTheDocument();
  });
});

describe("the filters", () => {
  it("narrow the list as the person types, and keep what they typed in the address", async () => {
    const app = open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter swimlanes" }), "presit");
    await waitFor(() => expect(screen.queryByRole("link", { name: "sitb" })).toBeNull());
    expect(screen.getByRole("link", { name: "presitb" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "presitc" })).toBeInTheDocument();
    await waitFor(() => expect(app.location()).toBe("/swimlanes?q=presit"));
  });

  it("start from the address: the tier, the text and Has drift", async () => {
    open("admin", "/swimlanes?tier=presit&state=drift&q=b");
    expect(await screen.findByRole("link", { name: "presitb" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "presitc" })).toBeNull();
    expect(screen.queryByRole("link", { name: "sitb" })).toBeNull();
    expect(screen.getByRole("searchbox", { name: "Filter swimlanes" })).toHaveValue("b");
    expect(screen.getByRole("button", { name: "Has drift" })).toHaveAttribute("aria-pressed", "true");
  });

  it("filter to the swimlanes that have drift when Has drift is on", async () => {
    const app = open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.click(screen.getByRole("button", { name: "Has drift" }));
    await waitFor(() => expect(screen.queryByRole("link", { name: "sit" })).toBeNull());
    expect(screen.getByRole("link", { name: "sitb" })).toBeInTheDocument();
    await waitFor(() => expect(app.location()).toBe("/swimlanes?state=drift"));
  });

  it("filter by tier from a menu that lists the tiers that exist", async () => {
    const app = open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.click(screen.getByRole("button", { name: /^Tier/ }));
    expect(await screen.findByRole("menuitemradio", { name: "All tiers" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("menuitemradio", { name: "PRESIT" }));
    await waitFor(() => expect(screen.queryByRole("link", { name: "sit" })).toBeNull());
    expect(screen.getByRole("link", { name: "presitc" })).toBeInTheDocument();
    await waitFor(() => expect(app.location()).toBe("/swimlanes?tier=presit"));
  });

  it("filter by agent status from a menu", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.click(screen.getByRole("button", { name: /^Agent status/ }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: "Offline" }));
    await waitFor(() => expect(screen.queryByRole("link", { name: "sitb" })).toBeNull());
    expect(screen.getByRole("link", { name: "presitb" })).toBeInTheDocument();
  });

  it("take a pattern character as itself", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter swimlanes" }), "(");
    expect(await screen.findByText("No swimlane matches")).toBeInTheDocument();
  });
});

describe("selection and Compare selected", () => {
  it("keeps Compare disabled, with its reason, until two swimlanes are selected", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    const compare = screen.getByRole("button", { name: "Compare selected" });
    expect(compare).toHaveAttribute("aria-disabled", "true");
    expect(compare).toHaveAccessibleDescription("Select two or more swimlanes to compare.");
    expect(screen.queryByRole("link", { name: "Compare selected" })).toBeNull();
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sit" }));
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Compare selected" })).toHaveAttribute("aria-disabled", "true");
  });

  it("links to the comparison of the selected swimlanes, in the order of the list", async () => {
    const app = open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sitb" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sit" }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();
    const compare = screen.getByRole("link", { name: "Compare selected" });
    expect(compare).toHaveAttribute("href", "/compare?lanes=sit%2Csitb");
    await userEvent.click(compare);
    await app.settled();
    expect(app.location()).toBe("/compare?lanes=sit%2Csitb");
  });

  it("compares the selected rows on the key c, and does nothing with fewer than two", async () => {
    const app = open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.keyboard("c");
    expect(app.location()).toBe("/swimlanes");
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sit" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select presitb" }));
    await userEvent.keyboard("c");
    await waitFor(() => expect(app.location()).toBe("/compare?lanes=presitb%2Csit"));
  });

  // The keymap row "Select row; Shift extends the range": found by the keyboard journey in a real browser, where Shift and Space
  // selected only the row under the cursor.
  it("extends the selection from the last row chosen to the row under the cursor on Shift and Space, in the order of the list", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    screen.getByRole("grid", { name: "Swimlanes" }).focus();
    await userEvent.keyboard("{ArrowDown}{ }");
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Shift>}{ }{/Shift}");
    expect(screen.getByText("3 selected")).toBeInTheDocument();
    for (const id of ["presitb", "presitc", "sit"]) expect(screen.getByRole("checkbox", { name: `Select ${id}` })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Select sitb" })).not.toBeChecked();
  });

  it("extends upwards too, from a row chosen lower down", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    screen.getByRole("grid", { name: "Swimlanes" }).focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{ }");
    expect(screen.getByRole("checkbox", { name: "Select sitb" })).toBeChecked();
    await userEvent.keyboard("{ArrowUp}{ArrowUp}{Shift>}{ }{/Shift}");
    expect(screen.getByText("3 selected")).toBeInTheDocument();
    for (const id of ["presitc", "sit", "sitb"]) expect(screen.getByRole("checkbox", { name: `Select ${id}` })).toBeChecked();
  });

  it("a Shift and Space with no row chosen before chooses that row alone", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    screen.getByRole("grid", { name: "Swimlanes" }).focus();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Shift>}{ }{/Shift}");
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Select presitc" })).toBeChecked();
  });

  it("forgets a selected swimlane that is filtered out of the list", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sit" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sitb" }));
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter swimlanes" }), "presit");
    await waitFor(() => expect(screen.queryByText("2 selected")).toBeNull());
  });
});

describe("sorting", () => {
  const names = () => screen.getAllByRole("link").filter((a) => /^\/swimlanes\/[a-z0-9-]+$/.test(a.getAttribute("href") ?? "") && a.closest('[role="row"]') !== null).map((a) => a.textContent);

  it("is by name to begin with, and the Findings header sorts by findings", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    expect(names()).toEqual(["presitb", "presitc", "sit", "sitb"]);
    expect(screen.getByRole("columnheader", { name: /Swimlane/ })).toHaveAttribute("aria-sort", "ascending");
    await userEvent.click(within(screen.getByRole("columnheader", { name: "Findings" })).getByRole("button"));
    await waitFor(() => expect(names()).toEqual(["presitc", "sit", "presitb", "sitb"]));
    expect(screen.getByRole("columnheader", { name: "Findings" })).toHaveAttribute("aria-sort", "ascending");
  });
});

describe("the column chooser", () => {
  it("hides a column, remembers the choice as a layout preference, and never offers to hide the Swimlane column", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.click(screen.getByRole("button", { name: /^Columns/ }));
    const swimlane = await screen.findByRole("menuitemcheckbox", { name: /Swimlane/ });
    expect(swimlane).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(screen.getByRole("menuitemcheckbox", { name: "Project" }));
    await waitFor(() => expect(screen.queryByRole("columnheader", { name: "Project" })).toBeNull());
    expect(prefs.get("tableColumns", "swimlanes")).not.toContain("project");
    expect(prefs.get("tableColumns", "swimlanes")).toContain("agent");
  });

  it("starts from the columns it remembered", async () => {
    prefs.set("tableColumns", "swimlanes", ["swimlane", "agent"]);
    open();
    await screen.findByRole("link", { name: "sitb" });
    expect(screen.getByRole("columnheader", { name: "Agent" })).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: "Findings" })).toBeNull();
  });
});

describe("the keyboard", () => {
  it("focuses the filter on / and types into it", async () => {
    open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.keyboard("/");
    expect(screen.getByRole("searchbox", { name: "Filter swimlanes" })).toHaveFocus();
  });

  it("moves down the rows with the arrow key and opens the swimlane with Enter, from the page", async () => {
    const app = open();
    await screen.findByRole("link", { name: "sitb" });
    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getByRole("grid", { name: "Swimlanes" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await app.settled();
    expect(app.location()).toBe("/swimlanes/presitb");
  });
});

describe("opening a swimlane in a new tab", () => {
  it("opens the swimlane under the cursor in a new tab on Control+Enter, and leaves this page where it is", async () => {
    const opened: string[] = [];
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      opened.push(`${this.getAttribute("href")} ${this.target}`);
    });
    try {
      const app = open();
      await screen.findByRole("link", { name: "sitb" });
      await userEvent.keyboard("{ArrowDown}");
      await userEvent.keyboard("{Control>}{Enter}{/Control}");
      expect(opened).toEqual(["/swimlanes/presitb _blank"]);
      expect(app.location()).toBe("/swimlanes");
    } finally {
      click.mockRestore();
    }
  });
});

describe("a selected row", () => {
  it("is drawn as selected, with the selection colour, and the others are not", async () => {
    open();
    await userEvent.click(await screen.findByRole("checkbox", { name: "Select sitb" }));
    const selected = await rowOf("sitb");
    expect(selected).toHaveAttribute("aria-selected", "true");
    expect(selected.firstElementChild).toHaveClass("bg-selection");
    expect((await rowOf("sit")).firstElementChild).not.toHaveClass("bg-selection");
  });
});

describe("permission", () => {
  it("shows the list to a Viewer, who can open every swimlane", async () => {
    const api: Api = worldApi({ ...world, role: "viewer" });
    renderApp("/swimlanes", api);
    expect(await screen.findByRole("link", { name: "presitc" })).toHaveAttribute("href", "/swimlanes/presitc");
  });
});

describe("the Pickup column", () => {
  it("counts each state under its own name, never the total of two states under the name of one", async () => {
    world.lanes = world.lanes.map((l) => (l.id === "sitb" ? { ...l, pickup_counts: [["needs_notify_or_restart", 2], ["needs_config_server_restart", 1]] } : l));
    open();
    const sitb = await rowOf("sitb");
    expect(within(sitb).getByRole("img", { name: "1 Needs config-server restart" })).toBeInTheDocument();
    expect(within(sitb).getByRole("img", { name: "2 Needs notify or restart" })).toBeInTheDocument();
    expect(within(sitb).queryByRole("img", { name: /^3 / })).toBeNull();
  });
});
