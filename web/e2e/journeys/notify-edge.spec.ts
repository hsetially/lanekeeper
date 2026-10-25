// The edges of Notify services (T6, slice B): text a hostile file share can put in the names of the files a service waits on, and the
// swimlane where nothing waits at all. Real browser, the Rust fixture's headers and CSP, a mock scenario of the hub; each journey ends
// in `finish`.
import type { Page } from "@playwright/test";
import { callsTo, DIST, expect, finish, open, test, writesOf } from "./support";
import { derive, responseOf } from "./scenarios";
import { NOTIFY_PATH, SERVICES_TAB } from "./t6";

test.use({ dist: DIST });

const notifyButton = (page: Page) => page.getByRole("button", { name: "Notify services" });
const dialog = (page: Page) => page.getByRole("alertdialog", { name: "Notify services in sitb?" });

test.describe("hostile text in the files a service waits on", () => {
  test.use({ scenario: "design-pickup-hostile" });

  test("the Notify dialog draws names with a right-to-left override, markup and a zero-width space as visible text, builds no element from them, runs nothing, and sends the paths exactly as the hub gave them", async ({ page, harness, csp }) => {
    const notify = responseOf("design-notify-services-tab", "POST", "/api/v1/swimlanes/{id}/notify");
    harness.setScenario(derive("design-pickup-hostile", [notify], "journey-hostile-notify"));
    const dialogs: string[] = [];
    page.on("dialog", (d) => {
      dialogs.push(d.message());
      void d.dismiss();
    });
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await notifyButton(page).click();
    const ask = dialog(page);
    await expect(ask).toBeVisible();
    const list = ask.getByRole("region", { name: "3 files to announce" });
    await expect(list).toBeVisible();
    // The markup is text; the override and the zero-width space are visible markers, not characters that move the text around.
    await expect(list).toContainText("<img src=x onerror=alert(1)>.yml");
    expect(await ask.locator("img, script, iframe").count()).toBe(0);
    expect(await list.evaluate((el) => /[\u202e\u200b]/.test(el.textContent ?? ""))).toBe(false);
    await ask.getByRole("button", { name: "Notify services" }).click();
    // The scenario's answer is a count of 2 for the 3 paths that were sent: partly accepted, and the page says no more than that.
    const done = page.getByRole("dialog", { name: "Notification partly accepted" });
    await expect(done).toBeVisible();
    await expect(done).toContainText("The hub accepted 2 of 3 paths for sitb.");
    const sent = JSON.parse(callsTo(harness, "POST", NOTIFY_PATH)[0]?.body ?? "{}") as { paths: string[] };
    expect([...sent.paths].sort()).toEqual(["tx-infinity-api/inv\u202eoice.yml", "tx-infinity-api/ze\u200bro.yml", "tx-infinity-api/<img src=x onerror=alert(1)>.yml"].sort());
    expect(dialogs).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("nothing waits for a notification", () => {
  test.use({ scenario: "design-pickup-quiet" });

  test("Notify services is off with the reason, and a press opens nothing", async ({ page, harness, csp }) => {
    await open(page, harness.url, SERVICES_TAB, "sitb");
    await expect(notifyButton(page)).toHaveAttribute("aria-disabled", "true");
    await expect(notifyButton(page)).toHaveAccessibleDescription("No service is waiting on a file.");
    await notifyButton(page).click({ force: true });
    await page.waitForTimeout(250);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(writesOf(harness)).toEqual([]);
    await finish(page, csp, harness);
  });
});
