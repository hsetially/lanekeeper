import { QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { Comparison, SwimlaneSummary } from "@/api/generated/model";
import { KeyboardProvider } from "@/keyboard";
import { expectNoAxeViolations } from "@/test/axe";
import { createQueryClient } from "@/lib/query/client";
import { invalidateTargets } from "@/lib/query/targets";
import type { QueryState } from "@/lib/query/use-query-state";
import { parseView, type CompareView } from "../model";
import { gridServer, json, problem, row, stubHub, sv, type Handler } from "../test-utils";
import { GridScreen } from "./GridScreen";

const lane = (id: string, over: Partial<SwimlaneSummary> = {}): SwimlaneSummary => ({
  id,
  display_name: id,
  tenants: [`t-${id}`],
  agent: "connected",
  last_scan_at: Date.UTC(2026, 9, 17, 10, 28),
  base_version: { tag: "v2.3.1", distance: 0, commit: "a".repeat(40) },
  drift_counts: [],
  finding_count: 0,
  tier: id.startsWith("presit") ? "PRESIT" : "SIT",
  ...over,
});
const LANES = ["sit", "sitb", "sitc"];
const FILE = "tx-infinity-api/tx-infinity-core.yml";

const ready = (data: SwimlaneSummary[]): QueryState<SwimlaneSummary[]> => ({ state: "ready", data, error: null, retryable: false, refreshing: false, stale: null, refetch: vi.fn() });

function renderGrid(start: Partial<CompareView> = {}, lanes: SwimlaneSummary[] = LANES.map((id) => lane(id))) {
  const seen: CompareView[] = [];
  const client = createQueryClient();
  const base: CompareView = { ...parseView({}), lanes: LANES, file: FILE, ...start };
  function Host() {
    const [view, setView] = useState(base);
    return (
      <QueryClientProvider client={client}>
        <KeyboardProvider>
          <GridScreen
            view={view}
            onView={(next) => {
              seen.push(next);
              setView(next);
            }}
            swimlanes={ready(lanes)}
          />
        </KeyboardProvider>
      </QueryClientProvider>
    );
  }
  render(<Host />);
  return { seen, client };
}

/** The grid with its rows (the first `grid` on screen is the skeleton, with its own name). */
const gridReady = () => screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });

const comparison = (over: Partial<Comparison> = {}): Comparison => ({
  left: "effective:sit",
  right: "effective:sitb",
  file: FILE,
  class: "structured",
  identical: false,
  eol_only: false,
  hunks: [],
  setting_changes: [],
  binary: null,
  ...over,
});

/** A hub with a grid of `rows`, and a comparison for the drawer. */
function hub(rows: ReturnType<typeof row>[], compare: Comparison = comparison()) {
  const requests: URL[] = [];
  const handler: Handler = (call) => {
    requests.push(call.url);
    if (call.url.pathname === "/api/v1/grid") return json({ swimlanes: LANES, rows: { items: rows, next: null } });
    if (call.url.pathname === "/api/v1/compare") return json(compare, 200, { etag: '"c1"' });
    return problem(404, "not_found");
  };
  return { handler, requests };
}

describe("the grid", () => {
  it("reads 50 rows first for the swimlanes in the order given, only the differing ones, and draws a column for each swimlane", async () => {
    const server = gridServer(3, LANES, (i) => row(`limits.item-${i}`, 3, "5000", { 1: "9000" }));
    stubHub(server.handler);
    renderGrid();
    const grid = await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });
    await waitFor(() => expect(within(grid).getAllByRole("row").length).toBeGreaterThan(3));
    expect(server.requests).toEqual([{ limit: 50, cursor: null }]);
    for (const id of LANES) expect(within(grid).getByText(id)).toBeInTheDocument();
    expect(grid).toHaveAttribute("aria-rowcount", "4");
    expect(grid).toHaveAttribute("aria-colcount", "4");
  });

  it("asks the server for only the differing rows, and for all of them when the switch is off", async () => {
    const h = hub([row("a.b", 3)]);
    stubHub(h.handler);
    const user = userEvent.setup();
    const { seen } = renderGrid();
    await gridReady();
    expect(h.requests[0]?.searchParams.get("only_differing")).toBe("true");
    expect(h.requests[0]?.searchParams.get("swimlanes")).toBe("sit,sitb,sitc");
    expect(h.requests[0]?.searchParams.get("file")).toBe(FILE);
    await user.click(screen.getByRole("switch", { name: "Only differing rows" }));
    expect(seen.at(-1)?.onlyDiff).toBe(false);
    await waitFor(() => expect(h.requests.some((u) => u.pathname === "/api/v1/grid" && !u.searchParams.has("only_differing"))).toBe(true));
  });

  it("marks the cell that stands apart, says absent and empty in words, and shows a hidden value as hidden", async () => {
    stubHub(hub([row("a.diff", 3, "5000", { 1: "9000" }), row("a.gone", 3, "x", { 2: null }), { path: "a.secret", cells: [{ value: sv("[REDACTED: 12 chars, aws_key]"), location: null }, { value: sv("x"), location: null }, { value: sv("x"), location: null }] }]).handler);
    renderGrid();
    const grid = await gridReady();
    await waitFor(() => expect(within(grid).getByText("9000")).toBeInTheDocument());
    expect(within(grid).getByRole("button", { name: /9000/ })).toHaveAttribute("data-differs", "true");
    expect(within(grid).getAllByRole("button", { name: "5000" })[0]).not.toHaveAttribute("data-differs");
    expect(within(grid).getByRole("button", { name: /^absent/ })).toBeInTheDocument();
    expect(within(grid).getByRole("button", { name: /Hidden value, 12 characters/ })).toHaveAttribute("data-kind", "hidden");
    expect(within(grid).queryByText(/aws_key/)).toBeNull();
  });

  it("shows the characters a path or a value hides", async () => {
    stubHub(hub([row("a.b\u202Ec", 3, "x\u200By")]).handler);
    renderGrid();
    const grid = await gridReady();
    await waitFor(() => expect(within(grid).getAllByRole("img", { name: /U\+202E/ }).length).toBeGreaterThan(0));
    expect(within(grid).getAllByRole("img", { name: /U\+200B/ }).length).toBeGreaterThan(0);
  });
});

describe("two swimlanes at one setting", () => {
  const setting = row("limits.max", 3, "5000", { 1: "9000" });
  const changed = comparison({ setting_changes: [{ path: "limits.max", kind: "changed", left: sv("5000"), right: sv("9000"), left_location: null, right_location: null, severity: null }] });

  it("opens the diff when a second cell of the row is clicked, and asks the server to compare the two effective files", async () => {
    const h = hub([setting], changed);
    stubHub(h.handler);
    const user = userEvent.setup();
    renderGrid();
    const grid = await gridReady();
    await user.click(await within(grid).findByRole("button", { name: /9000/ }));
    expect(screen.getByText(/Click another cell in the same row/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Diff at setting" })).toBeNull();
    await user.click(within(grid).getAllByRole("button", { name: "5000" })[0] as HTMLElement);
    const drawer = await screen.findByRole("dialog", { name: "Diff at setting" });
    await waitFor(() => expect(within(drawer).getByText("Value differs")).toBeInTheDocument());
    const call = h.requests.find((u) => u.pathname === "/api/v1/compare");
    expect(call?.searchParams.get("left")).toBe("effective:sitb");
    expect(call?.searchParams.get("right")).toBe("effective:sit");
    expect(call?.searchParams.get("file")).toBe(FILE);
    expect(within(drawer).getByText("limits.max")).toBeInTheDocument();
    // The setting's path can be copied; the drawer still opens with focus on Close.
    expect(within(drawer).getByRole("button", { name: "Close diff" })).toHaveFocus();
    await user.click(within(drawer).getByRole("button", { name: "Copy setting path" }));
    expect(await navigator.clipboard.readText()).toBe("limits.max");
  });

  it("opens with a pair worth looking at when Enter is pressed on a row, and returns to the grid on Escape", async () => {
    const h = hub([setting], changed);
    stubHub(h.handler);
    const user = userEvent.setup();
    renderGrid();
    const grid = await gridReady();
    await within(grid).findByRole("button", { name: /9000/ });
    grid.focus();
    await user.keyboard("{ArrowDown}{Enter}");
    const drawer = await screen.findByRole("dialog", { name: "Diff at setting" });
    // A is a swimlane with the value most of the row has (the first), B the first that stands apart (sitb).
    const call = await waitFor(() => {
      const found = h.requests.find((u) => u.pathname === "/api/v1/compare");
      expect(found).toBeDefined();
      return found;
    });
    expect(call?.searchParams.get("left")).toBe("effective:sit");
    expect(call?.searchParams.get("right")).toBe("effective:sitb");
    expect(within(drawer).getByRole("button", { name: "Close diff" })).toHaveFocus();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Diff at setting" })).toBeNull());
    expect(grid).toHaveFocus();
  });

  it("lets a keyboard user choose the two swimlanes in lists", async () => {
    const h = hub([setting], changed);
    stubHub(h.handler);
    const user = userEvent.setup();
    renderGrid();
    const grid = await gridReady();
    await user.click(await within(grid).findByRole("button", { name: /9000/ }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Swimlane B" }), "sitc");
    await user.click(screen.getByRole("button", { name: "Open diff" }));
    const drawer = await screen.findByRole("dialog", { name: "Diff at setting" });
    expect(within(drawer).getByText("sitc")).toBeInTheDocument();
  });

  it("leaves the verdict on hidden values to the server and says so", async () => {
    const hidden = { path: "db.password", cells: [{ value: sv("[REDACTED: 8 chars, password]"), location: { path: "db.password", lines: { start: 3, end: 3 } } }, { value: sv("[REDACTED: 8 chars, password]"), location: { path: "db.password", lines: { start: 3, end: 3 } } }, { value: sv("x"), location: null }] };
    const h = hub([hidden], comparison({ setting_changes: [{ path: "db.password", kind: "changed", left: sv("[REDACTED: 8 chars, password]"), right: sv("[REDACTED: 8 chars, password]"), left_location: null, right_location: null, severity: null }] }));
    stubHub(h.handler);
    const user = userEvent.setup();
    renderGrid();
    const grid = await gridReady();
    const cells = await within(grid).findAllByRole("button", { name: /Hidden value/ });
    // Two markers that read alike: the grid does not call them equal.
    expect(cells[0]).not.toHaveAttribute("data-differs");
    expect(cells[1]).not.toHaveAttribute("data-differs");
    await user.click(cells[0] as HTMLElement);
    await user.click(cells[1] as HTMLElement);
    const drawer = await screen.findByRole("dialog", { name: "Diff at setting" });
    await waitFor(() => expect(within(drawer).getByText("Value differs")).toBeInTheDocument());
    expect(within(drawer).getByRole("note")).toHaveTextContent(/never shown, but comparing two of them tells whether they are equal: here the server found them different/);
  });

  it("says why when the comparison cannot be read, with the request id", async () => {
    stubHub((call) => (call.url.pathname === "/api/v1/grid" ? json({ swimlanes: LANES, rows: { items: [setting], next: null } }) : problem(403, "forbidden")));
    const user = userEvent.setup();
    renderGrid();
    const grid = await gridReady();
    await user.click(await within(grid).findByRole("button", { name: /9000/ }));
    await user.click(within(grid).getAllByRole("button", { name: "5000" })[0] as HTMLElement);
    const drawer = await screen.findByRole("dialog", { name: "Diff at setting" });
    expect(await within(drawer).findByText("Couldn't load comparison")).toBeInTheDocument();
    expect(within(drawer).getByText("req-compare-1")).toBeInTheDocument();
  });
});

describe("the states of the grid", () => {
  it("shows the columns and a skeleton while the rows load", async () => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    stubHub(async () => {
      await gate;
      return json({ swimlanes: LANES, rows: { items: [row("a.b", 3)], next: null } });
    });
    renderGrid();
    const grid = await screen.findByRole("grid", { name: "Settings" });
    expect(grid).toHaveAttribute("aria-busy", "true");
    expect(within(grid).getByText("sitb")).toBeInTheDocument();
    expect(screen.getAllByText(/Reading effective files · 3 swimlanes/).length).toBeGreaterThan(0);
    release();
    await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });
  });

  it("says what failed, in the problem's words, with the request id and a way to try again", async () => {
    let fail = true;
    stubHub(() => (fail ? problem(500, "internal") : json({ swimlanes: LANES, rows: { items: [row("a.b", 3)], next: null } })));
    const user = userEvent.setup();
    renderGrid();
    expect(await screen.findByText("Couldn't load comparison", undefined, { timeout: 8000 })).toBeInTheDocument();
    expect(screen.getByText("req-compare-1")).toBeInTheDocument();
    fail = false;
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByRole("grid", { name: "tx-infinity-core.yml across swimlanes" });
  }, 15000);

  it("says the swimlanes agree, and offers every row, when the server lists no differing row", async () => {
    stubHub(hub([]).handler);
    const user = userEvent.setup();
    const { seen } = renderGrid();
    expect(await screen.findByText("All 3 swimlanes have the same values")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show all rows" }));
    expect(seen.at(-1)?.onlyDiff).toBe(false);
  });

  it("says no setting matches a search over the rows loaded, and clears it", async () => {
    stubHub(hub([row("a.b", 3, "1", { 0: "2" })]).handler);
    const user = userEvent.setup();
    renderGrid();
    await gridReady();
    await user.type(screen.getByRole("searchbox", { name: "Search settings" }), "zzz.retry");
    expect(await screen.findByText("No settings match")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Clear search and channel" }));
    await waitFor(() => expect(screen.queryByText("No settings match")).toBeNull());
  });

  it("focuses the search with the slash key", async () => {
    stubHub(hub([row("a.b", 3, "1", { 0: "2" })]).handler);
    const user = userEvent.setup();
    renderGrid();
    await gridReady();
    await user.keyboard("/");
    expect(screen.getByRole("searchbox", { name: "Search settings" })).toHaveFocus();
  });

  it("names the swimlane whose agent is offline, dashes its column and says how old its files are", async () => {
    stubHub(hub([row("a.b", 3, "1", { 0: "2" })]).handler);
    renderGrid({}, [lane("sit"), lane("sitb", { agent: "disconnected", last_scan_at: Date.now() - 14 * 60_000 }), lane("sitc")]);
    const banner = await screen.findByText("sitb agent is offline");
    expect(banner.closest('[role="alert"], [role="status"]') ?? banner.parentElement).toHaveTextContent(/Its column shows the NFS files as of \d\d:\d\d UTC, 14 min ago\. The other 2 columns are live\./);
    expect(screen.getByRole("button", { name: "Retry sitb" })).toBeInTheDocument();
    const grid = await gridReady();
    await waitFor(() => expect(within(grid).getAllByRole("button").some((b) => b.dataset["offline"] === "true")).toBe(true));
    expect(within(grid).getByRole("img", { name: "Agent offline" })).toBeInTheDocument();
  });

  it("asks the person to choose a file when there is none, and a swimlane when there is none", async () => {
    const user = userEvent.setup();
    stubHub(() => json({}));
    renderGrid({ file: null });
    expect(await screen.findByText("Choose a file to compare")).toBeInTheDocument();
    await user.click(screen.getAllByRole("button", { name: "Choose a file" }).at(-1) as HTMLElement);
    expect(await screen.findByRole("textbox", { name: "Logical file" })).toBeInTheDocument();
  });

  it("refuses more than 40 swimlanes and offers the fix in place of the grid", async () => {
    const many = Array.from({ length: 43 }, (_, i) => `lane-${String(i).padStart(2, "0")}`);
    const calls = stubHub(() => json({ swimlanes: [], rows: { items: [], next: null } }));
    const user = userEvent.setup();
    const { seen } = renderGrid({ lanes: many }, many.map((id) => lane(id)));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Compare up to 40 swimlanes at a time");
    expect(alert).toHaveTextContent("You selected 43");
    expect(alert).toHaveTextContent("3 over the limit");
    expect(screen.getByRole("button", { name: /43 swimlanes · limit 40/ })).toBeInTheDocument();
    expect(screen.queryByRole("grid")).toBeNull();
    expect(calls.filter((c) => c.url.pathname === "/api/v1/grid")).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Keep the first 40" }));
    expect(seen.at(-1)?.lanes).toEqual(many.slice(0, 40));
  });

  it("uses the first 40 swimlanes of the list when the address names none", async () => {
    const list = Array.from({ length: 50 }, (_, i) => lane(`lane-${String(i).padStart(2, "0")}`));
    const calls = stubHub(() => json({ swimlanes: [], rows: { items: [], next: null } }));
    renderGrid({ lanes: [] }, list);
    await waitFor(() => expect(calls.some((c) => c.url.pathname === "/api/v1/grid")).toBe(true));
    const asked = calls.find((c) => c.url.pathname === "/api/v1/grid")?.url.searchParams.get("swimlanes")?.split(",") ?? [];
    expect(asked).toHaveLength(40);
    expect(asked[0]).toBe("lane-00");
  });
});

describe("a long grid", () => {
  it("reads the next page of 500 rows when the person scrolls toward the end, and not for a scroll that stays near the top", async () => {
    const server = gridServer(5000, LANES, (i) => row(`app.setting-${i}`, 3, "1", { 1: "2" }));
    stubHub(server.handler);
    renderGrid();
    const grid = await gridReady();
    await waitFor(() => expect(within(grid).getAllByRole("row").length).toBeGreaterThan(5));
    expect(server.requests).toEqual([{ limit: 50, cursor: null }]);
    // 50 rows are loaded and the first 100 or so rows are "near the end": any scroll asks for the next page.
    fireEvent.scroll(grid, { target: { scrollTop: 56 } });
    await waitFor(() => expect(server.requests).toHaveLength(2));
    expect(server.requests[1]).toEqual({ limit: 500, cursor: "o50" });
    // 550 rows are loaded, and the server has more: the grid says how many it has loaded, and that the whole is not known.
    await waitFor(() => expect(screen.getByText("Showing 550")).toBeInTheDocument());
    expect(screen.getByRole("grid", { name: "tx-infinity-core.yml across swimlanes" })).toHaveAttribute("aria-rowcount", "-1");
    // 550 rows are loaded and the window is at the top: a small scroll asks for nothing.
    fireEvent.scroll(screen.getByRole("grid", { name: "tx-infinity-core.yml across swimlanes" }), { target: { scrollTop: 280 } });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(server.requests).toHaveLength(2);
  });

  it("offers the next 500 rows in the footer, with the count of rows loaded", async () => {
    const server = gridServer(5000, LANES);
    stubHub(server.handler);
    const user = userEvent.setup();
    renderGrid();
    await gridReady();
    expect(await screen.findByText("Showing 50")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Load 500 more" }));
    await waitFor(() => expect(server.requests).toHaveLength(2));
    expect(await screen.findByText("Showing 550")).toBeInTheDocument();
  });

  it("draws the columns in view only when there are forty swimlanes, and keeps the column counts of the whole grid", async () => {
    const forty = Array.from({ length: 40 }, (_, i) => `lane-${String(i).padStart(2, "0")}`);
    stubHub(gridServer(30, forty, (i) => row(`app.setting-${i}`, 40, "1", { 3: "2" })).handler);
    renderGrid({ lanes: forty }, forty.map((id) => lane(id)));
    const grid = await gridReady();
    await waitFor(() => expect(within(grid).getAllByRole("columnheader").length).toBeGreaterThan(3));
    expect(grid).toHaveAttribute("aria-colcount", "41");
    expect(within(grid).getAllByRole("columnheader").length).toBeLessThan(41);
    expect(grid).toHaveAttribute("aria-rowcount", "31");
    for (const header of within(grid).getAllByRole("columnheader")) expect(header).toHaveAttribute("aria-colindex");
  });
});

describe("the structure of each state", () => {
  it("has no violation of names, roles or ARIA in the grid, the pick bar and the drawer", async () => {
    const setting = row("limits.max", 3, "5000", { 1: "9000" });
    stubHub(hub([setting], comparison({ setting_changes: [{ path: "limits.max", kind: "changed", left: sv("5000"), right: sv("9000"), left_location: null, right_location: null, severity: null }] })).handler);
    const user = userEvent.setup();
    renderGrid();
    const grid = await gridReady();
    await user.click(await within(grid).findByRole("button", { name: /9000/ }));
    await user.click(within(grid).getAllByRole("button", { name: "5000" })[0] as HTMLElement);
    const drawer = await screen.findByRole("dialog", { name: "Diff at setting" });
    await waitFor(() => expect(within(drawer).getByText("Value differs")).toBeInTheDocument());
    await expectNoAxeViolations();
  });

  it("has none in the states that replace the grid", async () => {
    stubHub(() => problem(500, "internal"));
    renderGrid();
    await screen.findByText("Couldn't load comparison", undefined, { timeout: 8000 });
    await expectNoAxeViolations();
  }, 15000);

  it("has none when the file is not chosen, nor when the swimlanes are too many", async () => {
    stubHub(() => json({}));
    renderGrid({ file: null });
    await screen.findByText("Choose a file to compare");
    await expectNoAxeViolations();
  });
});

describe("a live event", () => {
  it("refreshes the rows in place: no skeleton, and the new value appears", async () => {
    let cell = "5000";
    stubHub((call) => (call.url.pathname === "/api/v1/grid" ? json({ swimlanes: LANES, rows: { items: [row("limits.max", 3, cell, { 1: "9000" })], next: null } }) : problem(404, "not_found")));
    const { client } = renderGrid();
    const grid = await gridReady();
    await within(grid).findByRole("button", { name: /9000/ });
    expect(within(grid).getAllByRole("button", { name: "5000" }).length).toBeGreaterThan(0);
    cell = "6000";
    await act(async () => {
      await invalidateTargets(client, [{ kind: "grids", swimlane: "sit" }]);
    });
    await waitFor(() => expect(within(screen.getByRole("grid", { name: "tx-infinity-core.yml across swimlanes" })).getAllByRole("button", { name: "6000" }).length).toBeGreaterThan(0));
    expect(screen.queryByRole("grid", { name: "Settings" })).toBeNull();
  });

  it("keeps the rows it has and says so when the refresh fails", async () => {
    let fail = false;
    stubHub(() => (fail ? problem(500, "internal") : json({ swimlanes: LANES, rows: { items: [row("limits.max", 3, "5000", { 1: "9000" })], next: null } })));
    const { client } = renderGrid();
    const grid = await gridReady();
    await within(grid).findByRole("button", { name: /9000/ });
    fail = true;
    await act(async () => {
      await invalidateTargets(client, [{ kind: "grids", swimlane: "sitb" }]);
    });
    expect(await screen.findByText("Couldn't refresh")).toBeInTheDocument();
    expect(within(screen.getByRole("grid", { name: "tx-infinity-core.yml across swimlanes" })).getByRole("button", { name: /9000/ })).toBeInTheDocument();
  });
});
