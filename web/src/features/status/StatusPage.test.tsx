import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { FlagStatus } from "@/api/generated/model";
import { FLAGS } from "@/mocks/design/search/data";
import { at, featureBackend, hub, type Route } from "@/mocks/design/search/hub";
import { json, problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Feature status: the documented flags against the swimlanes. One row for each flag, a cell for each swimlane, the cells that
// differ from the template default marked with an icon and a word as well as a colour, the channels under a flag, and the
// swimlanes whose agent is offline shown as last known and dimmed.

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const open = (url = "/status", routes: Route[] = featureBackend(), role = "viewer") => renderApp(url, hub(role, routes));

async function table(): Promise<HTMLElement> {
  await screen.findByRole("grid", { name: "Feature flags" });
  await waitFor(() => expect(screen.getByRole("grid", { name: "Feature flags" })).not.toHaveAttribute("aria-busy", "true"));
  return screen.getByRole("grid", { name: "Feature flags" });
}
const LANES = ["sit", "sitb", "sitc", "presita", "presitb", "presitc"];
const rowOf = (grid: HTMLElement, name: string): HTMLElement => within(grid).getByText(name).closest("[role=row]") as HTMLElement;
const cellOf = (row: HTMLElement, lane: string): HTMLElement => within(row).getAllByRole("gridcell")[3 + LANES.indexOf(lane)] as HTMLElement;

describe("the flags", () => {
  it("has one row for each flag and a column for each swimlane, and says how many differ", async () => {
    open();
    const grid = await table();
    expect(screen.getByRole("heading", { level: 1, name: "Feature status" })).toBeInTheDocument();
    expect(screen.getByText("8 documented flags across 6 swimlanes · 7 differ from the template default in at least one swimlane")).toBeInTheDocument();
    expect(screen.getByText("Showing 8 of 8 flags")).toBeInTheDocument();
    const headers = within(grid).getAllByRole("columnheader");
    expect(headers.map((h) => h.textContent)).toEqual([
      "Flag",
      "Doc",
      "Default",
      "sit1 differ",
      "sitb3 differ",
      "sitc1 differ",
      "presita3 differ",
      "presitblast known",
      "presitc1 differ",
    ]);
    expect(within(grid).getAllByRole("row")).toHaveLength(9);
  });

  it("shows the value of a flag in each swimlane, and marks a difference with an icon and a word", async () => {
    open();
    const grid = await table();
    const instant = rowOf(grid, "enableInstantPayments");
    expect(instant).toHaveTextContent("false");
    const sitb = within(cellOf(instant, "sitb")).getByRole("link");
    expect(sitb).toHaveTextContent("true");
    expect(sitb).toHaveAttribute("data-differs", "true");
    expect(sitb.querySelector("svg")).not.toBeNull();
    expect(sitb).toHaveAttribute("title", expect.stringContaining("template default false"));
    const sit = within(cellOf(instant, "sit")).getByRole("link");
    expect(sit).toHaveTextContent("false");
    expect(sit).not.toHaveAttribute("data-differs");
  });

  it("wraps the toolbar when its words do not fit, and lets the second line of a swimlane's header (`3 differ`, `last known`) end in an ellipsis (the layout run, labels 40% longer)", async () => {
    open();
    const grid = await table();
    const bar = screen.getByRole("searchbox", { name: "Filter flags" }).closest("[class*='border-b']");
    expect(bar?.className).toMatch(/(^|\s)flex-wrap(\s|$)/);
    expect(bar?.className).toMatch(/(^|\s)min-h-10(\s|$)/);
    expect(bar?.className).not.toMatch(/(^|\s)h-10(\s|$)/);
    const seconds = within(grid).getAllByText(/^\d+ differ$|^last known$/, { selector: "span.truncate" });
    expect(seconds).toHaveLength(6);
  });

  it("shows `mixed` for a flag whose channels disagree in a swimlane", async () => {
    open();
    const grid = await table();
    const sorting = rowOf(grid, "enableAccountSorting");
    expect(cellOf(sorting, "sit")).toHaveTextContent("mixed");
    expect(cellOf(sorting, "sitc")).toHaveTextContent("true");
    expect(cellOf(sorting, "presita")).toHaveTextContent("false");
  });

  it("links a value to the grid for the flag's file, and the flag to a search of the docs", async () => {
    open();
    const grid = await table();
    const instant = rowOf(grid, "enableInstantPayments");
    expect(within(cellOf(instant, "sitb")).getByRole("link")).toHaveAttribute("href", "/compare?file=tx-infinity-api%2Ftx-infinity-core.yml&lanes=sit%2Csitb%2Csitc%2Cpresita%2Cpresitb%2Cpresitc");
    expect(within(instant).getByRole("link", { name: "Docs" })).toHaveAttribute("href", "/search?q=enableInstantPayments&type=docs");
  });

  it("opens the channels of a flag, and closes them", async () => {
    open();
    const grid = await table();
    const toggle = within(grid).getByRole("button", { name: "Toggle channels of enableAccountSorting" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const channel = within(grid).getByText("remote-itm-teller").closest("[role=row]") as HTMLElement;
    expect(within(channel).getAllByRole("gridcell")[3 + 0]).toHaveTextContent("true");
    // A channel row sits under its flag in the muted colour (the design's), a flag row on the page's.
    expect(channel.firstElementChild).toHaveClass("bg-muted");
    expect((within(grid).getByText("enableAccountSorting").closest("[role=row]") as HTMLElement).firstElementChild).not.toHaveClass("bg-muted");
    expect(within(grid).getAllByRole("row")).toHaveLength(11);
    await userEvent.click(toggle);
    expect(within(grid).queryByText("remote-itm-teller")).toBeNull();
  });

  it("opens every channel from Show channels and closes them from Hide channels", async () => {
    open();
    const grid = await table();
    await userEvent.click(screen.getByRole("button", { name: "Show channels" }));
    expect(within(grid).getAllByRole("row")).toHaveLength(9 + 2 + 2 + 2 + 1);
    await userEvent.click(screen.getByRole("button", { name: "Hide channels" }));
    expect(within(grid).getAllByRole("row")).toHaveLength(9);
  });

  it("shows only the flags that differ when asked, and says how many are shown", async () => {
    open();
    const grid = await table();
    const toggle = screen.getByRole("switch", { name: "Only flags that differ" });
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(within(grid).queryByText("enableAuditTrail")).toBeNull();
    expect(screen.getByText("Showing 7 of 8 flags")).toBeInTheDocument();
  });

  it("filters by what is typed, as plain text", async () => {
    open();
    const grid = await table();
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter flags" }), "rate");
    expect(within(grid).getAllByRole("row")).toHaveLength(2);
    expect(screen.getByText("Showing 1 of 8 flags")).toBeInTheDocument();
    await userEvent.clear(screen.getByRole("searchbox", { name: "Filter flags" }));
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter flags" }), ".*");
    expect(screen.getByText("Showing 0 of 8 flags")).toBeInTheDocument();
  });

  it("shows a masked value and a hidden character as they are", async () => {
    const flags: FlagStatus[] = [
      { flag: "enable‮x", file: "a/b.yml", channel: null, setting: "x", template_default: { text: "false", ty: "bool", flag: null }, values: [{ swimlane: "sit", value: { text: "[REDACTED: 8 chars, generic_secret]", ty: "string", flag: null } }] },
    ];
    open("/status", featureBackend(flags));
    const grid = await table();
    expect(grid.textContent).toContain("<U+202E>");
    expect(grid.textContent).toContain("[REDACTED: 8 chars, generic_secret]");
  });

  it("shows a dash, with the reason, where a swimlane has no such key", async () => {
    const flags: FlagStatus[] = [{ flag: "enableX", file: "a/b.yml", channel: null, setting: "x", template_default: { text: "false", ty: "bool", flag: null }, values: [{ swimlane: "sit", value: null }] }];
    open("/status", featureBackend(flags));
    const grid = await table();
    const dash = within(cellOf(rowOf(grid, "enableX"), "sit")).getByText("—");
    expect(dash).toHaveAttribute("title", "Key not found in this swimlane");
  });
});

describe("agents that are offline", () => {
  it("says which, dims what they last reported, and says the values are the last known", async () => {
    open();
    const grid = await table();
    const note = screen.getByText("Agent for presitb is offline").closest("[role=status]");
    expect(note).toHaveTextContent("Showing the last known values for presitb.");
    expect(within(note as HTMLElement).getByRole("link", { name: "Check agent" })).toHaveAttribute("href", "/swimlanes/presitb");
    expect(cellOf(rowOf(grid, "enableAuditTrail"), "presitb").querySelector("[data-offline]")).not.toBeNull();
    expect(cellOf(rowOf(grid, "enableAuditTrail"), "sit").querySelector("[data-offline]")).toBeNull();
    expect(within(grid).getByRole("img", { name: "Agent offline" })).toBeInTheDocument();
    expect(within(grid).getAllByRole("img", { name: "Agent online" })).toHaveLength(5);
  });
});

describe("what an offline agent last reported is marked without fading it", () => {
  it("uses italics, not opacity: a faded value is under 4.5:1 (axe `color-contrast` found 2.71 on a differing value)", async () => {
    open();
    const grid = await table();
    const cells = [...grid.querySelectorAll("[data-offline]")];
    expect(cells.length).toBeGreaterThan(0);
    for (const cell of cells) {
      expect(cell.className).not.toMatch(/(^|\s)opacity-/);
      expect(cell.className).toMatch(/(^|\s)italic(\s|$)/);
    }
  });
});

describe("every state", () => {
  it("has its title at once and a skeleton while it loads, named and busy", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const api = hub("viewer", featureBackend());
    renderApp("/status", async (call) => {
      if (call.path.startsWith("/api/v1/features/status")) await held;
      return api(call);
    });
    expect(await screen.findByRole("heading", { level: 1, name: "Feature status" })).toBeInTheDocument();
    expect(await screen.findByLabelText("Loading feature flags")).toHaveAttribute("aria-busy", "true");
    release();
    await table();
  });

  it("says what failed, with the request id, and tries again", async () => {
    let fail = true;
    open("/status", [at("GET", "/api/v1/features/status", () => (fail ? problem(500, "internal") : json({ flags: FLAGS })))]);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't load feature flags");
    expect(alert).toHaveTextContent("req-test-1");
    fail = false;
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await table();
  });

  it("says when no flag is documented", async () => {
    open("/status", [at("GET", "/api/v1/features/status", () => json({ flags: [] }))]);
    expect(await screen.findByText("No feature flags found")).toBeInTheDocument();
    expect(screen.getByText(/A flag appears here once a doc has a Feature Flags table that names a setting/)).toBeInTheDocument();
  });
});

describe("a link to one setting", () => {
  it("says which setting and swimlane it is about, with the value, and opens the flag's channels", async () => {
    open("/status?setting=atm-iso.enableAccountSorting&lane=sitc");
    const grid = await table();
    const note = screen.getByRole("note");
    expect(note).toHaveTextContent("atm-iso.enableAccountSorting");
    expect(note).toHaveTextContent("sitc");
    expect(note).toHaveTextContent("true");
    expect(note).toHaveTextContent("template default false");
    expect(within(grid).getByRole("button", { name: "Toggle channels of enableAccountSorting" })).toHaveAttribute("aria-expanded", "true");
    expect(within(note).getByRole("link", { name: "Show all flags" })).toHaveAttribute("href", "/status");
  });

  it("lets the setting's path and its value be copied from the note", async () => {
    const user = userEvent.setup();
    open("/status?setting=atm-iso.enableAccountSorting&lane=sitc");
    await table();
    const note = screen.getByRole("note");
    await user.click(within(note).getByRole("button", { name: "Copy setting path" }));
    expect(await navigator.clipboard.readText()).toBe("atm-iso.enableAccountSorting");
    await user.click(within(note).getByRole("button", { name: "Copy value" }));
    expect(await navigator.clipboard.readText()).toBe("true");
  });

  it("says when the setting is not one of the documented flags", async () => {
    open("/status?setting=nothing.here&lane=sit");
    await table();
    expect(screen.getByRole("note")).toHaveTextContent("is not a documented flag");
  });
});

describe("the keyboard", () => {
  it("focuses the filter on /", async () => {
    open();
    await table();
    await userEvent.keyboard("/");
    await waitFor(() => expect(screen.getByRole("searchbox", { name: "Filter flags" })).toHaveFocus());
  });
});

describe("structure for assistive technology", () => {
  it("has no structural violation with the flags, with the channels open and with a link to one setting", async () => {
    const app = open();
    await table();
    await expectNoAxeViolations();
    await userEvent.click(screen.getByRole("button", { name: "Show channels" }));
    await expectNoAxeViolations();
    app.router.history.push("/status?setting=atm-iso.enableAccountSorting&lane=sitc");
    await screen.findByRole("note");
    await expectNoAxeViolations();
  });
});
