import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { prefs } from "@/lib/prefs";
import { FakeHub, frame } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";
import { json, me, renderApp, type Api } from "@/test/app";
import { resetCurrentSwimlane } from "./current-swimlane";

// The shell as a person meets it: the real router, the real session, a fake server. What the sidebar shows each role, which
// item is current, what the counts are, collapsing, the keys, the account menu, the switcher, the landmarks and focus.

const lane = (id: string, over: Record<string, unknown> = {}) => ({
  id,
  display_name: id,
  tenants: ["sit1"],
  agent: "online",
  last_scan_at: null,
  base_version: "v2.3.1",
  drift_counts: [],
  finding_count: 0,
  tier: "SIT",
  ...over,
});

const LANES = [
  lane("sit", { finding_count: 0 }),
  lane("sitb", { tenants: ["sit7", "sit8", "sit9"], drift_counts: [["conflict", 1], ["nfs_ahead", 2], ["git_ahead", 1]], finding_count: 2 }),
  lane("sitc", { finding_count: 1 }),
  lane("presitc", { tier: "PRESIT", tenants: [], drift_counts: [["unknown", 412]], finding_count: 8 }),
];

// An access request that waits for an Admin (the sidebar counts them; nothing else of it is read).
const waitingRequest = (i: number) => ({ id: i + 1, user: `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`, email: `person${i}@example.test`, requested_role: "editor", reason: null, status: "pending", created_at: 1_760_000_000_000 });

function server(role: string | null, opts: { pending?: number; pendingNext?: string; requests?: () => number; requestsNext?: string; credential?: unknown; mcp?: boolean } = {}): Api {
  return (call) => {
    if (call.path === "/api/v1/me") return json(me(role));
    if (call.path.startsWith("/api/v1/swimlanes")) return json({ items: LANES, next: null });
    if (call.path.startsWith("/api/v1/proposals")) return json({ items: Array.from({ length: opts.pending ?? 0 }, (_, i) => ({ id: i + 1, status: "pending" })), next: opts.pendingNext ?? null });
    if (call.path.startsWith("/api/v1/admin/access-requests")) return json({ items: Array.from({ length: opts.requests?.() ?? 0 }, (_, i) => waitingRequest(i)), next: opts.requestsNext ?? null });
    if (call.path === "/api/v1/me/github-token") return opts.credential === undefined ? new Response("{}", { status: 404 }) : json(opts.credential);
    if (call.path.startsWith("/api/v1/me/mcp-tokens")) return opts.mcp ? json({ items: [], next: null }) : new Response(JSON.stringify({ type: "urn:lanekeeper:problem:not_found", title: "Not found", status: 404 }), { status: 404, headers: { "content-type": "application/problem+json" } });
    if (call.path === "/auth/logout") return new Response(null, { status: 204 });
    return new Response(JSON.stringify({ type: "urn:lanekeeper:problem:not_found", title: "Not found", status: 404 }), { status: 404, headers: { "content-type": "application/problem+json" } });
  };
}

// The switcher's rows are grid cells, which jsdom (no stylesheet) reads as one run of text, so a row is found by its value (the
// swimlane's id, which is what cmdk keeps in `data-value`) and not by an accessible name.
async function row(id: string): Promise<HTMLElement> {
  return waitFor(
    () => {
      const found = document.querySelector<HTMLElement>(`[role="option"][data-value="${id}"]`);
      expect(found).not.toBeNull();
      return found as HTMLElement;
    },
    { timeout: 4000 },
  );
}

// A screen that shows nothing but its title, for the tests whose subject is the shell around it.
const FILES_STAND_IN = { files: () => Promise.resolve({ default: () => <h1>Files</h1> }) };

const sections = () => screen.findByRole("navigation", { name: "Sections" });
const itemNames = async () => {
  const nav = await sections();
  return within(nav)
    .getAllByRole("link")
    .map((a) => (a.getAttribute("aria-label") ?? a.textContent ?? "").replace(/\d+\s*(open|pending)?$/, "").replace(/\/$/, "").trim());
};

beforeEach(() => {
  prefs.clear();
  resetCurrentSwimlane();
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

describe("the sidebar by role", () => {
  it("shows an Admin all eight items, and Search with its three kinds", async () => {
    renderApp("/swimlanes", server("admin"));
    expect(await itemNames()).toEqual(["Swimlanes", "Compare", "Search", "Text", "Settings", "Docs", "Feature status", "Findings", "Approvals", "Audit", "Admin"]);
  });

  it("shows a Viewer, an Editor and an Operator neither Approvals nor Audit nor Admin", async () => {
    for (const role of ["viewer", "editor", "operator"]) {
      const app = renderApp("/swimlanes", server(role));
      expect(await itemNames(), role).toEqual(["Swimlanes", "Compare", "Search", "Text", "Settings", "Docs", "Feature status", "Findings"]);
      app.unmount();
    }
  });

  it("shows a person with no role no items at all (the screen is the permission state)", async () => {
    renderApp("/swimlanes", server(null));
    expect(await screen.findByRole("heading", { level: 1, name: "You do not have access" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Compare" })).toBeNull();
  });

  it("marks the item a screen belongs under as the current page", async () => {
    // The file screen is a stand-in: the shell is what is tested, and the real one is a chunk of hundreds of modules, which on a busy
    // machine took longer to load than a wait is willing to give (the one failure of a wide run).
    renderApp("/swimlanes/sitb/files?path=a.yml", server("admin"), { pages: FILES_STAND_IN });
    const nav = await sections();
    expect(within(nav).getByRole("link", { name: /^Swimlanes/ })).toHaveAttribute("aria-current", "page");
    expect(within(nav).getByRole("link", { name: "Compare" })).not.toHaveAttribute("aria-current");
  });

  it("links each item to its screen, and a plain click is a route change", async () => {
    const app = renderApp("/swimlanes", server("admin"));
    const nav = await sections();
    expect(within(nav).getByRole("link", { name: "Feature status" })).toHaveAttribute("href", "/status");
    expect(within(nav).getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/search?type=settings");
    await userEvent.click(within(nav).getByRole("link", { name: "Compare" }));
    await app.settled();
    expect(app.location()).toBe("/compare");
  });
});

describe("loading in the background", () => {
  it("leaves the palette's chunk to the browser's idle time: a timer would load dozens of modules in the middle of a test, and can still be loading when the file ends (EnvironmentTeardownError)", async () => {
    const timers = vi.spyOn(globalThis, "setTimeout");
    try {
      renderApp("/swimlanes", server("admin"));
      await sections();
      expect(timers.mock.calls.map((call) => call[1])).not.toContain(1500);
    } finally {
      timers.mockRestore();
    }
  });
});

describe("the counts", () => {
  it("adds up the findings of every swimlane, and counts the pending approvals for an Admin", async () => {
    renderApp("/swimlanes", server("admin", { pending: 6 }));
    const nav = await sections();
    await waitFor(() => expect(within(within(nav).getByRole("link", { name: /^Findings/ })).getByText("11")).toBeInTheDocument());
    await waitFor(() => expect(within(within(nav).getByRole("link", { name: /^Approvals/ })).getByText("6")).toBeInTheDocument());
  });

  it("caps a count at 99+", async () => {
    renderApp("/swimlanes", server("admin", { pending: 100 }));
    const nav = await sections();
    await waitFor(() => expect(within(within(nav).getByRole("link", { name: /^Approvals/ })).getByText("99+")).toBeInTheDocument());
  });

  it("adds a plus when the hub has more pending approvals than the page it sent (it sends at most 25), since the count is then a floor", async () => {
    renderApp("/swimlanes", server("admin", { pending: 25, pendingNext: "more" }));
    const nav = await sections();
    await waitFor(() => expect(within(within(nav).getByRole("link", { name: /^Approvals/ })).getByText("25+")).toBeInTheDocument());
    expect(within(nav).getByRole("link", { name: /^Approvals/ })).toHaveTextContent("25+ pending");
  });

  describe("the Admin item", () => {
    const adminLink = (nav: HTMLElement) => within(nav).getByRole("link", { name: /^Admin/ });

    it("counts the access requests that wait for an Admin, in words for assistive technology", async () => {
      renderApp("/swimlanes", server("admin", { requests: () => 3 }));
      const nav = await sections();
      await waitFor(() => expect(within(adminLink(nav)).getByText("3")).toBeInTheDocument());
      expect(adminLink(nav)).toHaveAccessibleName(/^Admin\s*3\s*pending requests$/);
      expect(adminLink(nav)).toHaveAttribute("href", "/admin/users");
    });

    it("is named 'Admin, 3 pending requests' in the collapsed rail", async () => {
      prefs.set("sidebarCollapsed", true);
      renderApp("/swimlanes", server("admin", { requests: () => 3 }));
      const nav = await sections();
      await waitFor(() => expect(within(nav).getByRole("link", { name: "Admin, 3 pending requests" })).toBeInTheDocument());
    });

    it("says 1 pending request, not 1 pending requests", async () => {
      prefs.set("sidebarCollapsed", true);
      renderApp("/swimlanes", server("admin", { requests: () => 1 }));
      const nav = await sections();
      await waitFor(() => expect(within(nav).getByRole("link", { name: "Admin, 1 pending request" })).toBeInTheDocument());
    });

    it("shows no count when nothing waits", async () => {
      const app = renderApp("/swimlanes", server("admin", { requests: () => 0 }));
      const nav = await sections();
      await waitFor(() => expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin/access-requests"))).toBe(true));
      await app.settled();
      expect(adminLink(nav)).toHaveAccessibleName("Admin");
      expect(within(adminLink(nav)).queryByText(/\d/)).toBeNull();
    });

    it("adds a plus when the hub has more waiting requests than the page it sent", async () => {
      renderApp("/swimlanes", server("admin", { requests: () => 4, requestsNext: "more" }));
      const nav = await sections();
      await waitFor(() => expect(within(adminLink(nav)).getByText("4+")).toBeInTheDocument());
      expect(adminLink(nav)).toHaveAccessibleName(/^Admin\s*4\+\s*pending requests$/);
    });

    it("is not asked for, and not shown, to anyone but an Admin", async () => {
      const app = renderApp("/swimlanes", server("operator", { requests: () => 3 }));
      const nav = await sections();
      await waitFor(() => expect(app.calls.some((c) => c.path.startsWith("/api/v1/swimlanes"))).toBe(true));
      await app.settled();
      expect(within(nav).queryByRole("link", { name: /^Admin/ })).toBeNull();
      expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin/access-requests"))).toBe(false);
    });

    it("changes when a request arrives, through the live stream, without a reload", async () => {
      let waiting = 0;
      const hub = new FakeHub();
      const app = renderApp("/swimlanes", server("admin", { requests: () => waiting }), { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
      const nav = await sections();
      await waitFor(() => expect(hub.connections).toHaveLength(1));
      await act(async () => {
        hub.last.push(FIRST_FRAME);
        await Promise.resolve();
      });
      await waitFor(() => expect(app.calls.filter((c) => c.path.startsWith("/api/v1/admin/access-requests")).length).toBeGreaterThanOrEqual(1));
      expect(within(adminLink(nav)).queryByText(/\d/)).toBeNull();
      waiting = 2;
      await act(async () => {
        hub.last.push(frame(41, { type: "access_request_created", user: "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002" }));
        await Promise.resolve();
      });
      await waitFor(() => expect(within(adminLink(nav)).getByText("2")).toBeInTheDocument(), { timeout: 4000 });
      expect(adminLink(nav)).toHaveAccessibleName(/^Admin\s*2\s*pending requests$/);
    });
  });

  it("asks for the pending approvals only when the person is an Admin", async () => {
    const app = renderApp("/swimlanes", server("editor"));
    await sections();
    await waitFor(() => expect(app.calls.some((c) => c.path.startsWith("/api/v1/swimlanes"))).toBe(true));
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/proposals"))).toBe(false);
  });
});

describe("collapsing", () => {
  it("collapses to a rail on the toggle, keeps the labels for assistive technology, and remembers the choice", async () => {
    renderApp("/swimlanes", server("admin", { pending: 6 }));
    const nav = await sections();
    expect(screen.getByRole("complementary", { name: "Primary" })).toHaveAttribute("data-collapsed", "false");
    await userEvent.click(screen.getByRole("button", { name: "Toggle sidebar" }));
    expect(screen.getByRole("complementary", { name: "Primary" })).toHaveAttribute("data-collapsed", "true");
    expect(within(nav).getByRole("link", { name: "Compare" })).toBeInTheDocument();
    await waitFor(() => expect(within(nav).getByRole("link", { name: /^Approvals, 6 pending$/ })).toBeInTheDocument());
    expect(prefs.get("sidebarCollapsed")).toBe(true);
  });

  it("starts collapsed when the person left it so", async () => {
    prefs.set("sidebarCollapsed", true);
    renderApp("/swimlanes", server("viewer"));
    await sections();
    expect(screen.getByRole("complementary", { name: "Primary" })).toHaveAttribute("data-collapsed", "true");
  });

  it("toggles on Control B too", async () => {
    renderApp("/swimlanes", server("viewer"));
    await sections();
    fireEvent.keyDown(document.body, { key: "b", ctrlKey: true });
    expect(screen.getByRole("complementary", { name: "Primary" })).toHaveAttribute("data-collapsed", "true");
    fireEvent.keyDown(document.body, { key: "b", metaKey: true });
    expect(screen.getByRole("complementary", { name: "Primary" })).toHaveAttribute("data-collapsed", "false");
  });
});

describe("the keys", () => {
  it("opens the shortcuts sheet on ? and the palette on Control K", async () => {
    renderApp("/swimlanes", server("viewer"));
    await sections();
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    expect(await screen.findByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
  });

  it("opens the palette from the header's search button", async () => {
    renderApp("/swimlanes", server("viewer"));
    await userEvent.click(await screen.findByRole("button", { name: /Search or jump to/ }));
    expect(await screen.findByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
  });

  it("goes to a screen on g then a letter, and not to one the role cannot open", async () => {
    const app = renderApp("/swimlanes", server("viewer"));
    await sections();
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "c" });
    await waitFor(() => expect(app.location()).toBe("/compare"));
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "a" });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(app.location()).toBe("/compare");
  });

  it("lets an Admin go to Audit", async () => {
    const app = renderApp("/swimlanes", server("admin"));
    await sections();
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "a" });
    await waitFor(() => expect(app.location()).toBe("/audit"));
  });

  it("goes to the text search on /", async () => {
    const app = renderApp("/compare", server("viewer"));
    await sections();
    fireEvent.keyDown(document.body, { key: "/" });
    await waitFor(() => expect(app.location()).toBe("/search?type=text"));
  });

  it("does nothing for a letter typed in a field", async () => {
    const app = renderApp("/swimlanes", server("viewer"), { pages: { swimlanes: () => Promise.resolve({ default: () => (<div><h1>Swimlanes</h1><input aria-label="Filter" /></div>) }) } });
    await sections();
    const input = await screen.findByLabelText("Filter");
    input.focus();
    fireEvent.keyDown(input, { key: "g" });
    fireEvent.keyDown(input, { key: "c" });
    fireEvent.keyDown(input, { key: "?", shiftKey: true });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(app.location()).toBe("/swimlanes");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("binds no key on a screen before sign-in", async () => {
    const app = renderApp("/signin", () => new Response("{}", { status: 404 }));
    await screen.findByRole("heading", { level: 1, name: "Lanekeeper" });
    fireEvent.keyDown(document.body, { key: "?", shiftKey: true });
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(app.location()).toBe("/signin");
  });
});

describe("the header", () => {
  it("shows the swimlane of the page with its tier, first tenant, +N chip and drift, and says the whole set to assistive technology", async () => {
    renderApp("/swimlanes/sitb", server("viewer"));
    const button = await screen.findByRole("button", { name: /Swimlane: sitb/ });
    await waitFor(() => expect(button).toHaveAccessibleName(/tenants sit7, sit8, sit9/));
    expect(button).toHaveAccessibleName(/SIT/);
    expect(button).toHaveAccessibleName(/4 drift/);
    expect(within(button).getByText("SIT")).toBeInTheDocument();
    expect(within(button).getByText("sit7")).toBeInTheDocument();
    expect(within(button).getByText(/^\+2/)).toBeInTheDocument();
  });

  it("tolerates a swimlane with no tenants and says its files are of unknown state", async () => {
    renderApp("/swimlanes/presitc", server("viewer"));
    const button = await screen.findByRole("button", { name: /Swimlane: presitc/ });
    await waitFor(() => expect(within(button).getByText("No tenants")).toBeInTheDocument());
    expect(within(button).getByText(/412/)).toBeInTheDocument();
    expect(within(button).getByText("unknown")).toBeInTheDocument();
  });

  it("opens the list, and choosing a swimlane keeps the person on the same tab of the new one", async () => {
    const app = renderApp("/swimlanes/sitb/services", server("viewer"));
    const button = await screen.findByRole("button", { name: /Swimlane: sitb/ });
    await userEvent.click(button);
    await userEvent.click(await row("sitc"));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitc/services"));
    await waitFor(() => expect(screen.getByRole("button", { name: /Swimlane: sitc/ })).toBeInTheDocument());
  });

  it("keeps the person on the Pull requests tab of a swimlane when another swimlane is chosen", async () => {
    const app = renderApp("/swimlanes/sitb/pull-requests", server("viewer"));
    const button = await screen.findByRole("button", { name: /Swimlane: sitb/ });
    await userEvent.click(button);
    await userEvent.click(await row("sitc"));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitc/pull-requests"));
  });

  it("keeps the person on the As served page of a swimlane when another swimlane is chosen, and asks for nothing the old one had selected", async () => {
    const app = renderApp("/swimlanes/sitb/served?application=payments&tenant=sit7&file=core%2Fa.yml", server("viewer"));
    const button = await screen.findByRole("button", { name: /Swimlane: sitb/ });
    await userEvent.click(button);
    await userEvent.click(await row("sitc"));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitc/served"));
  });

  it("puts focus in the filter field once the list has loaded, so that typing right after opening filters it", async () => {
    renderApp("/compare", server("viewer"));
    await userEvent.click(await screen.findByRole("button", { name: /Choose a swimlane/ }));
    const filter = await screen.findByLabelText("Filter swimlanes");
    await waitFor(() => expect(filter).toHaveFocus());
    await userEvent.keyboard("presitc");
    await waitFor(() => expect(document.querySelectorAll("[cmdk-item]:not([hidden])")).toHaveLength(1));
    expect(document.querySelector("[cmdk-item][data-value='presitc']")).not.toBeNull();
  });

  it("is a named dialog, and a filter that leaves nothing says so in a status, with no empty listbox (axe `aria-dialog-name`, `aria-required-children`)", async () => {
    renderApp("/compare", server("viewer"));
    await userEvent.click(await screen.findByRole("button", { name: /Choose a swimlane/ }));
    expect(await screen.findByRole("dialog", { name: "Choose a swimlane" })).toBeInTheDocument();
    const filter = await screen.findByLabelText("Filter swimlanes");
    await waitFor(() => expect(filter).toHaveFocus());
    await userEvent.keyboard("zzzzzz");
    const message = await screen.findByText("No swimlane matches.");
    expect(message.closest("[role='status']")).not.toBeNull();
    expect(message.closest("[cmdk-list]")).toBeNull();
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("opens the overview of the chosen swimlane from a screen that is not a swimlane's", async () => {
    const app = renderApp("/compare", server("viewer"));
    const button = await screen.findByRole("button", { name: /Choose a swimlane/ });
    await userEvent.click(button);
    await userEvent.click(await row("sitc"));
    await waitFor(() => expect(app.location()).toBe("/swimlanes/sitc"));
  });

  it("has no notifications bell: nothing in the contract to put behind it", async () => {
    renderApp("/swimlanes", server("viewer"));
    await sections();
    expect(screen.queryByRole("button", { name: /notifications/i })).toBeNull();
  });

  it("shows the breadcrumb of the page, with the current page marked", async () => {
    renderApp("/swimlanes/sitb/files?path=tx-infinity-api%2Fcore.yml", server("viewer"), { pages: FILES_STAND_IN });
    const crumbs = await screen.findByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs).getByRole("link", { name: "Swimlanes" })).toHaveAttribute("href", "/swimlanes");
    expect(within(crumbs).getByRole("link", { name: "tx-infinity-api" })).toHaveAttribute("href", "/swimlanes/sitb/files?path=tx-infinity-api");
    expect(within(crumbs).getByText("core.yml").closest("li")).toHaveAttribute("aria-current", "page");
  });

  it("lets the earlier crumbs shrink to an ellipsis too, after the last one has: a path with a long folder, or words 40% longer, cut a crumb in two with no sign (the layout run, German-length labels)", async () => {
    renderApp("/swimlanes/sitb/files?path=tx-infinity-api%2Fcore.yml", server("viewer"), { pages: FILES_STAND_IN });
    const crumbs = await screen.findByRole("navigation", { name: "Breadcrumb" });
    const link = within(crumbs).getByRole("link", { name: "tx-infinity-api" });
    const folder = link.closest("li");
    expect(folder?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    // The link is the box that gives way (a block that ends in an ellipsis), not a line of text that runs past a box that clips it: the
    // box of a link that overflows its item lies over the next link's (axe `target-size`).
    expect(link.className).toMatch(/(^|\s)block(\s|$)/);
    expect(link.className).toMatch(/(^|\s)truncate(\s|$)/);
    // The last crumb gives way first: its share of the lack of room is a thousand times that of any of the others (a factor under one
    // would take only that part of what is lacking, and the crumbs would still be cut by the box).
    expect(within(crumbs).getByText("core.yml").closest("li")?.className).toMatch(/shrink-\[1000\]/);
    expect(within(crumbs).getByText("core.yml").closest("li")?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    // A link inside a box that clips has its focus ring drawn inside it, or the ring would be cut.
    expect(within(crumbs).getByRole("link", { name: "tx-infinity-api" }).className).toMatch(/focus-visible:-outline-offset-2/);
  });

  it("shows the live pill and no banner while the stream is up", async () => {
    const hub = new FakeHub();
    renderApp("/swimlanes", server("viewer"), { live: { stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } } });
    await waitFor(() => expect(hub.connections).toHaveLength(1));
    hub.last.push(FIRST_FRAME);
    expect(await screen.findByText("Live", { selector: "[aria-hidden='true']" })).toBeInTheDocument();
    expect(screen.queryByText("Live updates paused, retrying")).toBeNull();
  });
});

describe("the account menu", () => {
  it("shows who is signed in with the role in words, and the items for an Editor", async () => {
    renderApp("/swimlanes", server("editor", { credential: { github_login: "ada", verified_at: 1, scopes: [], expires_at: Date.now() + 41 * 86_400_000 - 1000, expires_soon: false } }));
    await userEvent.click(await screen.findByRole("button", { name: /Ada Lovelace/ }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByText("My GitHub token")).toBeInTheDocument();
    await waitFor(() => expect(within(menu).getByText("41 days left")).toBeInTheDocument());
    expect(within(menu).getByRole("menuitem", { name: /Keyboard shortcuts/ })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Sign out" })).toBeInTheDocument();
  });

  it("lists Personal MCP tokens only when the hub has the mode on (its list answers, and does not answer 404)", async () => {
    const on = renderApp("/swimlanes", server("viewer", { mcp: true }));
    await userEvent.click(await screen.findByRole("button", { name: /Ada Lovelace/ }));
    expect(await screen.findByRole("menuitem", { name: "Personal MCP tokens" })).toHaveAttribute("href", "/me/tokens");
    on.unmount();
    renderApp("/swimlanes", server("viewer", { mcp: false }));
    await userEvent.click(await screen.findByRole("button", { name: /Ada Lovelace/ }));
    await screen.findByRole("menuitem", { name: /Keyboard shortcuts/ });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
    expect(screen.queryByRole("menuitem", { name: "Personal MCP tokens" })).toBeNull();
  });

  it("has no My GitHub token for a Viewer, who cannot use one", async () => {
    renderApp("/swimlanes", server("viewer"));
    await userEvent.click(await screen.findByRole("button", { name: /Ada Lovelace/ }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByText("My GitHub token")).toBeNull();
  });

  it("opens the shortcuts sheet from its item, once the menu has closed", async () => {
    renderApp("/swimlanes", server("viewer"));
    await userEvent.click(await screen.findByRole("button", { name: /Ada Lovelace/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /Keyboard shortcuts/ }));
    expect(await screen.findByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
  });

  it("switches the theme, and the choice stays marked", async () => {
    document.documentElement.classList.remove("dark");
    renderApp("/swimlanes", server("viewer"));
    await userEvent.click(await screen.findByRole("button", { name: /Ada Lovelace/ }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: "Dark" }));
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(screen.getByRole("menuitemradio", { name: "Dark" })).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getByRole("menuitemradio", { name: "Light" }));
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });
});

describe("landmarks and focus", () => {
  it("has one main, one banner, the primary sidebar and a skip link", async () => {
    renderApp("/swimlanes", server("viewer"));
    await sections();
    expect(screen.getAllByRole("main")).toHaveLength(1);
    expect(screen.getAllByRole("banner")).toHaveLength(1);
    expect(screen.getByRole("complementary", { name: "Primary" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Skip to content" })).toBeInTheDocument();
  });

  // The shell's header (z-6) and sidebar (z-5) sit above a dialog's scrim on purpose: the design dims the page and leaves the frame around it
  // as it is. What a screen draws at a layer of its own (a table's sticky header is z-10) must not reach past the screen into that order, or it
  // paints over the dialog and its scrim (seen on the severity rule dialog). So the screen's own box is a stacking context.
  // It is NOT <main>: the menus, popovers and tooltips are portaled into <main> (ui/overlay-root.ts, for the landmark rule) and are stacked at
  // z-40 in the page's order, above the sidebar and the header that a menu from the account button overlaps. A stacking context on <main> put
  // them under the sidebar, and the account menu's items could not be clicked.
  it("stacks what a screen draws inside the screen's own box, and leaves <main>, where the overlays are put, out of that", async () => {
    renderApp("/swimlanes", server("viewer"));
    await sections();
    const main = screen.getByRole("main");
    expect(main.className).not.toMatch(/(^|\s)isolate(\s|$)/);
    const screenBox = main.firstElementChild;
    expect(screenBox?.className).toMatch(/(^|\s)isolate(\s|$)/);
    // The screen is in it (the swimlanes list has a filter field), and so is nothing that the overlays are portaled beside.
    await waitFor(() => expect(screenBox?.querySelector("input")).not.toBeNull());
  });

  // WCAG 2.4.1 (Bypass Blocks): the skip link is the first stop of a page that has just loaded. Focus on the h1 is for a page the
  // person moved to, where the old page's focus is gone; on a first load it would put the skip link behind the person.
  it("leaves focus on the document when the app first opens, so the first Tab is the skip link", async () => {
    renderApp("/swimlanes", server("viewer"));
    await sections();
    await screen.findByRole("heading", { level: 1, name: "Swimlanes" });
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(document.activeElement).toBe(document.body);
    await userEvent.tab();
    expect(screen.getByRole("link", { name: "Skip to content" })).toHaveFocus();
  });

  it("moves focus to the h1 of the page after a route change, and not for a change of search keys", async () => {
    const app = renderApp("/swimlanes", server("viewer"));
    await sections();
    await userEvent.click(within(await sections()).getByRole("link", { name: "Compare" }));
    await app.settled();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("heading", { level: 1, name: "Compare" })));
    // The same path with another search: the person stays where they are.
    const input = document.createElement("input");
    document.body.append(input);
    input.focus();
    await act(async () => {
      await app.router.navigate({ to: "/compare", search: { only: "diff" } });
    });
    expect(document.activeElement).toBe(input);
  });

  it("keeps the tab title in step with the page", async () => {
    const app = renderApp("/swimlanes", server("viewer"));
    await sections();
    await waitFor(() => expect(document.title).toBe("Swimlanes · Lanekeeper"));
    await act(async () => {
      await app.router.navigate({ to: "/swimlanes/$id/services", params: { id: "sitb" } });
    });
    await waitFor(() => expect(document.title).toBe("Services · sitb · Swimlanes · Lanekeeper"));
  });

  it("shows no chrome on a screen before sign-in, and still one main", async () => {
    renderApp("/signin", () => new Response("{}", { status: 404 }));
    await screen.findByRole("heading", { level: 1, name: "Lanekeeper" });
    expect(screen.queryByRole("complementary", { name: "Primary" })).toBeNull();
    expect(screen.getAllByRole("main")).toHaveLength(1);
  });
});
