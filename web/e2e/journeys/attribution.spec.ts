// Who changed a file, how sure the hub is, and how serious the change is (T5, slice A), as a person meets it: names for an Admin and
// short ids for everyone else (with no request for the Admin-only list), an "Unknown author" that becomes a login as soon as the hub
// says so, and one severity badge, one order and one filter wherever a severity is drawn.
import type { Locator, Page } from "@playwright/test";
import { CORE_PATH } from "../../src/mocks/design/file/data.ts";
import { DIST, callsTo, expect, finish, open, test } from "./support";
import { PRIYA, SEVERITY_FINDINGS, asRole, emit, live, severityHub, upgradeHub } from "./t5";

test.use({ dist: DIST });

const FILE = `/swimlanes/sitb/files?path=${encodeURIComponent(CORE_PATH)}&panel=history`;
const history = (page: Page) => page.getByRole("complementary", { name: "History" });
const chips = (scope: Locator): Locator => scope.locator("[data-slot='attribution'] button");
const evidence = (page: Page) => page.getByRole("dialog", { name: "Attribution evidence" });
const users = (harness: Parameters<typeof callsTo>[0]) => callsTo(harness, "GET", /^\/api\/v1\/admin\/users(\?|$)/);

async function openHistory(page: Page, origin: string, versions = 9): Promise<void> {
  await open(page, origin, FILE, "tx-infinity-core.yml");
  await expect(chips(history(page))).toHaveCount(versions);
}

test.describe("names", () => {
  test("an Admin sees the people's names and 'You'; a user the list does not know is a short id, never a made-up name; one list serves every chip", async ({ page, harness, csp }) => {
    harness.setScenario("design-attribution-history");
    await openHistory(page, harness.url);
    const rows = chips(history(page));
    await expect(rows.nth(1)).toContainText("You");
    await expect(rows.nth(2)).toContainText("Marco Diaz");
    await expect(rows.nth(8)).toContainText("Lanekeeper user cccccccc");
    // The head of the file says the same of its last change.
    await expect(page.locator("[data-slot='file-changed-by']")).toContainText("dana.okoye");
    // A name is the person's own text: it is in the evidence too.
    await rows.nth(2).click();
    await expect(evidence(page)).toContainText("Marco Diaz");
    await page.keyboard.press("Escape");
    // One read of the list for all the chips (the same address each time), however many there are: once, or twice when the stream's first
    // connection reads again what is on screen.
    await live(page);
    await page.waitForTimeout(700);
    expect(users(harness).length).toBeGreaterThanOrEqual(1);
    expect(users(harness).length).toBeLessThanOrEqual(2);
    expect(new Set(users(harness).map((r) => r.path)).size).toBe(1);
    await finish(page, csp, harness);
  });

  test("an Editor sees the same people as short ids, and the page never asks for the Admin-only list of users", async ({ page, harness, csp }) => {
    harness.setScenario(asRole("design-attribution-history", "editor", { id: PRIYA, name: "Priya Shah", email: "priya.shah@example.test" }));
    await openHistory(page, harness.url);
    const rows = chips(history(page));
    // Their own changes are still "You": the session knows its own id.
    await expect(rows.nth(1)).toContainText("You");
    await expect(rows.nth(2)).toContainText("Lanekeeper user bbbbbbbb");
    await expect(rows.nth(8)).toContainText("Lanekeeper user cccccccc");
    await expect(page.locator("body")).not.toContainText("Marco Diaz");
    await rows.nth(2).click();
    await expect(evidence(page)).toContainText("Lanekeeper user bbbbbbbb");
    await expect(evidence(page)).not.toContainText("Marco Diaz");
    await page.keyboard.press("Escape");
    // The stream's first connection reads again what is on screen; a moment for that and for anything late, then: no request for the
    // user list at all (the scenario answers it, so this is not a gap in the mock).
    await live(page);
    await page.waitForTimeout(500);
    expect(users(harness)).toEqual([]);
    await finish(page, csp, harness);
  });

  test("the Audit log names the same people for an Admin: the actor, and the person an event is attributed to", async ({ page, harness, csp }) => {
    harness.setScenario("design-attribution-audit");
    await open(page, harness.url, "/audit", "Audit");
    const table = page.getByRole("grid", { name: /Audit/ });
    await expect(table.getByRole("row").nth(1)).toBeVisible();
    await expect(table).toContainText("Marco Diaz");
    await expect(table).toContainText("Lanekeeper user cccccccc");
    expect(users(harness).length).toBeGreaterThan(0);
    await finish(page, csp, harness);
  });
});

test.describe("an author that becomes known", () => {
  test("'Unknown author' turns into the login when the hub says attribution was updated, on the head of the file and in the history, without a reload", async ({ page, harness, csp }) => {
    const hub = upgradeHub();
    harness.setScenario(hub.before);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await live(page);
    const head = page.locator("[data-slot='file-changed-by'] [data-slot='attribution']");
    const newest = chips(history(page)).first();
    await expect(head).toContainText("Unknown author");
    await expect(newest).toContainText("Unknown author");
    await expect(head).toHaveAttribute("data-known", "false");
    // A page that is not reloaded keeps what it set on itself.
    await page.evaluate(() => {
      (window as unknown as { __kept?: string }).__kept = "same page";
    });
    // The sentinel's evidence arrives: the hub answers differently from now on and says so.
    harness.setScenario(hub.after);
    emit(harness, { type: "attribution_updated", swimlane: "sitb", path: CORE_PATH, hash: "a".repeat(64) });
    await expect(head).toContainText("dana.okoye");
    await expect(head).toContainText("edited on the NFS VM");
    await expect(head).toHaveAttribute("data-known", "true");
    await expect(newest).toContainText("dana.okoye");
    await expect(newest).not.toContainText("Unknown author");
    // Nothing else changed: the other versions are as they were, and the evidence behind the new name is the sentinel's.
    await expect(chips(history(page)).nth(1)).toContainText("You");
    await head.getByRole("button").click();
    await expect(evidence(page)).toContainText("Edited on the NFS VM");
    await expect(evidence(page)).toContainText("Executable");
    await page.keyboard.press("Escape");
    expect(await page.evaluate(() => (window as unknown as { __kept?: string }).__kept)).toBe("same page");
    await finish(page, csp, harness);
  });

  test("an update for another file changes nothing on this page and asks the hub for nothing", async ({ page, harness, csp }) => {
    const hub = upgradeHub();
    harness.setScenario(hub.before);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await live(page);
    await expect(chips(history(page)).first()).toContainText("Unknown author");
    const reads = () => callsTo(harness, "GET", /\/files\/history/).length;
    await page.waitForTimeout(400);
    const before = reads();
    harness.setScenario(hub.after);
    emit(harness, { type: "attribution_updated", swimlane: "sitb", path: "security/security-roles.yml", hash: "b".repeat(64) });
    await page.waitForTimeout(700);
    expect(reads()).toBe(before);
    await expect(chips(history(page)).first()).toContainText("Unknown author");
    await finish(page, csp, harness);
  });
});

// -- One severity ------------------------------------------------------------------------------------------------------------

/** What a severity badge looks like, per level: its word, its colours and its icon. Two screens that agree draw the same. */
async function look(scope: Locator): Promise<Record<string, string>> {
  return scope.locator("[data-slot='severity-badge']").evaluateAll((els) => {
    const found: Record<string, string> = {};
    for (const el of els) {
      const level = el.getAttribute("data-severity") ?? "?";
      const style = getComputedStyle(el);
      const icon = el.querySelector("svg");
      const iconStyle = icon === null ? null : getComputedStyle(icon);
      found[level] = [el.textContent?.trim(), style.color, style.backgroundColor, style.borderTopColor, style.borderTopStyle, style.fontWeight, style.height, icon?.innerHTML, iconStyle?.width].join("|");
    }
    return found;
  });
}

test.describe("severity", () => {
  test("is one badge: the same word, colours and icon for each level in the History panel, the Drift tab and the Findings screen", async ({ page, harness, csp }) => {
    harness.setScenario(severityHub());
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await expect(chips(history(page))).toHaveCount(9);
    const inHistory = await look(history(page));
    expect(Object.keys(inHistory).sort()).toEqual(["critical", "high", "low", "medium"]);

    await open(page, harness.url, "/swimlanes/sitb/drift", "sitb");
    await expect(page.getByRole("list", { name: "Files not in sync" })).toBeVisible();
    const inDrift = await look(page.getByRole("list", { name: "Files not in sync" }));
    expect(Object.keys(inDrift).sort()).toEqual(["critical", "high", "low", "medium"]);

    await open(page, harness.url, "/findings", "Findings");
    const grid = page.getByRole("grid", { name: /Findings/ });
    await expect(grid.getByRole("row").nth(1)).toBeVisible();
    const inFindings = await look(grid);
    expect(Object.keys(inFindings).sort()).toEqual(["critical", "high", "low", "medium"]);

    await open(page, harness.url, "/swimlanes/sitb/findings", "sitb");
    await expect(page.getByText("a prod host in a SIT file").first()).toBeVisible();
    const inTab = await look(page.getByRole("main"));
    expect(Object.keys(inTab).sort()).toEqual(["critical", "high", "low", "medium"]);

    expect(inDrift, "Drift against History").toEqual(inHistory);
    expect(inFindings, "Findings against History").toEqual(inHistory);
    expect(inTab, "the swimlane's Findings tab against History").toEqual(inHistory);
    // Each is an icon, a word and a colour: the word is on the badge, and no two levels share their look.
    expect(new Set(Object.values(inHistory)).size).toBe(4);
    expect(inHistory["critical"]).toContain("Critical|");
    expect(inHistory["low"]).toContain("Low|");
    await finish(page, csp, harness);
  });

  test("is sorted the same way wherever a list has one: most severe first, a file nobody rated last, the files of a group by path after that", async ({ page, harness, csp }) => {
    harness.setScenario(severityHub());
    await open(page, harness.url, "/swimlanes/sitb/drift", "sitb");
    const list = page.getByRole("list", { name: "Files not in sync" });
    await expect(list).toBeVisible();
    const pathsOf = (scope: Locator): Promise<string[]> => scope.locator("bdi[data-slot='safe-text'].font-mono").allTextContents();
    // The conflict group comes first (the most urgent state), then NFS ahead with its files by severity.
    expect(await pathsOf(list)).toEqual(["limits/limits.yml", "services/critical.yml", "services/high.yml", "services/medium.yml", "services/low.yml", "services/unrated.yml"]);

    await open(page, harness.url, "/findings", "Findings");
    const grid = page.getByRole("grid", { name: /Findings/ });
    await expect(grid.getByRole("row").nth(1)).toBeVisible();
    // The hub lists them newest first (low, critical, medium, high); the screen puts them by severity.
    const levels = async (): Promise<string[]> => grid.locator("[data-slot='severity-badge']").evaluateAll((els) => els.map((el) => el.getAttribute("data-severity") ?? "?"));
    expect(await levels()).toEqual(["critical", "high", "medium", "low"]);

    await open(page, harness.url, "/swimlanes/sitb/findings", "sitb");
    await expect(page.getByText("a prod host in a SIT file").first()).toBeVisible();
    expect(await page.getByRole("main").locator("[data-slot='severity-badge']").evaluateAll((els) => els.map((el) => el.getAttribute("data-severity") ?? "?"))).toEqual(["critical", "high", "medium", "low"]);
    await finish(page, csp, harness);
  });

  test("is filtered by the same four choices: 'that level and above', by the server on the Findings screen and on the loaded rows (and saying so) on the Drift tab", async ({ page, harness, csp }) => {
    harness.setScenario(severityHub());
    await open(page, harness.url, "/swimlanes/sitb/drift", "sitb");
    const list = page.getByRole("list", { name: "Files not in sync" });
    await expect(list).toBeVisible();
    const severity = page.getByLabel("Severity", { exact: true });
    const choices = await severity.locator("option").allTextContents();
    expect(choices).toEqual(["Any severity", "Medium and above", "High and above", "Critical only"]);
    await severity.selectOption({ label: "High and above" });
    // The rows at that level and above stay; nobody-rated rows go (a row with no severity is at no level).
    await expect(list.locator("[data-slot='severity-badge']")).toHaveCount(3);
    await expect(list).not.toContainText("services/medium.yml");
    await expect(list).not.toContainText("services/unrated.yml");
    await severity.selectOption({ label: "Critical only" });
    await expect(list.locator("[data-slot='severity-badge']")).toHaveCount(1);
    await expect(list).toContainText("services/critical.yml");
    await severity.selectOption({ label: "Any severity" });
    await expect(list).toContainText("services/unrated.yml");

    // The Findings screen offers the same words and asks the server (`min_severity`), so its answer is whole.
    await open(page, harness.url, "/findings", "Findings");
    const grid = page.getByRole("grid", { name: /Findings/ });
    await expect(grid.getByRole("row")).toHaveCount(5);
    await page.getByRole("group", { name: "Filters" }).getByRole("button", { name: /Severity/ }).click();
    const radios = page.getByRole("menuitemradio");
    expect(await radios.allTextContents()).toEqual(choices);
    await radios.filter({ hasText: "High and above" }).click();
    await expect(grid.getByRole("row")).toHaveCount(3);
    await expect(grid).not.toContainText("a is set twice");
    const asked = callsTo(harness, "GET", /^\/api\/v1\/findings\?/).map((r) => new URL(r.path, harness.url).searchParams.get("min_severity"));
    expect(asked).toContain("high");
    expect(SEVERITY_FINDINGS.filter((f) => f.severity === "critical" || f.severity === "high")).toHaveLength(2);
    await finish(page, csp, harness);
  });

  test("the Drift tab says when its filter reaches only the rows that are loaded", async ({ page, harness, csp }) => {
    // The second page of the list never arrives: the tab has files and the hub has more.
    await page.route("**/api/v1/swimlanes/sitb/drift**", async (route) => {
      if (new URL(route.request().url()).searchParams.get("cursor") !== null) return;
      const response = await route.fetch();
      const body = (await response.json()) as { items: unknown[]; next: string | null };
      await route.fulfill({ response, json: { ...body, next: "after-more" } });
    });
    harness.setScenario(severityHub());
    await open(page, harness.url, "/swimlanes/sitb/drift", "sitb");
    const list = page.getByRole("list", { name: "Files not in sync" });
    await expect(list).toBeVisible();
    await expect(page.getByText(/Applies to the/)).toHaveCount(0);
    await page.getByLabel("Severity", { exact: true }).selectOption({ label: "High and above" });
    await expect(page.getByText("Applies to the 6 files loaded so far. More are not loaded.")).toBeVisible();
    await finish(page, csp, harness);
  });
});
