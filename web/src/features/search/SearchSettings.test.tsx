import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SettingHit } from "@/api/generated/model";
import { settingHitsFor } from "@/mocks/design/search/data";
import { at, hub, searchBackend, type Route } from "@/mocks/design/search/hub";
import { json, problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Settings search: the Settings tab. It finds a setting by a part of its path, with its value in each swimlane, a link to the
// line, and a way into the grid. Results come by keyset pages; every state of the States index is here.

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const settingCalls = (calls: ReadonlyArray<{ path: string }>) => calls.filter((c) => c.path.startsWith("/api/v1/search/settings")).map((c) => new URL(c.path, "http://x").searchParams);
const open = (url: string, routes: Route[] = searchBackend()) => renderApp(url, hub("viewer", routes));
// The loading skeleton is a grid with the same name, marked busy; the table is the one that is not.
async function grid(): Promise<HTMLElement> {
  await screen.findByRole("grid", { name: "Settings" });
  await waitFor(() => expect(screen.getByRole("grid", { name: "Settings" })).not.toHaveAttribute("aria-busy", "true"));
  return screen.getByRole("grid", { name: "Settings" });
}
const query = () => screen.getByRole("searchbox", { name: "Setting query" });
const ALL = settingHitsFor("enableAccountSorting");

describe("a settings search from the address", () => {
  it("asks for the query as a part of a setting path and shows one row for each setting", async () => {
    const app = open("/search?type=settings&q=enableAccountSorting");
    const table = await grid();
    const asked = settingCalls(app.calls)[0];
    expect(asked?.get("q")).toBe("enableAccountSorting");
    expect(asked?.get("limit")).toBe("100");
    expect(asked?.has("swimlanes")).toBe(false);
    expect(asked?.has("cursor")).toBe(false);
    expect(ALL).toHaveLength(17);
    expect(screen.getByText("17 settings in 6 swimlanes")).toBeInTheDocument();
    expect(screen.getByText("Matches setting paths, ignoring case")).toBeInTheDocument();
    const headers = within(table).getAllByRole("columnheader").map((h) => h.textContent);
    expect(headers).toEqual(["Swimlane", "File", "Setting path", "Value", "Line", "Compare"]);
    expect(within(table).getAllByRole("row")).toHaveLength(18);
  });

  it("shows the swimlane, the file with its kind, the setting path with the match marked, the value and the line", async () => {
    open("/search?type=settings&q=enableAccountSorting&lanes=sitb");
    const table = await grid();
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    const tenantRow = rows.find((r) => r.textContent?.includes("tx-infinity-core-sit7.yml")) as HTMLElement;
    expect(tenantRow).toHaveTextContent("sitb");
    expect(tenantRow).toHaveTextContent("tenant");
    expect(tenantRow).toHaveTextContent("remote-itm-teller.enableAccountSorting");
    expect(tenantRow).toHaveTextContent("true");
    expect([...tenantRow.querySelectorAll("mark")].map((m) => m.textContent)).toEqual(["enableAccountSorting"]);
    const baseRow = rows.find((r) => r.textContent?.includes("atm-iso.enableAccountSorting")) as HTMLElement;
    expect(baseRow).toHaveTextContent("base");
    expect(baseRow).toHaveTextContent("false");
  });

  it("links the line to the file, and the row to the grid for the file", async () => {
    open("/search?type=settings&q=enableAccountSorting&lanes=sitb");
    const table = await grid();
    const line = within(table).getByRole("link", { name: "Open tx-infinity-core-sit7.yml at line 13" });
    expect(line).toHaveAttribute("href", "/swimlanes/sitb/files?line=13&path=tx-infinity-api%2Ftx-infinity-core-sit7.yml");
    const row = line.closest("[role=row]") as HTMLElement;
    expect(within(row).getByRole("link", { name: "Open in grid" })).toHaveAttribute("href", "/compare?file=tx-infinity-api%2Ftx-infinity-core.yml");
  });

  it("gives the Open in grid column the room of its link with the words 40% longer (148px; the layout run found it cut off at 1024 and 1280 wide)", async () => {
    open("/search?type=settings&q=enableAccountSorting&lanes=sitb");
    const table = await grid();
    const template = table.querySelector<HTMLElement>("[data-slot='virtual-list']")?.style.getPropertyValue("--lk-cols") ?? table.style.getPropertyValue("--lk-cols");
    const tracks = template.split(/\s+(?![^(]*\))/);
    expect(Number.parseInt(tracks[tracks.length - 1] ?? "0", 10)).toBeGreaterThanOrEqual(148);
  });

  it("offers no grid link for a setting whose logical file the server did not name", async () => {
    const hit: SettingHit = { swimlane: "sit", file: "a/b.yml", path: "x.y", value: { text: "1", ty: "int", flag: null }, location: { path: "x.y", lines: { start: 2, end: 2 } } };
    open("/search?type=settings&q=y", [at("GET", "/api/v1/search/settings", () => json({ items: [hit], next: null }))]);
    const table = await grid();
    expect(within(table).queryByRole("link", { name: "Open in grid" })).toBeNull();
    expect(within(table).getByRole("link", { name: "Open b.yml at line 2" })).toBeInTheDocument();
  });

  it("opens the file at the line when a row is chosen with the keyboard", async () => {
    const app = open("/search?type=settings&q=enableAccountSorting&lanes=sitb");
    const table = await grid();
    table.focus();
    await userEvent.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => expect(app.location()).toMatch(/^\/swimlanes\/sitb\/files\?line=\d+&path=/));
  });

  it("shows a hidden character and a masked value as the server sent them", async () => {
    const hits: SettingHit[] = [
      { swimlane: "sit", file: "a/b.yml", path: "db.pass‮w", value: { text: "[REDACTED: 16 chars, generic_secret]", ty: "string", flag: null }, location: { path: "db.passw", lines: { start: 4, end: 4 } }, logical_file: "a/b.yml" },
    ];
    open("/search?type=settings&q=pass", [at("GET", "/api/v1/search/settings", () => json({ items: hits, next: null }))]);
    const table = await grid();
    expect(table.textContent).toContain("<U+202E>");
    expect(table.textContent).not.toContain("‮");
    expect(table.textContent).toContain("[REDACTED: 16 chars, generic_secret]");
  });
});

describe("pages", () => {
  it("says how many are shown and asks for the next page by its cursor", async () => {
    const app = open("/search?type=settings&q=enableAccountSorting", searchBackend({ pageSize: 10 }));
    const table = await grid();
    expect(within(table).getAllByRole("row")).toHaveLength(11);
    expect(screen.getByText("10+ settings in 3 swimlanes")).toBeInTheDocument();
    expect(screen.getByText("Showing 10")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Load 100 more" }));
    await waitFor(() => expect(within(table).getAllByRole("row")).toHaveLength(18));
    expect(settingCalls(app.calls).map((p) => p.get("cursor"))).toEqual([null, "c10"]);
    expect(screen.getByText("17 settings in 6 swimlanes")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Load 100 more" })).toBeNull();
  });

  it("keeps the rows and says so when the next page fails, and tries it again", async () => {
    let fail = true;
    const backend = searchBackend({ pageSize: 10 });
    const routes: Route[] = [
      (call, url) => (call.method === "GET" && url.pathname === "/api/v1/search/settings" && url.searchParams.has("cursor") && fail ? problem(503, "unavailable") : undefined),
      ...backend,
    ];
    open("/search?type=settings&q=enableAccountSorting", routes);
    const table = await grid();
    await userEvent.click(screen.getByRole("button", { name: "Load 100 more" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't load more settings");
    expect(alert).toHaveTextContent("req-test-1");
    expect(within(table).getAllByRole("row")).toHaveLength(11);
    fail = false;
    await userEvent.click(within(alert).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(within(table).getAllByRole("row")).toHaveLength(18));
  });
});

describe("the query and the swimlanes", () => {
  it("asks nothing until there is a query", async () => {
    const app = open("/search?type=settings");
    expect(await screen.findByText("Search settings")).toBeInTheDocument();
    expect(screen.getByText(/Type a setting name, or a part of its path/)).toBeInTheDocument();
    expect(screen.getByText("Type to search")).toBeInTheDocument();
    expect(settingCalls(app.calls)).toEqual([]);
  });

  it("puts the query in the address and asks for it, once", async () => {
    const app = open("/search?type=settings");
    await screen.findByText("Search settings");
    await userEvent.type(query(), "timeoutMs");
    await waitFor(() => expect(app.location()).toBe("/search?q=timeoutMs&type=settings"));
    await grid();
    expect(settingCalls(app.calls).map((p) => p.get("q"))).toEqual(["timeoutMs"]);
  });

  it("does not ask for a query longer than the server takes", async () => {
    const app = open(`/search?type=settings&q=${"a".repeat(300)}`);
    expect((await screen.findAllByText("Query not searched")).length).toBeGreaterThan(0);
    expect(screen.getByText(/at most 256 characters/)).toBeInTheDocument();
    expect(settingCalls(app.calls)).toEqual([]);
  });

  it("sends the swimlanes that stay checked", async () => {
    const app = open("/search?type=settings&q=enableAccountSorting&lanes=sit,sitb");
    await grid();
    expect(settingCalls(app.calls)[0]?.get("swimlanes")).toBe("sit,sitb");
    expect(screen.getByRole("button", { name: /Swimlanes sit, sitb/ })).toBeInTheDocument();
  });

  it("carries the query from the Text tab, and keeps it when the tab is chosen", async () => {
    const app = open("/search?type=text&q=timeoutMs");
    await userEvent.click(await screen.findByRole("tab", { name: "Settings" }));
    await waitFor(() => expect(app.location()).toBe("/search?q=timeoutMs&type=settings"));
    await grid();
  });
});

describe("every state", () => {
  it("shows the table's own skeleton while the search runs, named and busy", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const api = hub("viewer", searchBackend());
    renderApp("/search?type=settings&q=timeoutMs", async (call) => {
      if (call.path.startsWith("/api/v1/search/settings")) await held;
      return api(call);
    });
    expect(await screen.findByLabelText("Loading settings")).toHaveAttribute("aria-busy", "true");
    release();
    await grid();
  });

  it("says what failed, with the request id, and tries again", async () => {
    let fail = true;
    const routes: Route[] = [at("GET", "/api/v1/search/settings", () => (fail ? problem(500, "internal") : json({ items: settingHitsFor("timeoutMs"), next: null })))];
    open("/search?type=settings&q=timeoutMs", routes);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't load settings");
    expect(alert).toHaveTextContent("req-test-1");
    fail = false;
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await grid();
  });

  it("says nothing matched, and offers file text and every swimlane", async () => {
    const app = open("/search?type=settings&q=enableAccountSortng&lanes=sitb");
    expect(await screen.findByText("No settings match “enableAccountSortng”")).toBeInTheDocument();
    expect(screen.getByText(/Searched 1 of 6 swimlanes\. Settings search matches setting paths, not values, comments or other text\./)).toBeInTheDocument();
    const box = screen.getByRole("region", { name: "Try instead" });
    expect(within(box).getByText("Search file text for “enableAccountSortng”")).toBeInTheDocument();
    expect(within(box).getByText("Finds comments and any other text")).toBeInTheDocument();
    await userEvent.click(within(box).getByRole("button", { name: "Widen" }));
    await waitFor(() => expect(app.location()).not.toContain("lanes="));
  });

  it("hops to the Text tab with the same query", async () => {
    const app = open("/search?type=settings&q=timeoutMs-nothing");
    const box = await screen.findByRole("region", { name: "Try instead" });
    await userEvent.click(within(box).getByRole("button", { name: "Open Text" }));
    await waitFor(() => expect(app.location()).toBe("/search?q=timeoutMs-nothing&type=text"));
  });
});

describe("the keyboard", () => {
  it("focuses the query on /", async () => {
    open("/search?type=settings&q=timeoutMs");
    await grid();
    await userEvent.keyboard("/");
    await waitFor(() => expect(query()).toHaveFocus());
  });
});

describe("structure for assistive technology", () => {
  it("has no structural violation with results and with no match", async () => {
    const app = open("/search?type=settings&q=enableAccountSorting");
    await grid();
    await expectNoAxeViolations();
    app.router.history.push("/search?type=settings&q=nothing-matches");
    await screen.findByText("No settings match “nothing-matches”");
    await expectNoAxeViolations();
  });
});
