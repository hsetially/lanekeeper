// Findings (S6a) as a person walks them: narrow the list to one check, read the row, and open the file at the line the check points to.
import { DIST, callsTo, derive, expect, finish, h1, open, responsesOf, test } from "./support";

test.use({ dist: DIST });

// The findings scenario has no file view behind its links; the file view's answers (same Admin, same swimlanes) are added to it.
const FILE_VIEW = responsesOf("design-file-core").filter((r) => !["/api/v1/me", "/api/v1/swimlanes", "/api/v1/proposals", "/api/v1/admin/access-requests", "/auth/logout"].includes(r.path));
const FINDINGS_AND_FILES = derive("design-findings-open", FILE_VIEW, "journey-findings-files");

test.describe("the findings", () => {
  test.use({ scenario: FINDINGS_AND_FILES });

  test("from the sidebar, narrowed to one check, a row opens the file at the line the check points to", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/swimlanes", "Swimlanes");
    // The sidebar's count and the page agree.
    const entry = page.getByRole("complementary", { name: "Primary" }).getByRole("link", { name: /^Findings 11 open/ });
    await expect(entry).toBeVisible();
    await entry.click();
    await expect(page).toHaveURL(/\/findings$/);
    await expect(h1(page)).toBeFocused();
    const table = page.getByRole("grid", { name: "Findings" });
    await expect(table.getByRole("row")).toHaveCount(12);
    await expect(page.getByText("Showing 11 findings")).toBeVisible();

    // One check: C7, a sync that overwrote an NFS change. The server filters; the address keeps the choice.
    const filters = page.getByRole("group", { name: "Filters" });
    await filters.getByRole("button", { name: "Check" }).click();
    const menu = page.getByRole("menu");
    await menu.getByRole("menuitemcheckbox", { name: /^C7/ }).click();
    await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
    await expect(filters.getByRole("button", { name: "Check" })).toBeFocused();
    await expect(page).toHaveURL(/code=C7/);
    const asked = callsTo(harness, "GET", /^\/api\/v1\/findings\?/).map((r) => new URL(r.path, harness.url).searchParams);
    expect(asked.at(-1)?.get("kinds")).toBe("overwritten_nfs_change");
    await expect(table.getByRole("row")).toHaveCount(2);
    const row = table.getByRole("row", { name: /C7 Sync overwrote an NFS change/ });
    await expect(row).toContainText("sitb");
    await expect(row).toContainText("tx-infinity-gateway-sit7.yml");

    // The row opens the drawer with the evidence; the link in it, and the one in the row, open the file at the line.
    const link = row.getByRole("link", { name: "Open file at L22" });
    await expect(link).toHaveAttribute("href", "/swimlanes/sitb/files?line=22&path=tx-infinity-api%2Ftx-infinity-gateway-sit7.yml");
    await link.click();
    await expect(page).toHaveURL(/\/swimlanes\/sitb\/files\?line=22&path=tx-infinity-api%2Ftx-infinity-gateway-sit7\.yml$/);
    await expect(h1(page)).toBeVisible();
    const marked = page.locator("[data-highlight='true']");
    await expect(marked).toHaveCount(1);
    await expect(marked).toContainText("22");
    // Back is the list with the same check chosen.
    await page.goBack();
    await expect(filters.getByRole("button", { name: "Check" })).toContainText("C7");
    await finish(page, csp, harness);
  });
});
