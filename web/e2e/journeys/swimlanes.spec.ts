// The Swimlanes screens (S2) as a person walks them: restarting a service (applied, sent for approval, refused because the swimlane
// is locked), and picking swimlanes in the list to compare them.
import { DIST, callsTo, expect, finish, h1, open, test, writesOf } from "./support";

test.use({ dist: DIST });

const SERVICES = "/swimlanes/sitb/services";
const RESTART = "/api/v1/swimlanes/sitb/restart";

test.describe("restarting a service", () => {
  test.use({ scenario: "design-swimlanes-six" });

  test("is applied: the dialog says what will happen, asks once, closes on the hub's answer, and the page says it was accepted (and no more)", async ({ page, harness, csp }) => {
    await open(page, harness.url, SERVICES, "sitb");
    await page.getByRole("button", { name: "Restart tx-infinity-core" }).click();
    const dialog = page.getByRole("alertdialog", { name: "Restart tx-infinity-core in sitb?" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("replaces its pods");
    await expect(dialog).toContainText("tx-infinity-core-sit7.yml and 1 more file");
    await expect(dialog).toContainText("Other people testing in sitb will be interrupted.");
    // Nothing is sent until the person confirms.
    expect(writesOf(harness)).toEqual([]);
    await dialog.getByRole("button", { name: "Restart tx-infinity-core" }).click();
    await expect(dialog).toHaveCount(0);
    const calls = callsTo(harness, "POST", RESTART);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ csrf: "ok", status: 200 });
    expect(calls[0]?.idempotencyKey).toBeTruthy();
    expect(calls[0]?.body).toContain("tx-infinity-core");
    // The page says the hub accepted it, and does not say the service is back: the contract has no restart progress.
    await expect(page.getByText("The hub accepted it.")).toBeVisible();
    await expect(page.getByText(/is back|restarted successfully|ready again/i)).toHaveCount(0);
    // The row's button works again, and focus is back on it.
    await expect(page.getByRole("button", { name: "Restart tx-infinity-core" })).toBeFocused();
    await finish(page, csp, harness);
  });

  // `restartService` answers 200 with a WriteResult, and a restart that needs an Admin is the `proposal_created` member of it (plan
  // W33); the contract has no 202 for this operation, and a mock that answered one would not be the hub's.
  test("is sent for approval (200, proposal_created): nothing restarts, the dialog says the request's number, and an Admin can open it", async ({ page, harness, csp }) => {
    harness.setScenario("design-swimlanes-restart-approval");
    await open(page, harness.url, SERVICES, "sitb");
    await page.getByRole("button", { name: "Restart ui-service" }).click();
    await page.getByRole("alertdialog", { name: "Restart ui-service in sitb?" }).getByRole("button", { name: "Restart ui-service" }).click();
    const sent = page.getByRole("dialog", { name: "Restart sent for approval" });
    await expect(sent).toBeVisible();
    await expect(sent).toContainText("Nothing restarts until an Admin approves request AP-2042.");
    await expect(sent.getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/2042");
    const calls = callsTo(harness, "POST", RESTART);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ status: 200 });
    await sent.getByRole("button", { name: "Close" }).click();
    await expect(sent).toHaveCount(0);
    await expect(page.getByText("Restart of ui-service sent for approval")).toBeVisible();
    await expect(page.getByText("The hub accepted it.")).toHaveCount(0);
    await finish(page, csp, harness);
  });

  test("is refused while the swimlane is locked (423): the dialog stays, names the cause and the request ID, and the person can leave it", async ({ page, harness, csp }) => {
    harness.setScenario("design-swimlanes-restart-locked");
    await open(page, harness.url, SERVICES, "sitb");
    await page.getByRole("button", { name: "Restart document-service" }).click();
    const dialog = page.getByRole("alertdialog", { name: "Restart document-service in sitb?" });
    await dialog.getByRole("button", { name: "Restart document-service" }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("alert")).toBeVisible();
    await expect(dialog).toContainText("req-mock-0001");
    await expect(page.getByText("The hub accepted it.")).toHaveCount(0);
    expect(callsTo(harness, "POST", RESTART)[0]).toMatchObject({ status: 423 });
    // The dialog still has its buttons, and Cancel leaves it with nothing changed.
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Restart document-service" })).toBeFocused();
    expect(callsTo(harness, "POST", RESTART)).toHaveLength(1);
    await finish(page, csp, harness, { expected: [423] });
  });
});

test.describe("comparing swimlanes from the list", () => {
  test.use({ scenario: "design-swimlanes-six" });

  test("two selected swimlanes are compared, in the order of the list, and the comparison opens with them in its columns", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/swimlanes", "Swimlanes");
    const compare = page.getByRole("button", { name: "Compare selected" });
    await expect(compare).toBeDisabled();
    await expect(page.getByText("Select two or more swimlanes to compare.")).toBeVisible();
    await page.getByRole("checkbox", { name: "Select sitb" }).check();
    await expect(page.getByText("1 selected")).toBeVisible();
    await expect(compare).toBeDisabled();
    await page.getByRole("checkbox", { name: "Select presita" }).check();
    await expect(page.getByText("2 selected")).toBeVisible();
    const link = page.getByRole("link", { name: "Compare selected" });
    await expect(link).toHaveAttribute("href", "/compare?lanes=presita%2Csitb");
    await link.click();
    await expect(page).toHaveURL(/\/compare\?lanes=presita%2Csitb$/);
    await expect(h1(page)).toHaveText("Compare");
    await expect(h1(page)).toBeFocused();
    // The comparison is for those two swimlanes: it says so, and asks for the file to compare them on.
    await expect(page.getByRole("button", { name: "2 swimlanes" })).toBeVisible();
    await expect(page.getByText("Pick one logical file. Its settings are listed with one column for each of the 2 swimlanes.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Choose a file" }).first()).toBeVisible();
    await finish(page, csp, harness);
  });
});
