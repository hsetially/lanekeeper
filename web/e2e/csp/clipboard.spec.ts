// The copy buttons under the hub's own headers (T4 Phase C1b, the human's request of 2026-10-17). The question is a platform one: the
// hub sends a `Permissions-Policy` that denies the sensitive features (crates/hub/src/headers.rs) and says it leaves `clipboard-write`
// at its default, `self`. Does the browser then let the app write the clipboard from a click? This spec answers it against the real
// header (the harness proxies every page to the Rust `web_fixture`, which is the hub's header code), in a real Chromium, for the
// secret copy button (the MCP token a person has just made) and for the generic one (a request ID), and it checks what a person
// sees: the words, that nothing moves, and that the page stays clean under the production CSP.
import type { Locator, Page } from "@playwright/test";
import { expect, pageViolations, test } from "../fixtures/test";

type PolicyApi = { allowsFeature(feature: string): boolean };
// Chromium 141 still names it `featurePolicy`; the standard name is `permissionsPolicy`. Either answers the same question.
type PolicyDocument = Document & { permissionsPolicy?: PolicyApi; featurePolicy?: PolicyApi };

const allowsFeature = (page: Page, feature: string): Promise<boolean | undefined> =>
  page.evaluate((name) => {
    const doc = document as PolicyDocument;
    return (doc.permissionsPolicy ?? doc.featurePolicy)?.allowsFeature(name);
  }, feature);

async function clean(page: Page, csp: { report(): string }): Promise<void> {
  expect(await pageViolations(page)).toEqual([]);
  expect(csp.report()).toBe("");
}

const clipboardText = (page: Page): Promise<string> => page.evaluate(() => navigator.clipboard.readText());

async function box(target: Locator): Promise<{ x: number; y: number; width: number; height: number }> {
  const found = await target.boundingBox();
  if (found === null) throw new Error("the element has no box");
  return found;
}

/** Make a token on the MCP page and return the field that shows it. */
async function makeToken(page: Page, origin: string): Promise<Locator> {
  await page.goto(`${origin}/me/tokens`);
  await expect(page.getByRole("heading", { level: 1, name: "Personal MCP tokens" })).toBeVisible();
  await page.getByLabel("Label").fill("ci");
  await page.getByRole("button", { name: "Create token" }).click();
  const field = page.getByLabel("Your new token");
  await expect(field).toBeVisible();
  return field;
}

test.describe("copying under the hub's Permissions-Policy", () => {
  test.use({ scenario: "design-access-mcp-token-issued" });

  test("the policy the hub sends allows clipboard-write for the app's own origin", async ({ page, harness, context, csp }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    const response = await page.goto(`${harness.url}/me/tokens`);
    const policy = response?.headers()["permissions-policy"] ?? "";
    // It is the hub's header (it denies the sensitive features), and it does not name clipboard-write, so the default (self) holds.
    expect(policy).toContain("camera=()");
    expect(policy).toContain("geolocation=()");
    expect(policy).not.toMatch(/clipboard-write/);
    expect(await allowsFeature(page, "clipboard-write")).toBe(true);
    // The features it does deny are denied, so the check above says something.
    expect(await allowsFeature(page, "camera")).toBe(false);
    await clean(page, csp);
  });

  test("the secret copy button puts the token on the clipboard, says it will not be shown again, and moves nothing", async ({ page, harness, context, csp }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    const field = await makeToken(page, harness.url);
    const token = await field.inputValue();
    expect(token).toMatch(/^lkpat_/);
    await page.evaluate(() => navigator.clipboard.writeText("something else"));

    const button = page.getByRole("button", { name: "Copy token" });
    const before = await box(button);
    const fieldBefore = await box(field);
    // Nothing was copied by showing the token.
    expect(await clipboardText(page)).toBe("something else");

    await button.click();
    await expect(page.getByRole("status").filter({ hasText: "Copied. It will not be shown again." })).toHaveCount(1);
    await expect(button).toContainText("Copied");
    expect(await clipboardText(page)).toBe(token);

    // No layout shift: the button and the field are where they were, to the pixel, while it says Copied.
    expect(await box(button)).toEqual(before);
    expect(await box(field)).toEqual(fieldBefore);
    // The focus is where the click left it, on the button, which is still enabled.
    await expect(button).toBeFocused();
    await expect(button).toBeEnabled();
    // Not a toast, and no word of the token anywhere but the field.
    await expect(page.getByRole("region", { name: "Notifications" }).getByRole("listitem")).toHaveCount(0);
    const elsewhere = await page.evaluate((secret) => {
      const field = document.querySelector<HTMLInputElement>("input[readonly]");
      return [...document.querySelectorAll("*")].filter((el) => el !== field && [...el.attributes].some((a) => a.value.includes(secret))).length + (document.body.innerText.includes(secret) ? 1 : 0);
    }, token);
    expect(elsewhere).toBe(0);

    // After a moment the button says Copy token again.
    await expect(page.getByRole("status").filter({ hasText: "Copied. It will not be shown again." })).toHaveCount(0, { timeout: 4000 });
    expect(await box(button)).toEqual(before);

    // Put away, it is gone: the button, the words, and the token.
    await page.getByRole("button", { name: "I have stored it" }).click();
    await expect(page.getByRole("button", { name: "Copy token" })).toHaveCount(0);
    await expect(page.getByLabel("Your new token")).toHaveCount(0);
    expect(await page.evaluate((secret) => document.documentElement.outerHTML.includes(secret) || document.body.innerText.includes(secret), token)).toBe(false);
    await clean(page, csp);
  });

  test("works with the keyboard alone: Tab to the button, Enter copies, focus stays", async ({ page, harness, context, csp }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    const field = await makeToken(page, harness.url);
    const token = await field.inputValue();
    // The field has focus and selects the whole token; the next stop is the copy button.
    await expect(field).toBeFocused();
    await page.keyboard.press("Tab");
    const button = page.getByRole("button", { name: "Copy token" });
    await expect(button).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(button).toContainText("Copied");
    expect(await clipboardText(page)).toBe(token);
    await expect(button).toBeFocused();
    await clean(page, csp);
  });

  test("says so, and selects the token, when the browser has no clipboard to write (the page is not secure, or a policy took it away)", async ({ page, harness, csp }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
    });
    const field = await makeToken(page, harness.url);
    const token = await field.inputValue();
    const button = page.getByRole("button", { name: "Copy token" });
    await button.click();
    await expect(page.getByRole("status").filter({ hasText: "Couldn't copy. Select the text and copy it manually." })).toHaveCount(1);
    await expect(field).toBeFocused();
    const selected = await field.evaluate((input: HTMLInputElement) => input.value.slice(input.selectionStart ?? 0, input.selectionEnd ?? 0));
    expect(selected).toBe(token);
    // Still there, still usable: the person can copy it by hand.
    await expect(page.getByLabel("Your new token")).toHaveValue(token);
    await clean(page, csp);
  });

  test("control: if a deployment's policy did say clipboard-write=(), the browser refuses and the button says so instead of failing silently", async ({ page, harness, context, csp }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    // The hub's own header with the one feature added, only on the page itself.
    await page.route(`${harness.url}/me/tokens`, async (route) => {
      const response = await route.fetch();
      const headers = response.headers();
      await route.fulfill({ response, headers: { ...headers, "permissions-policy": `${headers["permissions-policy"] ?? ""}, clipboard-write=()` } });
    });
    const field = await makeToken(page, harness.url);
    const token = await field.inputValue();
    expect(await allowsFeature(page, "clipboard-write")).toBe(false);
    await page.getByRole("button", { name: "Copy token" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Couldn't copy. Select the text and copy it manually." })).toHaveCount(1);
    await expect(field).toBeFocused();
    expect(await field.evaluate((input: HTMLInputElement) => input.value.slice(input.selectionStart ?? 0, input.selectionEnd ?? 0))).toBe(token);
    await clean(page, csp);
  });

  test("the generic button copies a request ID, shows Copied in a bubble, and the page stays clean", async ({ page, harness, context, csp }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    await page.goto(`${harness.url}/403?req=req-test-0001`);
    const button = page.getByRole("button", { name: "Copy request ID" });
    await expect(button).toBeVisible();
    const before = await box(button);
    await button.click();
    await expect(page.locator("[data-slot='tooltip-content']").filter({ hasText: "Copied" }).first()).toBeVisible();
    await expect(page.getByRole("status").filter({ hasText: /^Copied$/ })).toHaveCount(1);
    expect(await clipboardText(page)).toBe("req-test-0001");
    expect(await box(button)).toEqual(before);
    await expect(button).toBeFocused();
    await clean(page, csp);
  });

  test("a copy that the clipboard refuses says so in the bubble and selects the text", async ({ page, harness, csp }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText: () => Promise.reject(new DOMException("not allowed", "NotAllowedError")), readText: () => Promise.resolve("") },
      });
    });
    await page.goto(`${harness.url}/403?req=req-test-0002`);
    await page.getByRole("button", { name: "Copy request ID" }).click();
    await expect(page.locator("[data-slot='tooltip-content']").filter({ hasText: "Couldn't copy. Select the text and copy it manually." }).first()).toBeVisible();
    await clean(page, csp);
  });
});
