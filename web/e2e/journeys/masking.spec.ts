// A masked value, as a person meets it (T5, slice C): the hub's markers in place of the values, Reveal for an Editor (a reason, one
// audited request, the value for a short while, Hide, and focus where it belongs), what the hub's refusals say, a Viewer who cannot
// ask, a file whose content is withheld, and a hub that gets its masks wrong, which the page answers by showing less. Each journey
// ends in `finish`: no policy violation, no console error, no request the scenario could not answer.
//
// The "value" is the design's made-up one (`REVEALED_VALUE`) and the broken hub's raw one (`RAW_SECRET`). Where a value must not be,
// `whereIs` looks in the markup, the text, the accessible tree, every title and label, and the clipboard after a select-all and a copy.
import type { Page } from "@playwright/test";
import { REVEALED_VALUE, SECRET_SETTING } from "../../src/mocks/design/masking/data.ts";
import type { Harness } from "../harness/serve.ts";
import { DIST, allowClipboard, callsTo, expect, expectNoTrace, finish, h1, open, test, writesOf } from "./support";
import { PRIYA, RAW_SECRET, asRole, malformedMaskScenario, whereIs } from "./t5";

test.use({ dist: DIST });

const FILE = "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const REASON = "Teller login fails on sit7; checking the secret matches the vault entry (OPS-2291).";
const reveal = (page: Page) => page.getByRole("button", { name: `Reveal value of ${SECRET_SETTING}` });
const hide = (page: Page) => page.getByRole("button", { name: `Hide value of ${SECRET_SETTING}` });
const dialog = (page: Page) => page.getByRole("alertdialog", { name: "Reveal masked value?" });
const shown = (page: Page) => page.locator("[data-slot='revealed-value']");
const token = (page: Page) => page.locator("[data-slot='hidden-value']");
const reveals = (harness: Harness) => callsTo(harness, "POST", "/api/v1/swimlanes/sitb/files/reveal");

async function openFile(page: Page, harness: Harness, context: Parameters<typeof allowClipboard>[0]): Promise<void> {
  await allowClipboard(context, harness.url);
  await open(page, harness.url, FILE, "tx-infinity-core.yml");
  await expect(token(page).first()).toBeVisible();
}

/** Ask for the value with the reason, with the mouse as most people do. Leaves the page with the value shown. */
async function askFor(page: Page): Promise<void> {
  await reveal(page).click();
  await dialog(page).getByRole("textbox").fill(REASON);
  await dialog(page).getByRole("button", { name: "Reveal value" }).click();
}

test.describe("the masked view", () => {
  test.use({ scenario: "design-masking-reveal" });

  test("shows a token where the hub hid a value, and the value is nowhere on the page, in a name, a title or the clipboard, before, while and after it is revealed", async ({ page, harness, context, csp }) => {
    await openFile(page, harness, context);
    // The token says what it is, how long the value was, and which setting it is for (for a screen reader); it holds no part of the value.
    await expect(token(page).first()).toContainText("Hidden value");
    await expect(token(page).first()).toContainText("22 chars");
    await expect(token(page).first()).toContainText(`for setting ${SECRET_SETTING}`);
    await expect(page.getByRole("table", { name: "tx-infinity-core-sit7.yml" })).not.toContainText("generic_secret");
    expect(await whereIs(page, REVEALED_VALUE), "before the reveal").toEqual([]);

    // While it is shown, it is in one element, with its words, and in no name, title or attribute.
    await askFor(page);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    const during = await whereIs(page, REVEALED_VALUE);
    expect(during.filter((where) => where !== "the markup" && where !== "the text" && where !== "the accessible tree" && where !== "the clipboard"), "an attribute").toEqual([]);

    // Hidden again, it is gone from every place, and the clipboard never had it: the page does not copy it.
    await hide(page).click();
    await expect(shown(page)).toHaveCount(0);
    expect(await whereIs(page, REVEALED_VALUE), "after Hide").toEqual([]);
    await expectNoTrace(page, csp, harness, REVEALED_VALUE);
    await finish(page, csp, harness);
  });

  test("an Editor's Reveal with a reason sends one audited request with the reason, shows the value with Hide focused, and Hide takes it away with focus back on Reveal", async ({ page, harness, context, csp }) => {
    // An Editor is the lowest role that may reveal; the design's person is an Admin, so the same hub with a different role.
    harness.setScenario(asRole("design-masking-reveal", "editor", { id: PRIYA, name: "Priya Shah", email: "priya.shah@example.test" }));
    await openFile(page, harness, context);
    expect(reveals(harness)).toEqual([]);
    await reveal(page).click();
    await expect(dialog(page)).toBeVisible();
    // A reason is required: without one the dialog says so and nothing is sent.
    await dialog(page).getByRole("button", { name: "Reveal value" }).click();
    await expect(dialog(page).getByText("Enter a reason")).toBeVisible();
    expect(reveals(harness)).toEqual([]);
    await dialog(page).getByRole("textbox").fill(REASON);
    await dialog(page).getByRole("button", { name: "Reveal value" }).click();
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await expect(dialog(page)).toHaveCount(0);
    await expect(hide(page)).toBeFocused();
    await expect(page.getByText("Revealed · recorded in the audit log")).toBeVisible();
    const sent = reveals(harness);
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({ status: 200, csrf: "ok" });
    // The contract wants the key on every write; the hub does not claim it for a reveal (a stored answer would be the value), so a
    // second request is a second record and nothing in the page sends one by itself.
    expect(sent[0]?.idempotencyKey).toBeTruthy();
    expect(JSON.parse(sent[0]?.body ?? "{}")).toEqual({ path: "tx-infinity-api/tx-infinity-core-sit7.yml", setting: SECRET_SETTING, reason: REASON });
    // The only write was the reveal, and the value has no copy button.
    expect(writesOf(harness)).toHaveLength(1);
    await expect(page.getByRole("table", { name: "tx-infinity-core-sit7.yml" }).getByRole("button", { name: /copy/i })).toHaveCount(0);
    await hide(page).click();
    await expect(shown(page)).toHaveCount(0);
    await expect(reveal(page)).toBeFocused();
    await expect(token(page).first()).toBeVisible();
    await finish(page, csp, harness);
  });

  test("the value goes by itself after 30 seconds (a fake clock), with focus handed to Reveal if Hide had it", async ({ page, harness, context, csp }) => {
    await page.clock.install();
    await openFile(page, harness, context);
    await askFor(page);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await expect(hide(page)).toBeFocused();
    await page.clock.runFor(29_000);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await page.clock.runFor(1_500);
    await expect(shown(page)).toHaveCount(0);
    await expect(page.locator("p[aria-live='polite']")).toHaveText("Value hidden.");
    await expect(reveal(page)).toBeFocused();
    expect(await whereIs(page, REVEALED_VALUE)).toEqual([]);
    expect(reveals(harness)).toHaveLength(1);
    // The next reveal is a new request with its own 30 seconds: the timer of the first one is not left running.
    await askFor(page);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await page.clock.runFor(20_000);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await page.clock.runFor(11_000);
    await expect(shown(page)).toHaveCount(0);
    expect(reveals(harness)).toHaveLength(2);
    await finish(page, csp, harness);
  });

  test("the value goes when the window loses focus and when the tab is hidden, and when the person moves to another file", async ({ page, harness, context, csp }) => {
    await openFile(page, harness, context);
    // The window loses focus (a headless browser has no other window to take it, so the event is dispatched, as the browser would).
    await askFor(page);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await page.evaluate(() => window.dispatchEvent(new Event("blur")));
    await expect(shown(page)).toHaveCount(0);
    expect(await whereIs(page, REVEALED_VALUE)).toEqual([]);
    // The tab is hidden.
    await askFor(page);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await page.evaluate(() => {
      Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await expect(shown(page)).toHaveCount(0);
    await page.evaluate(() => {
      Reflect.deleteProperty(document, "visibilityState");
      document.dispatchEvent(new Event("visibilitychange"));
    });
    // Another file: what was revealed belongs to the file it was revealed in.
    await askFor(page);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await page.getByRole("tab", { name: /^Git/ }).click();
    await expect(shown(page)).toHaveCount(0);
    await expect(page.locator("body")).not.toContainText(REVEALED_VALUE);
    await page.getByRole("tab", { name: /^NFS/ }).click();
    await expect(token(page).first()).toBeVisible();
    await expect(shown(page)).toHaveCount(0);
    expect(reveals(harness)).toHaveLength(3);
    await finish(page, csp, harness);
  });

  test("only one value is shown at a time: another file's page never has the first one's", async ({ page, harness, context, csp }) => {
    harness.setScenario("design-masking-multi");
    await allowClipboard(context, harness.url);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    // Three masks: one that names a setting (Reveal), one that names none (a token only), one found by its form (a token only).
    await expect(token(page)).toHaveCount(3);
    await expect(page.getByRole("button", { name: /^Reveal value of/ })).toHaveCount(1);
    await askFor(page);
    await expect(shown(page)).toHaveText(REVEALED_VALUE);
    await expect(token(page)).toHaveCount(2);
    await finish(page, csp, harness);
  });
});

test.describe("when the hub says no", () => {
  const cases = [
    { scenario: "design-masking-throttled", status: 429, says: ["Too many requests", "Reveals are limited to 30 a minute. Wait 30 seconds, then try again. Nothing was shown."] },
    { scenario: "design-masking-unavailable", status: 503, says: ["Service unavailable", "The audit log could not take the record, so nothing was shown. Try again in 5 seconds."] },
    { scenario: "design-masking-gone", status: 404, says: ["Not found", "There is nothing to reveal there"] },
  ] as const;

  for (const { scenario, status, says } of cases) {
    test(`a ${String(status)} stays in the dialog with the reason it gives, shows nothing, and sends exactly one request (a second try is the person's, and is a second record)`, async ({ page, harness, context, csp }) => {
      harness.setScenario(scenario);
      await openFile(page, harness, context);
      await reveal(page).click();
      await dialog(page).getByRole("textbox").fill(REASON);
      await dialog(page).getByRole("button", { name: "Reveal value" }).click();
      for (const text of says) await expect(dialog(page)).toContainText(text);
      await expect(dialog(page).getByRole("textbox")).toHaveValue(REASON);
      await expect(shown(page)).toHaveCount(0);
      // Nothing retries by itself: a moment later it is still one request, and the dialog is where it was.
      await page.waitForTimeout(1_500);
      expect(reveals(harness)).toHaveLength(1);
      expect(reveals(harness)[0]).toMatchObject({ status });
      await expect(dialog(page)).toBeVisible();
      expect(await whereIs(page, REVEALED_VALUE)).toEqual([]);
      // Trying again is a press of the button, and it is a second request.
      await dialog(page).getByRole("button", { name: "Reveal value" }).click();
      await expect.poll(() => reveals(harness).length).toBe(2);
      // Cancel closes it, and focus goes back to Reveal.
      await dialog(page).getByRole("button", { name: "Cancel" }).click();
      await expect(dialog(page)).toHaveCount(0);
      await expect(reveal(page)).toBeFocused();
      await finish(page, csp, harness, { expected: [status] });
    });
  }
});

test.describe("a Viewer", () => {
  test.use({ scenario: "design-masking-viewer" });

  test("sees the token and a Reveal button that is disabled and says what it needs; pressing it opens nothing and asks the hub for nothing", async ({ page, harness, context, csp }) => {
    await openFile(page, harness, context);
    const button = reveal(page);
    await expect(button).toBeVisible();
    await expect(button).toHaveAttribute("aria-disabled", "true");
    await button.hover();
    await expect(page.getByRole("tooltip")).toContainText("Needs the Editor role. Ask an Admin to grant it.");
    // A button that is off still takes the press (it is off, not gone: aria-disabled), and does nothing with it.
    await button.click({ force: true });
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(reveals(harness)).toEqual([]);
    expect(await whereIs(page, REVEALED_VALUE)).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("a file whose content is withheld", () => {
  test.use({ scenario: "design-file-denied" });

  test("says so, shows the name, the size and the hash, and offers no edit and nothing to copy but the file's path and link", async ({ page, harness, context, csp }) => {
    await allowClipboard(context, harness.url);
    await open(page, harness.url, "/swimlanes/sitb/files?path=security%2Fkeystore.jks", "keystore.jks");
    await expect(page.getByText("Content withheld: sensitive file type")).toBeVisible();
    const card = page.getByRole("tabpanel", { name: /^NFS/ });
    await expect(card).toContainText("Name");
    await expect(card).toContainText("keystore.jks");
    await expect(card).toContainText("3.2 KB");
    await expect(card).toContainText("9d2c47e");
    // No text, no code, nothing to edit, upload, delete, raise, download or compare.
    for (const name of ["Edit", "Upload", "Delete", "Raise PR", "Download", "Revert", "Compare with Git", "Enable editing"]) await expect(page.getByRole("button", { name, exact: true }), name).toHaveCount(0);
    await expect(page.getByRole("table")).toHaveCount(0);
    await expect(page.getByRole("textbox")).toHaveCount(0);
    // The copy buttons that are left are for metadata that is not the content: the path and the link. None is for the hash.
    const copies = await page.getByRole("button", { name: /^Copy / }).evaluateAll((els) => els.map((el) => el.getAttribute("aria-label")));
    expect(copies.sort()).toEqual(["Copy link to this file", "Copy path"]);
    // The content is asked for once; the hub answers it without text, and the page draws none of it.
    expect(callsTo(harness, "GET", /\/files\/content\?.*source=nfs/).length).toBeGreaterThan(0);
    expect(callsTo(harness, "GET", /\/compare\?/)).toEqual([]);
    expect(writesOf(harness)).toEqual([]);
    // The History panel lists the versions' hashes, and offers to copy none of them either.
    await page.getByRole("button", { name: "History" }).click();
    const panel = page.getByRole("complementary", { name: "History" });
    await expect(panel.getByRole("listitem").first()).toBeVisible();
    await expect(panel.getByRole("button", { name: /^Copy / })).toHaveCount(0);
    await panel.getByRole("button", { name: "Close" }).click();
    // The other tabs say the same, and ask for nothing that does not exist.
    for (const tab of [/^Git/, /^Baseline/, /^Diff/]) {
      await page.getByRole("tab", { name: tab }).click();
      await expect(page.getByText("Content withheld: sensitive file type")).toBeVisible();
    }
    expect(callsTo(harness, "GET", /\/files\/content\?.*source=(git|baseline|blob)/)).toEqual([]);
    expect(callsTo(harness, "GET", /\/api\/v1\/blobs\//)).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("a hub that gets its masks wrong", () => {
  test("a mask whose span is not a marker hides the whole line: the raw value the hub left in the text is not on the page, in a name or on the clipboard, and no Reveal is offered for it", async ({ page, harness, context, csp }) => {
    harness.setScenario(malformedMaskScenario("not-a-marker"));
    await allowClipboard(context, harness.url);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await expect(page.locator("[data-slot='hidden-line']")).toHaveCount(1);
    await expect(page.locator("[data-slot='hidden-line']")).toContainText("Hidden line");
    await expect(page.getByRole("table", { name: "tx-infinity-core-sit7.yml" })).toBeVisible();
    // The other lines are there; this one is not.
    await expect(page.getByRole("table", { name: "tx-infinity-core-sit7.yml" })).toContainText("timeoutMs: 30000");
    expect(await whereIs(page, RAW_SECRET)).toEqual([]);
    expect(await whereIs(page, "clientSecret")).toEqual([]);
    await expect(reveal(page)).toHaveCount(0);
    await expectNoTrace(page, csp, harness, RAW_SECRET);
    await finish(page, csp, harness);
  });

  for (const kind of ["outside-line", "missing-line"] as const) {
    test(`a mask that cannot be placed (${kind}) refuses the whole text: nothing of the file is shown, and the raw value is nowhere`, async ({ page, harness, context, csp }) => {
      harness.setScenario(malformedMaskScenario(kind));
      await allowClipboard(context, harness.url);
      await open(page, harness.url, FILE, "tx-infinity-core.yml");
      await expect(page.getByText("This text can't be shown safely")).toBeVisible();
      await expect(page.getByText("None of the text is shown.")).toBeVisible();
      await expect(page.getByRole("table", { name: "tx-infinity-core-sit7.yml" })).toHaveCount(0);
      await expect(page.locator("body")).not.toContainText("timeoutMs");
      expect(await whereIs(page, RAW_SECRET)).toEqual([]);
      await expect(reveal(page)).toHaveCount(0);
      await expect(h1(page)).toHaveText("tx-infinity-core.yml");
      await finish(page, csp, harness);
    });
  }
});
