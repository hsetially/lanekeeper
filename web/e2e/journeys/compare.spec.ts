// Compare (S4) as a person walks it: one file across swimlanes (choose the file, pick two cells in a row, open the diff and leave it),
// a whole swimlane against another swimlane, a branch, a tag (and a binary file in it), and a live event on a grid that is loaded
// page after page, which must cost one request and not one for every page.
import type { Page } from "@playwright/test";
import { DIST, callsTo, expect, finish, h1, open, test } from "./support";

test.use({ dist: DIST });

const FILE = "services/payments/application.yml";
const grid = (page: Page) => page.getByRole("grid", { name: "application.yml across swimlanes" });

test.describe("one file across swimlanes", () => {
  test.use({ scenario: "design-compare-default" });

  test("choose a file, click two cells of a row, open the diff of that setting between those two swimlanes, and leave it with Escape", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/compare?lanes=sit,sitb,sitc", "Compare");
    await expect(page.getByText("Choose a file to compare")).toBeVisible();

    // Choose a file from the ones that differ between the first two swimlanes.
    await page.getByRole("button", { name: "Logical file: none chosen" }).first().click();
    const chooser = page.getByRole("dialog", { name: "Choose a logical file" });
    await expect(chooser).toBeVisible();
    await expect(chooser.getByRole("list", { name: "Files that differ" })).toBeVisible();
    // A path that is not below the config root is refused where it is typed, before anything is sent.
    await chooser.getByLabel("Logical file").fill("../etc/passwd");
    await page.keyboard.press("Enter");
    await expect(chooser.getByRole("alert")).toContainText("not a path below the config root");
    await chooser.getByLabel("Logical file").fill(FILE);
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(new RegExp(`file=${encodeURIComponent(FILE).replaceAll(".", "\\.")}`));
    await expect(grid(page)).toBeVisible();

    // A row where sitb differs: click its cell in sit, then its cell in sitb. The first click starts the pick, the second completes it
    // and opens the diff of the two.
    const row = grid(page).getByRole("row", { name: /core\.pool\.host-conn\. max-size/ });
    await expect(row).toBeVisible();
    await row.getByRole("gridcell").nth(1).getByRole("button").click();
    const bar = page.getByRole("status").filter({ hasText: "Comparing at core.pool.host-conn.max-size" });
    await expect(bar).toBeVisible();
    await expect(bar).toContainText("Click another cell in the same row");
    await expect(bar.getByRole("button", { name: "Open diff" })).toHaveCount(0);
    const before = callsTo(harness, "GET", /^\/api\/v1\/compare\?/).length;
    await row.getByRole("gridcell").nth(2).getByRole("button").click();
    await expect(bar.getByLabel("Swimlane A")).toHaveValue("0");
    await expect(bar.getByLabel("Swimlane B")).toHaveValue("1");
    await expect(bar).not.toContainText("Click another cell");

    // The diff: asked for once, between the effective files of those two, for the file, and shown in a drawer that has the keyboard.
    const drawer = page.getByRole("dialog", { name: "Diff at setting" });
    await expect(drawer).toBeVisible();
    await expect(drawer.getByRole("button", { name: "Close diff" })).toBeFocused();
    await expect(drawer).toContainText("core.pool.host-conn.max-size");
    await expect(drawer.getByText("Read only. Edit from a file view.")).toBeVisible();
    const asked = callsTo(harness, "GET", /^\/api\/v1\/compare\?/).slice(before);
    expect(asked).toHaveLength(1);
    const query = new URL(asked[0]?.path ?? "", harness.url).searchParams;
    expect(query.get("left")).toBe("effective:sit");
    expect(query.get("right")).toBe("effective:sitb");
    expect(query.get("file")).toBe(FILE);

    // Swap, then Escape: the drawer goes, the picks stay, and the keyboard is back on the cell that was clicked last.
    await drawer.getByRole("button", { name: "Swap A and B" }).click();
    await expect(bar.getByLabel("Swimlane A")).toHaveValue("1");
    await expect(bar.getByLabel("Swimlane B")).toHaveValue("0");
    await page.keyboard.press("Escape");
    await expect(drawer).toHaveCount(0);
    await expect(bar).toBeVisible();
    await expect(bar.getByRole("button", { name: "Open diff" })).toBeVisible();
    // It opens again from the bar. The drawer starts under the bar, so the bar's buttons are still where the pointer can reach them:
    // Hide diff closes the drawer it opened, and Clear takes the picks and the drawer away together (the drawer open).
    await bar.getByRole("button", { name: "Open diff" }).click();
    await expect(drawer).toBeVisible();
    await expect(bar.getByRole("button", { name: "Hide diff" })).toHaveAttribute("aria-expanded", "true");
    const drawerBox = await drawer.boundingBox();
    const barBox = await bar.boundingBox();
    expect(drawerBox, "the drawer is on screen").not.toBeNull();
    expect(barBox, "the bar is on screen").not.toBeNull();
    expect((drawerBox?.y ?? 0) + 0.5, "the drawer starts below the bar").toBeGreaterThanOrEqual((barBox?.y ?? 0) + (barBox?.height ?? 0));
    await bar.getByRole("button", { name: "Hide diff" }).click();
    await expect(drawer).toHaveCount(0);
    await bar.getByRole("button", { name: "Open diff" }).click();
    await expect(drawer).toBeVisible();
    await bar.getByRole("button", { name: "Clear" }).click();
    await expect(drawer).toHaveCount(0);
    await expect(bar).toHaveCount(0);
    await finish(page, csp, harness);
  });
});

test.describe("a whole swimlane", () => {
  test.use({ scenario: "design-compare-default" });
  const left = (page: Page, name: string) => page.getByRole("button", { name: `Left side: ${name}` });
  const right = (page: Page, name: string) => page.getByRole("button", { name: `Right side: ${name}` });
  const tree = (page: Page) => page.getByRole("tree", { name: "Differing files" });

  test("sitb against sitc, then against a branch, then against a tag: each lists the files that differ, and a binary file is compared by hash", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/compare?lanes=sitb,sitc", "Compare");
    await page.getByRole("button", { name: "Whole swimlane" }).click();
    await expect(page).toHaveURL(/left=effective%3Asitb&right=effective%3Asitc/);
    await expect(left(page, "sitb")).toBeVisible();
    await expect(right(page, "sitc")).toBeVisible();
    await expect(page.getByRole("group", { name: "Summary" })).toContainText("38");
    await expect(page.getByRole("button", { name: "8 Differing logical files" })).toBeVisible();
    await expect(page.getByRole("button", { name: "3 Only in sitb" })).toBeVisible();
    await expect(page.getByRole("button", { name: "2 Only in sitc" })).toBeVisible();
    await expect(page.getByText(/Compared at \d\d:\d\d UTC · 13 files differ/)).toBeVisible();
    await expect(tree(page).getByRole("treeitem", { name: /queue-routes\.yml changed/ })).toBeVisible();

    // A text file: the server's hunks, with the sides named.
    await tree(page).getByRole("treeitem", { name: /queue-routes\.yml/ }).click();
    await expect(page.getByRole("heading", { level: 2, name: "messaging/queues/queue-routes.yml" })).toBeVisible();
    await expect(page.getByRole("grid", { name: "Differences in the file" })).toBeVisible();
    const sent = callsTo(harness, "GET", /^\/api\/v1\/compare\?/).map((r) => new URL(r.path, harness.url).searchParams);
    expect(sent.some((q) => q.get("left") === "effective:sitb" && q.get("right") === "effective:sitc" && q.get("file") === "messaging/queues/queue-routes.yml")).toBe(true);

    // A binary file: compared by hash, with no lines to show.
    await tree(page).getByRole("treeitem", { name: /receipt-logo\.jpeg/ }).click();
    await expect(page.getByRole("heading", { level: 2, name: "resources/receipt-logo.jpeg" })).toBeVisible();
    await expect(page.getByText("Binary", { exact: true }).first()).toBeVisible();
    await expect(page.getByRole("grid", { name: "Differences in the file" })).toHaveCount(0);

    // Against a Git branch of the tenant repository: the right side's list is the repository's branches.
    await page.getByRole("radio", { name: "Git branch" }).click();
    await right(page, "").or(page.getByRole("button", { name: /^Right side: choose a Git branch/ })).click();
    const branches = page.getByRole("dialog", { name: "Right side: choose a Git branch" });
    await expect(branches.getByRole("list", { name: "Branches of the tenant repository" })).toBeVisible();
    await branches.getByRole("button", { name: /^sit7/ }).click();
    await expect(page).toHaveURL(/right=git%3Atenant%40sit7/);
    await expect(right(page, "sit7")).toBeVisible();
    await expect(tree(page).getByRole("treeitem").first()).toBeVisible();
    await expect(page.getByText(/Compared at \d\d:\d\d UTC/)).toBeVisible();

    // Against a Git tag of the base repository.
    await page.getByRole("radio", { name: "Git tag" }).click();
    await page.getByRole("button", { name: /^Right side: choose a Git tag/ }).click();
    const tags = page.getByRole("dialog", { name: "Right side: choose a Git tag" });
    await expect(tags.getByRole("list", { name: "Tags of the base repository" })).toBeVisible();
    await tags.getByRole("button", { name: /^v2\.3\.1/ }).click();
    await expect(page).toHaveURL(/right=git%3Abase%40v2\.3\.1/);
    await expect(right(page, "v2.3.1")).toBeVisible();
    await expect(tree(page).getByRole("treeitem").first()).toBeVisible();
    await finish(page, csp, harness);
  });
});

test.describe("a live event on a grid of many pages", () => {
  test.use({ scenario: "design-compare-large" });

  test("costs one request for the first page and none for the pages loaded after it", async ({ page, harness, csp }) => {
    test.setTimeout(120_000);
    const lanes = Array.from({ length: 40 }, (_, i) => `swimlane-${String(i + 1).padStart(2, "0")}`).join(",");
    await page.goto(`${harness.url}/compare?file=services%2Fpayments%2Fapplication.yml&lanes=${lanes}&only=all`);
    await expect(h1(page)).toHaveText("Compare");
    await expect(grid(page)).toBeVisible();
    // The page asks for its first page, and once more when the stream first connects (nothing to resume from): let that settle.
    await expect(page.locator("[data-slot='live-indicator']")).toHaveAttribute("data-status", "live");
    await page.waitForTimeout(1500);
    const pages = (): string[] => callsTo(harness, "GET", /^\/api\/v1\/grid\?/).map((r) => new URL(r.path, harness.url).searchParams.get("cursor") ?? "first");
    harness.clearRequests();

    // Page after page: End on the grid takes the cursor to the last row loaded, which asks for the next page of 500.
    await grid(page).focus();
    for (const cursor of ["o50", "o550", "o1050"]) {
      await page.keyboard.press("End");
      await expect.poll(pages, { timeout: 15_000 }).toContain(cursor);
      await page.waitForTimeout(400);
    }
    expect(pages(), "each page once").toEqual(["o50", "o550", "o1050"]);

    // Back at the top, where the first page is what is in view, a live event for a swimlane of the grid: one request, for the
    // first page, and the pages below it are dropped and not asked for again.
    await page.keyboard.press("Home");
    await page.waitForTimeout(600);
    harness.clearRequests();
    harness.emit(`id: 500\nevent: file_observed\ndata: ${JSON.stringify({ type: "file_observed", swimlane: "swimlane-01", path: FILE, hash: "a".repeat(64), source: "scan" })}\n\n`);
    await expect.poll(() => pages().length, { timeout: 10_000 }).toBeGreaterThanOrEqual(1);
    await page.waitForTimeout(1500);
    expect(pages(), "one request: the first page").toEqual(["first"]);
    const first = callsTo(harness, "GET", /^\/api\/v1\/grid\?/)[0];
    expect(new URL(first?.path ?? "", harness.url).searchParams.get("limit")).toBe("50");
    // Nothing else was asked for either, but the swimlane list (the sidebar and the switcher show its scan times and counts), once.
    expect(harness.requests().filter((r) => r.kind !== "stream" && !r.path.startsWith("/api/v1/grid")).map((r) => `${r.method} ${r.path}`)).toEqual(["GET /api/v1/swimlanes"]);
    await finish(page, csp, harness);
  });
});
