import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { sentinel } from "@/mocks/design/admin-t5/fixtures";
import { FIRST, installAdminClock, openSection, rowIn } from "@/mocks/design/admin-t5/harness";
import { newT5World, t5Api } from "@/mocks/design/admin-t5/world";
import { problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// Sentinels (route /admin/sentinels, Admin only): the agent on each NFS VM that records who edits files. The contract gives a list and
// nothing else: a host, a version, a status, the time it was last seen and the records waiting on the VM. There is no operation to
// register, switch off or configure one, no swimlane or last-record time (plan CG15) and no limit for being silent, so the page is
// read only and the gap it shows is the hub's status, never a threshold of its own. The last-seen time is the last hello or batch of
// records, not a heartbeat (D123), so it is called "Last seen".

installAdminClock();
const grid = () => screen.findByRole("grid", { name: "Sentinels" });

describe("the sentinels", () => {
  it("lists each VM with its version, when it was last seen, its backlog and its status", async () => {
    await openSection(newT5World(), "/admin/sentinels");
    expect(await screen.findByRole("heading", { level: 2, name: "Sentinels" })).toBeInTheDocument();
    expect(await screen.findByText("6 registered · 1 disconnected")).toBeInTheDocument();
    const sitb = await rowIn(await grid(), "nfs-sitb-01");
    const cells = within(sitb).getAllByRole("gridcell");
    expect(cells[1]).toHaveTextContent("v0.9.4");
    expect(cells[2]).toHaveTextContent("just now");
    expect(cells[3]).toHaveTextContent("2 records");
    expect(cells[4]).toHaveTextContent("Connected");
    const old = await rowIn(await grid(), "nfs-sitc-01");
    expect(within(old).getAllByRole("gridcell")[1]).toHaveTextContent("v0.9.3");
    expect(within(old).getAllByRole("gridcell")[3]).toHaveTextContent("None");
  });

  it("names the disconnected VM, says what that costs, and shows when it was last seen", async () => {
    await openSection(newT5World(), "/admin/sentinels");
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("nfs-presitb-01 is disconnected");
    expect(alert).toHaveTextContent("Last seen 14 min ago. Edits made on this VM can't be attributed to a person until it connects again.");
    const gap = await rowIn(await grid(), "nfs-presitb-01");
    const cells = within(gap).getAllByRole("gridcell");
    expect(cells[2]).toHaveTextContent("14 min ago");
    expect(within(cells[2] as HTMLElement).getByText("14 min ago")).toHaveAttribute("data-gap", "true");
    expect(cells[3]).toHaveTextContent("38 records");
    expect(cells[4]).toHaveTextContent("Disconnected");
  });

  // The status is one badge in a cell of one line. With the words 40% longer "Disconnected" is wider than the cell, and a badge cut off by
  // its cell loses its end (found by the layout run with the labels stretched): the word gives way with an ellipsis instead.
  it("lets the status word give way with an ellipsis when the cell is narrow, and never cuts the badge off", async () => {
    await openSection(newT5World(), "/admin/sentinels");
    const gap = await rowIn(await grid(), "nfs-presitb-01");
    const badge = within(gap).getByText("Disconnected").closest<HTMLElement>("[data-slot='sentinel-status']");
    expect(badge?.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    expect(within(gap).getByText("Disconnected").className).toMatch(/(^|\s)truncate(\s|$)/);
  });

  it("names every silent VM when there are several", async () => {
    const world = newT5World();
    world.sentinels = world.sentinels.map((s) => (s.host === "nfs-sit-01" ? { ...s, status: "disconnected" as const } : s));
    await openSection(world, "/admin/sentinels");
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("2 sentinels are disconnected")).toBeInTheDocument();
    expect(alert).toHaveTextContent("nfs-sit-01 (last seen just now), nfs-presitb-01 (last seen 14 min ago). Edits made on these VMs can't be attributed to a person until they connect again.");
  });

  it("raises no alert when every sentinel is connected, and one that was never seen is not a gap", async () => {
    const world = newT5World();
    world.sentinels = [sentinel("nfs-a-01"), sentinel("nfs-b-01", { status: "never_seen", last_seen_at: null, version: "v0.9.4" })];
    await openSection(world, "/admin/sentinels");
    const never = await rowIn(await grid(), "nfs-b-01");
    expect(within(never).getAllByRole("gridcell")[2]).toHaveTextContent("Never");
    expect(within(never).getAllByRole("gridcell")[4]).toHaveTextContent("Never seen");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(await screen.findByText("2 registered · 1 never seen")).toBeInTheDocument();
  });

  it("is read only: the contract has nothing to register, switch off or configure a sentinel, and the page says so", async () => {
    const app = await openSection(newT5World(), "/admin/sentinels");
    await grid();
    const region = screen.getByRole("region", { name: "Admin section: Sentinels" });
    expect(within(region).queryByRole("button")).toBeNull();
    expect(within(region).getByText(/has no operation to register, switch off or configure a sentinel/)).toBeInTheDocument();
    expect(within(region).getByText(/Last seen is the time of its last hello or batch of records/)).toBeInTheDocument();
    expect(app.calls.filter((c) => c.method !== "GET")).toHaveLength(0);
  });
});

describe("when there is nothing to show", () => {
  it("says no sentinel is registered and what that means for who made an edit", async () => {
    const world = newT5World();
    world.sentinels = [];
    await openSection(world, "/admin/sentinels");
    expect(await screen.findByText("No sentinels registered")).toBeInTheDocument();
    expect(screen.getByText(/Until a sentinel reports from an NFS VM, edits made on it can't be attributed to a person/)).toBeInTheDocument();
  });

  it("shows a skeleton while loading", async () => {
    const world = newT5World();
    world.hold.add("/api/v1/admin/sentinels");
    await openSection(world, "/admin/sentinels");
    expect(await screen.findByLabelText("Loading sentinels")).toHaveAttribute("aria-busy", "true");
  });

  it("says the list could not be loaded, with the request id", async () => {
    const world = newT5World();
    world.overrides.push({ match: (c) => c.path.startsWith("/api/v1/admin/sentinels"), respond: () => problem(500, "internal"), once: true });
    await openSection(world, "/admin/sentinels");
    expect(await screen.findByText("Couldn't load sentinels")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });

  it("gives a Viewer the permission state and asks the hub for nothing of the Admin area", async () => {
    const app = renderApp("/admin/sentinels", t5Api(newT5World("viewer")));
    expect(await screen.findByRole("heading", { level: 1, name: "Admin needs the Admin role" }, FIRST)).toBeInTheDocument();
    expect(app.calls.some((c) => c.path.startsWith("/api/v1/admin"))).toBe(false);
  });
});

describe("text the hub was given", () => {
  const RLO = "‮";
  const ZWSP = "​";

  it("draws the hidden characters of a host, an id and a version as visible markers", async () => {
    const world = newT5World();
    world.sentinels = [sentinel(`nfs${RLO}-01`, { id: `id${ZWSP}1`, version: `v${ZWSP}0.9` })];
    await openSection(world, "/admin/sentinels");
    await grid();
    await waitFor(() => expect(screen.getAllByRole("img", { name: /U\+(202E|200B)/ }).length).toBeGreaterThanOrEqual(3));
    expect(document.body.textContent).not.toContain(RLO);
    expect(document.body.textContent).not.toContain(ZWSP);
  });
});

describe("accessibility", () => {
  it("has no structural violations", async () => {
    await openSection(newT5World(), "/admin/sentinels");
    await rowIn(await grid(), "nfs-sit-01");
    await expectNoAxeViolations();
  });
});
