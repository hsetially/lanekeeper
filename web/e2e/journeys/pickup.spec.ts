// The pickup chips (T6, slice A) as the live layer keeps them: a `pickup_changed` frame changes the chip of a service on the Services
// tab and the mark of a file in the tree within a second, from what the hub says when the page asks again (the journey changes
// the hub's answer and then writes the frame, as a real hub would), and the same for the head of a file.
//
// Real browser, the real Rust fixture's headers and CSP, and a design scenario: each journey ends in `finish`. Each waits for the
// first read after the stream opens (`settled`) before it changes the hub's answers, so that the frame, and not that read, is what
// it measures.
import { CORE_PATH } from "../../src/mocks/design/file/data.ts";
import { DIST, expect, finish, open, test } from "./support";
import { afterPickup, chipOf, emit, live, serviceRow, settled } from "./t6";

test.use({ dist: DIST });

const BASE = "design-pickup-states";
const SERVICES = "/swimlanes/sitb/services";
const CORE = { namespace: "sitb", name: "tx-infinity-core" };
const picked = (name: string) => (service: Parameters<NonNullable<Parameters<typeof afterPickup>[1]["service"]>>[0]) => (service.service.name === name ? { ...service, pickup: "live" as const, pending_since: null, pending_files: [] } : service);

test.describe("a pickup_changed frame", () => {
  test.use({ scenario: BASE });

  test("changes the chip of the service and the mark of the file within a second, and the rows that did not change keep theirs", async ({ page, harness, csp }) => {
    await open(page, harness.url, SERVICES, "sitb");
    await live(page);
    await settled(harness);
    const core = serviceRow(page, "tx-infinity-core");
    const other = serviceRow(page, "ui-service");
    const mark = page.getByRole("treeitem", { name: /^application\.properties/ });
    await expect(chipOf(core)).toHaveAttribute("data-pickup", "needs_notify_or_restart");
    await expect(chipOf(other)).toHaveAttribute("data-pickup", "needs_notify_or_restart");
    await expect(mark.getByRole("img", { name: "Needs notify or restart" })).toBeVisible();

    // The services picked the change up: the hub's answers say so, and then it tells the page that pickup changed. Nothing the page
    // does by itself in between would read the new answers (the stream is quiet), so a change on screen is the frame's doing.
    harness.setScenario(afterPickup(BASE, { service: picked("tx-infinity-core"), entry: (entry) => (entry.path === "application.properties" ? { ...entry, pickup_state: "live" } : entry) }));
    await page.waitForTimeout(300);
    await expect(chipOf(core), "nothing asks the hub again until a frame names it").toHaveAttribute("data-pickup", "needs_notify_or_restart");
    await expect(mark.getByRole("img", { name: "Needs notify or restart" })).toBeVisible();

    const started = Date.now();
    emit(harness, { type: "pickup_changed", swimlane: "sitb", services: [CORE] });
    await expect(chipOf(core)).toHaveAttribute("data-pickup", "live", { timeout: 1_000 });
    await expect(mark.getByRole("img")).toHaveCount(0, { timeout: 1_000 });
    // P13: an event reaches the screen inside a second. This is the whole walk: the frame, the plan, the reads, the draw.
    expect(Date.now() - started, "from the frame to both changes on screen (ms)").toBeLessThan(1_000);
    // What did not change is as it was.
    await expect(chipOf(other)).toHaveAttribute("data-pickup", "needs_notify_or_restart");
    await expect(chipOf(serviceRow(page, "config-server"))).toHaveAttribute("data-pickup", "needs_config_server_restart");
    await expect(page.getByRole("treeitem", { name: /^channels\.yml/ }).getByRole("img", { name: "Needs config-server restart" })).toBeVisible();
    await finish(page, csp, harness);
  });

  test("pending_restart_changed does the same, and a frame that names no pickup does not read the services again", async ({ page, harness, csp }) => {
    await open(page, harness.url, SERVICES, "sitb");
    await live(page);
    await settled(harness);
    const core = serviceRow(page, "tx-infinity-core");
    await expect(chipOf(core)).toHaveAttribute("data-pickup", "needs_notify_or_restart");
    harness.setScenario(afterPickup(BASE, { service: picked("tx-infinity-core") }));
    harness.clearRequests();
    // The hub's answer for sitb has changed, but a frame about ANOTHER swimlane names nothing of sitb's: its detail is not read, so
    // the chip is still what the page last read. Only a frame that names sitb's pickup makes the page ask.
    emit(harness, { type: "drift_changed", swimlane: "sit", paths: ["a.yml"] });
    await page.waitForTimeout(500);
    expect(
      harness.requests().filter((r) => r.method === "GET" && r.path === "/api/v1/swimlanes/sitb"),
      "reads of sitb's detail after a frame about another swimlane",
    ).toEqual([]);
    await expect(chipOf(core)).toHaveAttribute("data-pickup", "needs_notify_or_restart");
    emit(harness, { type: "pending_restart_changed", swimlane: "sitb", services: [CORE] });
    await expect(chipOf(core)).toHaveAttribute("data-pickup", "live", { timeout: 1_000 });
    await finish(page, csp, harness);
  });
});

test.describe("the head of a file", () => {
  test.use({ scenario: "design-notify-file-saved" });

  test("follows a pickup_changed frame: its chip changes with no reload", async ({ page, harness, csp }) => {
    await open(page, harness.url, `/swimlanes/sitb/files?path=${encodeURIComponent(CORE_PATH)}`, "tx-infinity-core.yml");
    await live(page);
    await settled(harness);
    const chip = page.locator("[data-slot='pickup-chip']").first();
    await expect(chip).toHaveAttribute("data-pickup", "needs_notify_or_restart");
    harness.setScenario(afterPickup("design-notify-file-saved", { entry: (entry) => (entry.path === CORE_PATH ? { ...entry, pickup_state: "live" } : entry) }));
    emit(harness, { type: "pickup_changed", swimlane: "sitb", services: [CORE] });
    await expect(chip).toHaveAttribute("data-pickup", "live", { timeout: 1_000 });
    await finish(page, csp, harness);
  });
});

test.describe("the Swimlanes list", () => {
  test.use({ scenario: BASE });

  test("the Pickup column of a swimlane follows a pickup_changed frame: each state has its own chip and its own count", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/swimlanes", "Swimlanes");
    await live(page);
    await settled(harness);
    const row = page.getByRole("row").filter({ has: page.getByRole("link", { name: "sitb", exact: true }) });
    const cell = row.getByRole("gridcell", { name: /Needs config-server restart/ });
    await expect(cell.getByRole("img", { name: "1 Needs config-server restart" })).toBeVisible();
    await expect(cell.getByRole("img", { name: "2 Needs notify or restart" })).toBeVisible();
    // The hub's list now says one service waits for a notify and none for the config-server.
    harness.setScenario(afterPickup(BASE, { lane: (lane) => (lane.id === "sitb" ? { ...lane, pickup_counts: [["needs_notify_or_restart", 1]] } : lane) }));
    emit(harness, { type: "pickup_changed", swimlane: "sitb", services: [CORE] });
    const after = row.getByRole("gridcell", { name: /Needs notify or restart/ });
    await expect(after.getByRole("img", { name: "1 Needs notify or restart" })).toBeVisible({ timeout: 1_000 });
    await expect(after.getByRole("img", { name: /Needs config-server restart/ })).toHaveCount(0);
    await finish(page, csp, harness);
  });
});
