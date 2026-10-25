// "As served" (T6, slice C) as a person meets it: the page the MCP tool `get_served_file` sends people to, a file as the config-server
// answers for one application, tenant and channel. Each request is a round trip through the swimlane's agent, so the page asks only
// when told to: the address holds the selection (a link, a reload and Back all mean the same), nothing is asked until the button is
// pressed, a refused or failed ask is NOT tried again by the page, and "Ask again" is the person's to press.
//
// The text is shown as the hub sent it (M1): the web masks nothing, and the real hub sends no `masks` yet (D128). So a served text
// that holds a value the config-server substituted is shown as received, and a text the hub did mask shows hidden values, with no
// Reveal and no way to copy the text. Real browser, the Rust fixture's headers and CSP, a mock scenario of the hub; each journey ends
// in `finish`.
import type { Page } from "@playwright/test";
import { maskOf, servedFile, servedText, withheldServedFile } from "../../src/mocks/design/served/data.ts";
import { DIST, expect, finish, h1, open, test } from "./support";
import { MCP_SERVED_LINK, SERVED_FILE, echoed, emit, live, servedAnswer, servedCalls, settled } from "./t6";
import { whereIs } from "./t5";

test.use({ dist: DIST });

const form = (page: Page) => page.getByRole("form", { name: "What to ask the config-server for" });
const file = (page: Page) => form(page).getByRole("textbox", { name: "File" });
const application = (page: Page) => form(page).getByRole("textbox", { name: "Application" });
const tenant = (page: Page) => form(page).getByRole("combobox", { name: "Tenant" });
const channel = (page: Page) => form(page).getByRole("textbox", { name: "Channel" });
const show = (page: Page) => form(page).getByRole("button", { name: "Show served text" });
const answer = (page: Page) => page.getByRole("region", { name: "The config-server's answer" });
const facts = (page: Page) => answer(page).getByRole("group", { name: "About this answer" });
const text = (page: Page) => answer(page).getByRole("table", { name: "Served text of connection-pool.yml" });
const askAgain = (page: Page) => answer(page).getByRole("button", { name: "Ask again" });

test.describe("a link from the MCP tool", () => {
  test.use({ scenario: "design-served-text" });

  test("opens the page with the selection filled in and asks once; changing the tenant and pressing Show goes to the new selection, and Back returns to the earlier one", async ({ page, harness, csp }) => {
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await live(page);
    await settled(harness);
    await expect(file(page)).toHaveValue(SERVED_FILE);
    await expect(application(page)).toHaveValue("tx-infinity-core");
    await expect(tenant(page)).toHaveValue("sit7");
    await expect(channel(page)).toHaveValue("");
    await expect(facts(page)).toContainText("tx-infinity-core · sit7 · no channel");
    await expect(facts(page)).toContainText("Resource file");
    await expect(text(page)).toContainText("size: 24");
    expect(servedCalls(harness), "one ask for the link").toEqual([{ status: 200, application: "tx-infinity-core", tenant: "sit7", channel: null, file: SERVED_FILE }]);

    // Choosing and typing ask nothing; pressing the button does, and the address says what was asked.
    await tenant(page).selectOption("sit9");
    await page.waitForTimeout(300);
    expect(servedCalls(harness)).toHaveLength(1);
    await show(page).click();
    await expect(facts(page)).toContainText("tx-infinity-core · sit9 · no channel");
    expect(new URL(page.url()).searchParams.get("tenant")).toBe("sit9");
    expect(servedCalls(harness).map((c) => c.tenant)).toEqual(["sit7", "sit9"]);

    // Back is the earlier selection: the address, the fields and the answer all say sit7.
    await page.goBack();
    await expect(page).toHaveURL(/tenant=sit7/);
    await expect(tenant(page)).toHaveValue("sit7");
    await expect(facts(page)).toContainText("tx-infinity-core · sit7 · no channel");
    await page.goForward();
    await expect(tenant(page)).toHaveValue("sit9");
    await expect(facts(page)).toContainText("tx-infinity-core · sit9 · no channel");
    await finish(page, csp, harness);
  });

  test("a channel is part of the selection: it is typed, sent, shown in the answer, and kept in the address; clearing it asks without one", async ({ page, harness, csp }) => {
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await expect(facts(page)).toContainText("no channel");
    await channel(page).fill("beta");
    await show(page).click();
    await expect(facts(page)).toContainText("channel beta");
    expect(new URL(page.url()).searchParams.get("channel")).toBe("beta");
    expect(servedCalls(harness).filter((c) => c.channel === "beta")).toEqual([{ status: 200, application: "tx-infinity-core", tenant: "sit7", channel: "beta", file: SERVED_FILE }]);
    // Without the channel it is the selection of the link again: the address has no channel, the answer says so, and nothing in the
    // request names one. (An answer kept from the last minute is shown with the time it was asked, so this may not be a new ask.)
    await channel(page).fill("");
    await show(page).click();
    await expect(facts(page)).toContainText("no channel");
    expect(new URL(page.url()).searchParams.has("channel")).toBe(false);
    expect(servedCalls(harness).filter((c) => c.channel === "beta")).toHaveLength(1);
    await finish(page, csp, harness);
  });

  test("Ask again, and Show for the selection already shown, each ask the config-server once more: the answer is live and nothing tells the page it changed", async ({ page, harness, csp }) => {
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await live(page);
    await settled(harness);
    await expect(text(page)).toBeVisible();
    const before = servedCalls(harness).length;
    await askAgain(page).click();
    await expect.poll(() => servedCalls(harness).length).toBe(before + 1);
    await show(page).click();
    await expect.poll(() => servedCalls(harness).length).toBe(before + 2);
    // The address did not change, and neither did what is shown.
    await expect(page).toHaveURL(new RegExp(`tenant=sit7`));
    await expect(text(page)).toBeVisible();
    await finish(page, csp, harness);
  });

  test("frames about the files, the drift and the pickup of the swimlane do not ask the config-server again: no event names this answer", async ({ page, harness, csp }) => {
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await live(page);
    await settled(harness);
    const before = servedCalls(harness).length;
    emit(harness, { type: "file_observed", swimlane: "sitb", path: SERVED_FILE, hash: "a".repeat(64), source: "scan" });
    emit(harness, { type: "drift_changed", swimlane: "sitb", paths: [SERVED_FILE] });
    emit(harness, { type: "pickup_changed", swimlane: "sitb", services: [{ namespace: "sitb", name: "tx-infinity-core" }] });
    emit(harness, { type: "findings_changed", swimlane: "sitb" });
    await page.waitForTimeout(800);
    expect(servedCalls(harness), "the answer was not asked for again").toHaveLength(before);
    await finish(page, csp, harness);
  });

  test("the address is the selection, and nothing is asked on a cold open of the bare page", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/swimlanes/sitb/served", "As served");
    await expect(page.getByText("Choose what to ask for", { exact: true })).toBeVisible();
    await live(page);
    await settled(harness);
    expect(servedCalls(harness)).toEqual([]);
    // A selection that is complete is asked for with the button, and then it is in the address.
    await file(page).fill(SERVED_FILE);
    await application(page).fill("tx-infinity-core");
    await tenant(page).selectOption("sit7");
    await page.waitForTimeout(300);
    expect(servedCalls(harness)).toEqual([]);
    await show(page).click();
    await expect(text(page)).toBeVisible();
    await expect(page).toHaveURL(/application=tx-infinity-core/);
    expect(servedCalls(harness)).toHaveLength(1);
    await finish(page, csp, harness);
  });
});

test.describe("an address that cannot be a request", () => {
  test.use({ scenario: "design-served-text" });

  test("is not sent: the fields say what is wrong without repeating what was given, and nothing is asked of the hub", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/swimlanes/sitb/served?file=..%2F..%2Fetc%2Fpasswd&application=tx%20core%3Cb%3E&tenant=SIT%207&channel=a%2Fb", "As served");
    await live(page);
    await settled(harness);
    await expect(page.getByText("That selection can't be asked for", { exact: true })).toBeVisible();
    await expect(form(page).getByText("That is not a path below the swimlane's root.")).toBeVisible();
    await expect(form(page).getByText("Use letters, digits, . _ and -, starting with a letter, digit or _, at most 128 characters").first()).toBeVisible();
    await expect(form(page).getByText("Use lower case letters, digits, - and _, starting with a letter or digit, at most 63 characters")).toBeVisible();
    // What was in the address stays in the fields (it is the person's to correct) and is nowhere else on the page.
    await expect(answer(page)).toHaveCount(0);
    expect(servedCalls(harness)).toEqual([]);
    await show(page).click();
    await page.waitForTimeout(300);
    expect(servedCalls(harness)).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("a 404 from the config-server", () => {
  test.use({ scenario: "design-served-not-served" });

  test("is a state, not a fault: nothing for this selection, with the request ID, and Ask again asks once more and shows the text when it is there", async ({ page, harness, csp }) => {
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    const notServed = answer(page).locator("[data-slot='not-served']");
    await expect(notServed).toBeVisible();
    await expect(notServed).toContainText("Not served for this selection");
    await expect(notServed).toContainText("req-mock-0001");
    await expect(answer(page).getByRole("alert")).toHaveCount(0);
    await live(page);
    await settled(harness);
    expect(servedCalls(harness).map((c) => c.status), "a 404 is not tried again").toEqual([404]);
    // The config-server has caught up: the same selection, asked again.
    harness.setScenario("design-served-text");
    await notServed.getByRole("button", { name: "Ask again" }).click();
    await expect(text(page)).toBeVisible();
    await expect(notServed).toHaveCount(0);
    expect(servedCalls(harness).map((c) => c.status)).toEqual([404, 200]);
    await finish(page, csp, harness, { expected: [404] });
  });
});

test.describe("a 503 (the agent is offline)", () => {
  test.use({ scenario: "design-served-agent-offline" });

  test("is asked for ONCE: the page does not try again by itself, shows the error with the request ID at once, and Retry is the person's", async ({ page, harness, csp }) => {
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await live(page);
    const error = answer(page).getByRole("alert");
    await expect(error).toBeVisible({ timeout: 2_000 });
    await expect(error).toContainText("Couldn't load the served file");
    await expect(error).toContainText("req-mock-0001");
    await expect(answer(page).getByRole("status").filter({ hasText: "Agent for sitb is offline" })).toBeVisible();
    // The default for a read is two more tries after a 503 (src/api/retry.ts), with a pause before each: wait past all of them.
    await settled(harness, 2_500, 12_000);
    expect(servedCalls(harness).map((c) => c.status), "exactly one request: no retry, no second ask of any kind").toEqual([503]);
    // The agent is back. Retry asks once more.
    harness.setScenario("design-served-text");
    await error.getByRole("button", { name: "Retry" }).click();
    await expect(text(page)).toBeVisible();
    expect(servedCalls(harness).map((c) => c.status)).toEqual([503, 200]);
    await finish(page, csp, harness, { expected: [503] });
  });
});

test.describe("the text of a served file", () => {
  test.use({ scenario: "design-served-text" });

  test("is shown as the hub sent it: a value the config-server substituted is in the text (the real hub sends no masks yet), and nothing is hidden, revealed or copied by the page", async ({ page, harness, context, csp }) => {
    const secret = "hunter2-served-value";
    harness.setScenario(
      servedAnswer("design-served-text", (query) => {
        const body = servedText({ password: secret });
        return echoed(query, servedFile({ text: body }));
      }),
    );
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await expect(text(page)).toContainText(`password: ${secret}`);
    // The page does not pretend: it draws no hidden value (the hub gave no mask), and offers no Reveal and no copy of the text.
    await expect(page.locator("[data-slot='hidden-value']")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /reveal|hide value/i })).toHaveCount(0);
    await expect(text(page).getByRole("button")).toHaveCount(0);
    await expect(answer(page).getByRole("button", { name: /copy/i })).toHaveCount(2); // the path and the hash, which are not the text
    await expect(answer(page).getByRole("button", { name: /copy/i }).first()).toHaveAccessibleName("Copy path");
    await expect(answer(page).getByRole("button", { name: /copy/i }).nth(1)).toHaveAccessibleName("Copy hash");
    // Select-all and copy gives what is on screen, as for any text; the page has no button that puts the text on the clipboard.
    await finish(page, csp, harness);
  });

  test("masked by the hub: hidden-value tokens in place of the values, no Reveal (a served mask names no setting), and the marker is the only thing that stood there", async ({ page, harness, context, csp }) => {
    harness.setScenario("design-served-masked");
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    const token = page.locator("[data-slot='hidden-value']");
    await expect(token).toHaveCount(1);
    await expect(token).toContainText("Hidden value");
    await expect(token).toContainText("22 chars");
    await expect(text(page)).not.toContainText("REDACTED");
    await expect(page.getByRole("button", { name: /reveal/i })).toHaveCount(0);
    await expect(text(page).getByRole("button")).toHaveCount(0);
    expect(await whereIs(page, "REDACTED"), "the marker's words are not on the page, only the token").toEqual([]);
    await finish(page, csp, harness);
  });

  test("a hub that gets its masks wrong shows less, not more: a mask over a value that is not a marker hides the whole line, and the value is in no markup, name, title or clipboard", async ({ page, harness, context, csp }) => {
    const secret = "hunter2-hostile-hub-value";
    const body = servedText({ password: secret });
    harness.setScenario(servedAnswer("design-served-text", (query) => echoed(query, servedFile({ text: body, masks: [maskOf(body, 7, secret)] }))));
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await expect(text(page)).toBeVisible();
    await expect(text(page)).toContainText("size: 24");
    await expect(text(page)).not.toContainText(secret);
    expect(await whereIs(page, secret), "where the value is").toEqual([]);
    await expect(page.getByRole("button", { name: /reveal/i })).toHaveCount(0);
    await finish(page, csp, harness);
  });
});

test.describe("a file the hub withholds", () => {
  test.use({ scenario: "design-served-withheld" });

  test("says the content is withheld, shows no text and no placeholders, and the hash has no copy button", async ({ page, harness, csp }) => {
    await open(page, harness.url, `/swimlanes/sitb/served?application=tx-infinity-core&file=${encodeURIComponent("security/keystore.jks")}&tenant=sit7`, "As served");
    await expect(answer(page).getByText("Content withheld", { exact: true })).toBeVisible();
    await expect(answer(page).getByRole("table")).toHaveCount(0);
    await expect(facts(page)).toContainText(withheldServedFile().hash.slice(0, 7));
    await expect(answer(page).getByRole("button", { name: "Copy hash" })).toHaveCount(0);
    await expect(answer(page).getByText("The engine reads")).toHaveCount(0);
    await finish(page, csp, harness);
  });

  test("an answer that is withheld wins over a text it carries: the text is not drawn", async ({ page, harness, context, csp }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: harness.url });
    harness.setScenario(servedAnswer("design-served-withheld", (query) => echoed(query, withheldServedFile({ text: "password: leaked-by-a-broken-hub" }))));
    await open(page, harness.url, `/swimlanes/sitb/served?application=tx-infinity-core&file=${encodeURIComponent("security/keystore.jks")}&tenant=sit7`, "As served");
    await expect(answer(page).getByText("Content withheld", { exact: true })).toBeVisible();
    await expect(page.locator("body")).not.toContainText("leaked-by-a-broken-hub");
    expect(await whereIs(page, "leaked-by-a-broken-hub")).toEqual([]);
    await finish(page, csp, harness);
  });
});

test.describe("the other answers", () => {
  test("an answer that is not text, and a file of 0 bytes, are each said in words", async ({ page, harness, csp }) => {
    harness.setScenario("design-served-not-text");
    await open(page, harness.url, `/swimlanes/sitb/served?application=tx-infinity-core&file=${encodeURIComponent("tx-infinity-api/logo.png")}&tenant=sit7`, "As served");
    await expect(answer(page).getByText("Not shown as text", { exact: true })).toBeVisible();
    harness.setScenario("design-served-empty-file");
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await expect(answer(page).getByText("The config-server served an empty file", { exact: true })).toBeVisible();
    await expect(h1(page)).toHaveText("As served");
    await finish(page, csp, harness);
  });

  test("the comparison with the rendered view is said not to be there yet, and no control pretends to offer it", async ({ page, harness, csp }) => {
    harness.setScenario("design-served-text");
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await expect(answer(page)).toContainText("Comparison with the rendered view is not available yet.");
    await expect(page.getByRole("button", { name: /rendered|compare/i })).toHaveCount(0);
    await expect(page.getByRole("switch")).toHaveCount(0);
    await expect(page.getByRole("tab", { name: /rendered/i })).toHaveCount(0);
    await finish(page, csp, harness);
  });
});

test.describe("the file view's As served link", () => {
  test.use({ scenario: "design-file-core" });

  test("opens the page with the file and the file's tenant filled in, no application, and nothing asked; the person then chooses the application and asks", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml", "tx-infinity-core.yml");
    const link = page.getByRole("link", { name: "As served" });
    await expect(link).toHaveAttribute("href", "/swimlanes/sitb/served?file=tx-infinity-api%2Ftx-infinity-core-sit7.yml&tenant=sit7");
    await link.click();
    await expect(h1(page)).toHaveText("As served");
    await expect(page).toHaveURL(/\/swimlanes\/sitb\/served\?file=tx-infinity-api%2Ftx-infinity-core-sit7\.yml&tenant=sit7$/);
    await expect(file(page)).toHaveValue("tx-infinity-api/tx-infinity-core-sit7.yml");
    await expect(application(page)).toHaveValue("");
    await expect(page.getByText("Choose what to ask for", { exact: true })).toBeVisible();
    expect(servedCalls(harness), "choosing a file asks nothing").toEqual([]);
    // The hub that answers from here on is the one with the served answers: the person gives the application and presses the button.
    harness.setScenario("design-served-text");
    await application(page).fill("tx-infinity-core");
    await expect(tenant(page)).toHaveValue("sit7");
    await show(page).click();
    await expect(facts(page)).toContainText("tx-infinity-core · sit7 · no channel");
    expect(servedCalls(harness).map((c) => [c.application, c.tenant, c.file])).toEqual([["tx-infinity-core", "sit7", "tx-infinity-api/tx-infinity-core-sit7.yml"]]);
    await finish(page, csp, harness);
  });

  test("a file that belongs to no tenant is linked without one", async ({ page, harness, csp }) => {
    harness.setScenario("design-file-lineending");
    await open(page, harness.url, "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-gateway.yml", "tx-infinity-gateway.yml");
    await expect(page.getByRole("link", { name: "As served" })).toHaveAttribute("href", "/swimlanes/sitb/served?file=tx-infinity-api%2Ftx-infinity-gateway.yml");
    await finish(page, csp, harness);
  });

  test("a file whose content the hub withholds has no link: there is nothing to compare it with, and the page would show nothing of it", async ({ page, harness, csp }) => {
    harness.setScenario("design-file-denied");
    await open(page, harness.url, "/swimlanes/sitb/files?path=security%2Fkeystore.jks", "keystore.jks");
    await expect(page.getByRole("button", { name: "History" })).toBeVisible();
    await expect(page.getByRole("link", { name: "As served" })).toHaveCount(0);
    await finish(page, csp, harness);
  });
});

test.describe("a person with no role yet", () => {
  test.use({ scenario: "pending" });

  test("opening the link shows the access-pending screen and the page asks the hub for nothing but who they are", async ({ page, harness, csp }) => {
    await page.goto(`${harness.url}${MCP_SERVED_LINK}`);
    await expect(page.getByRole("heading", { level: 1, name: "Access requested" })).toBeVisible();
    await expect(form(page)).toHaveCount(0);
    await page.waitForTimeout(500);
    expect(harness.requests().filter((r) => r.kind !== "stream").map((r) => `${r.method} ${r.path.split("?")[0]}`)).toEqual(["GET /api/v1/me"]);
    await finish(page, csp, harness, { expected: [403] });
  });
});

test.describe("an answer that is a minute old", () => {
  test.use({ scenario: "design-served-text" });

  test("says it may be out of date and offers to read it again; nothing asks the config-server until the person does, and asking clears the notice", async ({ page, harness, csp }) => {
    await page.clock.install();
    await open(page, harness.url, MCP_SERVED_LINK, "As served");
    await live(page);
    await settled(harness);
    await expect(text(page)).toBeVisible();
    const stale = answer(page).getByRole("status").filter({ hasText: "This view may be out of date" });
    await expect(stale).toHaveCount(0);
    const asked = servedCalls(harness).length;
    await page.clock.runFor(62_000);
    await expect(stale).toBeVisible();
    // The answer is still on screen, and nothing has asked for a new one by itself.
    await expect(text(page)).toBeVisible();
    expect(servedCalls(harness)).toHaveLength(asked);
    await stale.getByRole("button", { name: "Refresh" }).click();
    await expect.poll(() => servedCalls(harness).length).toBe(asked + 1);
    await expect(stale).toHaveCount(0);
    await finish(page, csp, harness);
  });
});
