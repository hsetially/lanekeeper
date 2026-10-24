// The accessibility rows of the T6 screens that are not "axe finds nothing" (that is a11y.spec.ts, over the cases of cases-t6-*.ts),
// read with the real keyboard and the real layout: the pickup chips (an image with a name of its own, not a stop of the Tab key, the
// words and the icon and not the colour, a tooltip that closes with Escape, nothing the page must not trust in an attribute), the
// notify and restart buttons (a name of its own, a button that is off stays in the tab order and says why), the dialogs they open
// (a name and a description, focus on Cancel or on the dialog, focus kept inside, focus given back), a typed confirmation, the
// words a refusal is announced in, and "As served" (the form and its errors, the text as a table, what it does not offer, the states).
import type { Locator, Page } from "@playwright/test";
import { expect, pageViolations, test } from "../fixtures/test";
import { modalHoldsFocus, settle } from "./axe.ts";
import { freezeDate, modal } from "./helpers.ts";
import { API, CORE_FILE, SERVICES_URL, bannerRestart, chips, confirmNotify, openConfigServerRestart, openNotify, notifyButton, patchServed, refuse, saveFile, servedUrl } from "./t6.ts";

test.use({ dist: process.env["LK_E2E_DIST"] ?? process.env["LK_DIST"] ?? "dist" });

async function open(page: Page, url: string): Promise<void> {
  await freezeDate(page);
  await page.goto(url);
  await expect(page.locator("h1").first()).toBeVisible();
  await settle(page);
}

/** What has focus, as a person would name it. */
const focused = (page: Page): Promise<string> =>
  page.evaluate(() => {
    const el = document.activeElement;
    if (el === null || el === document.body) return "(nothing)";
    return `${el.tagName.toLowerCase()} "${(el.getAttribute("aria-label") ?? el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 50)}"`;
  });

const SERVED_ADDRESS = "/swimlanes/sitb/served";

const posts = (harness: { requests(): Array<{ method: string; path: string }> }, tail: RegExp): number => harness.requests().filter((r) => r.method === "POST" && tail.test(r.path)).length;

/** The characters that must not reach an attribute as themselves (SafeText makes them visible in the text; an attribute cannot hold markup). */
const HIDDEN = /[\u202A-\u202E\u2066-\u2069\u200B-\u200F\u2060\uFEFF]/;

// ---- The pickup chips ---------------------------------------------------------------------------------------------------------------------
test.describe("the pickup chips", () => {
  test.use({ scenario: "design-pickup-states" });

  const STATE_WORDS = /^(\d+ )?(Live|Live within client cache|Live by \d\d:\d\d UTC \((client cache|time passed)\)|Needs notify or restart|Needs config-server restart)$/;

  test("a small chip is an image with a name of its own (the count and the state in words) and nothing inside it a person could reach", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes`);
    const column = await page.locator("[data-slot='pickup-chip'][data-density='compact']").evaluateAll((els) =>
      els.map((el) => ({ role: el.getAttribute("role"), name: el.getAttribute("aria-label") ?? "", tabindex: el.getAttribute("tabindex"), reachable: el.querySelectorAll("a, button, input, select, textarea, [tabindex]").length, text: (el.textContent ?? "").trim() })),
    );
    expect(column.length, "the Pickup column draws a chip for each state that is waiting").toBeGreaterThanOrEqual(2);
    for (const chip of column) {
      expect(chip.role).toBe("img");
      expect(chip.name, "the name is the count and the state").toMatch(STATE_WORDS);
      expect(chip.name).not.toMatch(/pending restart/i);
      expect(chip.reachable, `"${chip.name}" holds nothing to press`).toBe(0);
      expect(chip.tabindex, `"${chip.name}" is not a stop of the Tab key`).toBeNull();
    }
    await page.goto(`${harness.url}${SERVICES_URL}`);
    await expect(page.getByRole("tree")).toBeVisible();
    await page.getByRole("button", { name: "Open tx-infinity-api" }).click();
    await expect(page.getByRole("tree").getByText("tx-infinity-core-sit7.yml")).toBeVisible();
    for (const chip of await page.getByRole("tree").locator("[data-slot='pickup-chip']").all()) {
      await expect(chip).toHaveAttribute("role", "img");
      expect(await chip.getAttribute("aria-label")).toMatch(STATE_WORDS);
    }
  });

  test("no chip is a stop of the Tab key: from the top of the Services tab to its end, focus never lands on one (or in one)", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    await expect(page.getByRole("table", { name: "Services" })).toBeVisible();
    await page.getByRole("link", { name: "Swimlanes" }).first().focus();
    const seen = new Set<string>();
    for (let i = 0; i < 70; i++) {
      await page.keyboard.press("Tab");
      const where = await page.evaluate(() => {
        const el = document.activeElement;
        if (el === null || el === document.body) return "(nothing)";
        return `${el.closest("[data-slot='pickup-chip']") !== null ? "IN A CHIP: " : ""}${el.tagName.toLowerCase()} ${(el.getAttribute("aria-label") ?? el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 40)}`;
      });
      seen.add(where);
      expect(where, "focus is in a chip").not.toMatch(/^IN A CHIP/);
    }
    expect([...seen].some((s) => /Restart tx-infinity-core/.test(s)), "the Restart buttons are stops").toBe(true);
  });

  test("a chip says its state in words and in an icon, never in a colour alone, and the full chip carries its sentence in the page for a screen reader", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    const rows = await page
      .getByRole("table", { name: "Services" })
      .locator("[data-slot='pickup-chip']")
      .evaluateAll((els) =>
        els.map((el) => ({
          state: el.getAttribute("data-pickup"),
          icon: el.querySelector("svg") !== null,
          words: (el.querySelector("[data-slot='status-badge']")?.textContent ?? el.textContent ?? "").replace(/\s+/g, " ").trim(),
          hidden: (el.querySelector(".sr-only")?.textContent ?? "").trim(),
        })),
      );
    expect(rows.length, "seven services, seven chips").toBe(7);
    for (const row of rows) {
      expect(row.icon, `${row.state}: an icon`).toBe(true);
      expect(row.words.length, `${row.state}: words`).toBeGreaterThan(2);
      expect(row.hidden, `${row.state}: the sentence of the tooltip is in the page too`).toMatch(/\.$/);
    }
    expect(new Set(rows.map((r) => r.state)), "all four states are on the tab").toEqual(new Set(["needs_notify_or_restart", "needs_config_server_restart", "live_within_ttl", "live"]));
  });

  test("a time is a time of day in UTC (rounded up to the minute), one that passed says so, and a missing one says neither", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    const text = await page.getByRole("table", { name: "Services" }).innerText();
    expect(text).toContain("Live by 09:15 UTC");
    expect(text).toContain("(client cache)");
    expect(text).toContain("Live by 08:40 UTC");
    expect(text).toContain("(time passed)");
    expect(text).toContain("Live within client cache");
    expect(text).not.toMatch(/pending restart/i);
  });

  test("the tooltip opens with the pointer, says when the change takes effect, closes with Escape, and focus does not move", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    const restart = page.getByRole("button", { name: "Restart tx-infinity-core" });
    await restart.focus();
    const chip = page.getByRole("table", { name: "Services" }).locator("[data-slot='pickup-chip'][data-pickup='needs_notify_or_restart']").first();
    await chip.hover();
    const tip = page.getByRole("tooltip").first();
    await expect(tip).toBeVisible();
    await expect(tip).toContainText("A property file changed. This service takes it after it is notified or restarted.");
    await expect(tip).toContainText("Notifications: on (as reported by the hub)");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("tooltip")).toHaveCount(0);
    expect(await focused(page), "Escape in a tooltip leaves focus where it was").toBe('button "Restart tx-infinity-core"');
  });
});

test.describe("the pickup chips, text the page must not trust", () => {
  test.use({ scenario: "design-pickup-hostile" });

  test("no hidden or direction-changing character reaches a title or a name: the paths a service waits on are marked in the text and in the title", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    await page.getByRole("button", { name: "Open tx-infinity-api" }).click();
    await expect(page.getByRole("tree").locator("[data-slot='pickup-chip']").first()).toBeVisible();
    const raw = await page.evaluate((source) => {
      const bad = new RegExp(source, "u");
      const found: string[] = [];
      for (const el of document.querySelectorAll("*")) for (const name of ["title", "aria-label", "alt", "placeholder"]) if (bad.test(el.getAttribute(name) ?? "")) found.push(`${el.tagName.toLowerCase()}[${name}]`);
      return found;
    }, HIDDEN.source);
    expect(raw, "attributes that hold a hidden character").toEqual([]);
    const titles = await page.locator("[title^='since']").first().evaluate((el) => el.parentElement?.parentElement?.querySelector("[title*='U+']")?.getAttribute("title") ?? "");
    expect(titles, "the title of the Waiting on line holds the markers of the characters it hides").toMatch(/U\+202E/);
    // Times that are not times (after the year 9999, before 1970): the chip says no time and not a made-up one.
    const text = await page.getByRole("table", { name: "Services" }).innerText();
    expect(text).not.toMatch(/Live by/);
    expect(text).toContain("Live within client cache");
  });
});

// ---- The notify and restart buttons -------------------------------------------------------------------------------------------------------------
test.describe("the notify and restart buttons", () => {
  test.use({ scenario: "design-pickup-states" });

  test("the banner's Restart config-server has a name of its own, apart from the Restart button of a service that is called config-server", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    const main = page.locator("main");
    const snapshot = await main.ariaSnapshot();
    const names = [...snapshot.matchAll(/^\s*- button "([^"]+)"/gm)].map((m) => m[1] ?? "").filter((n) => /^(Restart|Notify)/.test(n));
    expect(names.length, `the buttons that restart or notify: ${names.join(" | ")}`).toBeGreaterThanOrEqual(9);
    expect(names.filter((n, i) => names.indexOf(n) !== i), "two buttons that do different things have one name").toEqual([]);
    const row = main.getByRole("button", { name: "Restart config-server", exact: true });
    const banner = main.getByRole("button", { name: "Restart config-server (type its name to confirm)", exact: true });
    await expect(row).toHaveCount(1);
    await expect(banner).toHaveCount(1);
    // The visible words are the start of the name (WCAG 2.5.3, Label in Name).
    await expect(banner).toHaveText("Restart config-server");
    await expect(row).toHaveText("Restart");
    // They also do different things: the row's opens the one-service dialog, the banner's the typed one.
    await row.click();
    await expect(page.getByRole("alertdialog", { name: "Restart config-server in sitb?" })).toBeVisible();
    await expect(page.getByRole("alertdialog").getByRole("textbox")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    // The banner's is the typed restart of the config-server (this scenario has no offer for it, and its dialog says so): not the one above.
    await banner.click();
    await expect(modal(page)).toBeVisible();
    await expect(page.getByRole("alertdialog", { name: "Restart config-server in sitb?" })).toHaveCount(0);
    await expect(modal(page)).toContainText("config-server restart");
  });
});

test.describe("a button that is off", () => {
  test.use({ scenario: "design-notify-viewer" });

  test("stays in the tab order, says why (its description, and the tooltip), and a press, Enter or Space does nothing", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    const button = notifyButton(page);
    await expect(button).toHaveAttribute("aria-disabled", "true");
    await expect(button).not.toHaveAttribute("disabled", "");
    const reason = await button.evaluate((el) => document.getElementById(el.getAttribute("aria-describedby") ?? "")?.textContent ?? "");
    expect(reason, "aria-describedby points at the reason").toMatch(/Operator/);
    await button.focus();
    await expect(button).toBeFocused();
    await expect(page.getByRole("tooltip").first()).toContainText("Operator");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Space");
    // (Playwright does not click what is aria-disabled unless it is forced: a person's pointer does.)
    await button.click({ force: true });
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(posts(harness, /\/notify$/)).toBe(0);
    // Tab from the filter reaches it, and the next Tab goes on (it is a stop, not a trap).
    await page.getByRole("searchbox", { name: "Filter services" }).focus();
    await page.keyboard.press("Tab");
    expect(await focused(page)).toBe('button "Notify services"');
  });
});

test.describe("the auto-notify line", () => {
  test.use({ scenario: "design-notify-services-tab" });

  test("is a group named Auto-notify: on and off differ in a word and an icon, and the Admin's link is a link with a name", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    const line = page.getByRole("group", { name: "Auto-notify" });
    await expect(line).toContainText("Auto-notify on");
    const on = await line.evaluate((el) => ({ icon: el.querySelector("svg")?.getAttribute("class") ?? "", hidden: el.querySelector("svg")?.getAttribute("aria-hidden") }));
    expect(on.hidden, "the icon is decoration: the word says it").toBe("true");
    await expect(line.getByRole("link", { name: "Change in Admin" })).toHaveAttribute("href", "/admin/swimlanes");
    harness.setScenario("design-notify-auto-notify-off");
    await page.goto(`${harness.url}${SERVICES_URL}`);
    await expect(line).toContainText("Auto-notify off");
    const off = await line.evaluate((el) => el.querySelector("svg")?.getAttribute("class") ?? "");
    expect(off, "off and on draw different icons").not.toBe(on.icon);
  });
});

// ---- The dialogs ------------------------------------------------------------------------------------------------------------------------------
type Flow = { id: string; scenario: string; url: string; opener: (page: Page) => Locator; before?: (page: Page) => Promise<void>; prepare?: (page: Page) => Promise<void>; title: RegExp };

const FLOWS: Flow[] = [
  { id: "Notify services (the Services tab)", scenario: "design-notify-services-tab", url: SERVICES_URL, opener: notifyButton, title: /^Notify services in sitb\?$/ },
  { id: "Restart config-server (the banner)", scenario: "design-notify-config-server", url: SERVICES_URL, opener: bannerRestart, title: /^Restart csp-configuration-server in sitb\?$/ },
  { id: "Restart one service (T4's dialog)", scenario: "design-swimlanes-six", url: SERVICES_URL, opener: (page) => page.getByRole("button", { name: "Restart tx-infinity-core" }), title: /^Restart tx-infinity-core in sitb\?$/ },
  {
    id: "Restart services (the file page's banner)",
    scenario: "design-notify-file-saved",
    url: CORE_FILE,
    opener: (page) => page.getByRole("button", { name: "Restart services" }),
    prepare: saveFile,
    title: /^Restart 2 services in sitb\?$/,
  },
];

for (const flow of FLOWS) {
  test.describe(`the confirmation of ${flow.id}`, () => {
    test.use({ scenario: flow.scenario });

    test("is an alert dialog with a name and a description, starts on Cancel, keeps Tab inside, and Escape and Cancel give focus back to the button", async ({ page, harness }) => {
      test.setTimeout(90_000);
      await open(page, `${harness.url}${flow.url}`);
      await flow.prepare?.(page);
      const opener = flow.opener(page);
      for (const closeBy of ["Escape", "Cancel"] as const) {
        await opener.click();
        const dialog = page.getByRole("alertdialog");
        await expect(dialog).toBeVisible();
        await expect(dialog).toHaveAccessibleName(flow.title);
        const described = await dialog.evaluate((el) => document.getElementById(el.getAttribute("aria-describedby") ?? "")?.textContent ?? "");
        expect(described.length, "the consequence is the dialog's description").toBeGreaterThan(30);
        expect(await dialog.getAttribute("aria-modal") ?? "true", "the page behind is out of reach").toBe("true");
        expect(await focused(page), "focus starts on Cancel, the safe choice").toBe('button "Cancel"');
        const held = await modalHoldsFocus(page);
        expect(held.ok, held.detail).toBe(true);
        if (closeBy === "Escape") await page.keyboard.press("Escape");
        else await dialog.getByRole("button", { name: "Cancel" }).click();
        await expect(page.getByRole("alertdialog")).toHaveCount(0);
        await expect(opener, `focus goes back to the button that opened it (closed by ${closeBy})`).toBeFocused();
      }
    });
  });
}

test.describe("a typed confirmation", () => {
  test.use({ scenario: "design-notify-config-server" });

  test("has a label that names what to type, the button is off until it is typed exactly, and Enter in the field sends nothing", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    await openConfigServerRestart(page);
    const field = modal(page).getByRole("textbox", { name: "Type csp-configuration-server to confirm" });
    await expect(field).toBeVisible();
    const confirm = modal(page).getByRole("button", { name: "Restart csp-configuration-server" });
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    await field.fill("csp-configuration-serve");
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    await field.fill("csp-configuration-server");
    await expect(confirm).not.toHaveAttribute("aria-disabled", "true");
    await field.press("Enter");
    await confirm.press("Enter");
    await page.waitForTimeout(500);
    await expect(modal(page).getByRole("textbox")).toBeVisible();
    expect(posts(harness, /\/restart$/), "Enter never confirms a typed confirmation").toBe(0);
    await confirm.click();
    await expect(modal(page).getByText("Restart requested")).toBeVisible();
    expect(posts(harness, /\/restart$/)).toBe(1);
  });
});

test.describe("what a dialog says when it is done", () => {
  test.use({ scenario: "design-notify-services-tab" });

  test("an answer is a dialog with a name and a description, focus starts on the dialog, and Close and Escape give focus back to Notify services", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    for (const closeBy of ["Close", "Escape"] as const) {
      await openNotify(page);
      await confirmNotify(page);
      const dialog = page.getByRole("dialog", { name: "Notification sent" });
      await expect(dialog).toBeVisible();
      expect(await dialog.evaluate((el) => document.getElementById(el.getAttribute("aria-describedby") ?? "")?.textContent ?? "")).toContain("The hub accepted 2 paths");
      expect(await dialog.evaluate((el) => el === document.activeElement), "focus starts on the dialog, which is read first").toBe(true);
      const held = await modalHoldsFocus(page);
      expect(held.ok, held.detail).toBe(true);
      if (closeBy === "Close") await dialog.getByRole("button", { name: "Close" }).click();
      else await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(notifyButton(page), `closed by ${closeBy}`).toBeFocused();
    }
  });

  test("a refusal is an alert inside the dialog (what happened, why, the request id), focus stays in the dialog, and nothing is sent again by itself", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVICES_URL}`);
    await refuse(page, API.notify, 423, "Locked");
    let asked = 0;
    await page.route(API.notify, (route) => {
      asked += 1;
      return route.fallback();
    });
    await openNotify(page);
    await confirmNotify(page);
    const alert = modal(page).getByRole("alert");
    await expect(alert).toContainText("sitb is locked");
    await expect(alert).toContainText("Nothing was sent");
    await expect(alert).toContainText("req-a11y-0001");
    expect(await focused(page), "focus has not left the dialog").not.toBe("(nothing)");
    await expect(modal(page).getByRole("button", { name: "Notify services", exact: true })).not.toHaveAttribute("aria-disabled", "true");
    await page.waitForTimeout(1500);
    expect(asked, "the refused request is not sent again by the page").toBe(1);
  });
});

// ---- As served -------------------------------------------------------------------------------------------------------------------------------------
test.describe("As served", () => {
  test.use({ scenario: "design-served-text" });

  test("is a page with one heading and a title of its own, and a form that has a name, four labelled fields and one button", async ({ page, harness }) => {
    await open(page, `${harness.url}${servedUrl()}`);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("As served");
    await expect(page).toHaveTitle(/^As served · sitb/);
    const form = page.getByRole("form", { name: "What to ask the config-server for" });
    await expect(form).toBeVisible();
    for (const [name, role] of [["File", "textbox"], ["Application", "textbox"], ["Tenant", "combobox"], ["Channel", "textbox"]] as const) {
      await expect(form.getByRole(role, { name, exact: true })).toBeVisible();
    }
    await expect(form.getByRole("textbox", { name: "File" })).toHaveAttribute("aria-required", "true");
    await expect(form.getByRole("textbox", { name: "Channel" })).not.toHaveAttribute("aria-required", "true");
    await expect(form.getByRole("button")).toHaveCount(1);
  });

  test("a wrong field is marked invalid and tied to its message, the first wrong one has focus after Show, and typing again is not scolded before the next press", async ({ page, harness }) => {
    await open(page, `${harness.url}${SERVED_ADDRESS}`);
    await page.getByRole("button", { name: "Show served text" }).click();
    for (const [name, message] of [["File", "A file is required"], ["Application", "An application is required"], ["Tenant", "A tenant is required"]] as const) {
      const field = page.getByRole(name === "Tenant" ? "combobox" : "textbox", { name, exact: true });
      await expect(field).toHaveAttribute("aria-invalid", "true");
      const described = await field.evaluate((el) => (el.getAttribute("aria-describedby") ?? "").split(/\s+/).map((id) => document.getElementById(id)?.textContent ?? "").join(" "));
      expect(described, `${name}: the message is the field's description`).toContain(message);
    }
    await expect(page.getByRole("textbox", { name: "File" })).toBeFocused();
    await page.getByRole("textbox", { name: "File" }).fill("tx-infinity-api/connection-pool.yml");
    await expect(page.getByRole("textbox", { name: "File" })).not.toHaveAttribute("aria-invalid", "true");
    expect(harness.requests().filter((r) => r.path.includes("/served")), "a form that cannot be a request sends nothing").toEqual([]);
  });

  test("the text is a table named for the file with a row for each line, the facts and the status are groups, and what the page does not offer is not there", async ({ page, harness }) => {
    await open(page, `${harness.url}${servedUrl()}`);
    await expect(page.getByRole("table", { name: "Served text of connection-pool.yml" })).toBeVisible();
    await expect(page.getByRole("group", { name: "About this answer" })).toBeVisible();
    await expect(page.getByRole("group", { name: "About this text" })).toContainText("8 lines");
    const section = page.getByRole("region", { name: "The config-server's answer" });
    // No Reveal (a served mask names no setting), no copy of the text, no Rendered toggle and no comparison control: the page says so in a line.
    await expect(section.getByRole("button", { name: /reveal|copy text|copy served/i })).toHaveCount(0);
    await expect(section.getByRole("tab")).toHaveCount(0);
    await expect(section.getByRole("switch")).toHaveCount(0);
    await expect(section.getByText("Comparison with the rendered view is not available yet.")).toBeVisible();
    // The sources are links to the file view, named by the path.
    await expect(section.getByRole("list", { name: "Files the engine reads for this selection" }).getByRole("link", { name: "tx-infinity-api/connection-pool-sit7.yml" })).toHaveAttribute("href", /\/swimlanes\/sitb\/files\?path=/);
  });

  test("a value the hub masked is named in words (Hidden value, how long) and the marker is not read as the setting's value", async ({ page, harness }) => {
    harness.setScenario("design-served-masked");
    await open(page, `${harness.url}${servedUrl()}`);
    const row = page.getByRole("table", { name: "Served text of connection-pool.yml" }).getByRole("row", { name: /password/ });
    await expect(row).toContainText("Hidden value");
    await expect(row).toContainText("22 chars");
    await expect(page.getByRole("button", { name: /Reveal/ })).toHaveCount(0);
    expect(await page.locator("main").innerText()).not.toContain("********");
  });

  test("a file the hub withholds has no text and no button to copy its hash, and says why in words", async ({ page, harness }) => {
    harness.setScenario("design-served-withheld");
    await open(page, `${harness.url}${servedUrl({ file: "security/keystore.jks" })}`);
    const section = page.getByRole("region", { name: "The config-server's answer" });
    await expect(section.getByText("Content withheld")).toBeVisible();
    await expect(section.getByRole("button", { name: /copy hash/i }), "no copy of the hash of a file the hub withholds").toHaveCount(0);
    await expect(section.getByRole("button", { name: "Copy path" }), "the path is not secret").toHaveCount(1);
    await expect(section.getByRole("table")).toHaveCount(0);
  });

  test("loading is announced and the button says it is asking; the error is an alert with the request id and a Retry, and the page does not retry by itself", async ({ page, harness }) => {
    await freezeDate(page);
    let held = true;
    let asked = 0;
    await page.route(API.served, async (route) => {
      asked += 1;
      while (held) await new Promise((resolve) => setTimeout(resolve, 50));
      await route.fulfill({ status: 503, contentType: "application/problem+json", headers: { "x-request-id": "req-a11y-0001" }, body: JSON.stringify({ type: "urn:lanekeeper:problem:test", title: "Service unavailable", status: 503, request_id: "req-a11y-0001" }) });
    });
    await page.goto(`${harness.url}${servedUrl()}`);
    await expect(page.getByRole("button", { name: /Asking the config-server/ })).toBeVisible();
    await expect(page.getByRole("region", { name: "The config-server's answer" }).locator("[aria-busy='true'], [role='status']").first()).toBeVisible();
    held = false;
    const alert = page.getByRole("alert").filter({ hasText: "req-a11y-0001" });
    await expect(alert.first()).toBeVisible();
    await page.waitForTimeout(2500);
    expect(asked, "one request: a 503 is shown at once, not tried again").toBe(1);
    await page.getByRole("button", { name: "Retry" }).click();
    await expect.poll(() => asked).toBe(2);
  });
});

// Several states of the page, in the scenario that answers them.
for (const [scenario, title, url] of [
  ["design-served-not-served", "Not served for this selection", servedUrl()],
  ["design-served-not-text", "Not shown as text", servedUrl({ file: "tx-infinity-api/logo.png" })],
  ["design-served-empty-file", "The config-server served an empty file", servedUrl()],
  ["design-served-agent-offline", "Agent for sitb is offline", servedUrl()],
] as const) {
  test.describe(`As served: ${title}`, () => {
    test.use({ scenario });

    test("is said in words in a region with a name, and the one button of the state is a real button", async ({ page, harness }) => {
      await open(page, `${harness.url}${url}`);
      const section = page.getByRole("region", { name: "The config-server's answer" });
      await expect(section.getByText(title)).toBeVisible();
      for (const button of await section.getByRole("button").all()) expect(((await button.getAttribute("aria-label")) ?? (await button.innerText())).trim().length).toBeGreaterThan(0);
    });
  });
}

// ---- The file page -------------------------------------------------------------------------------------------------------------------------------
test.describe("the file page's banner", () => {
  test.use({ scenario: "design-notify-file-saved" });

  test("is a status that names what was done and holds the buttons the file calls for, each a real button with a name; the chip in the head is named", async ({ page, harness }) => {
    test.setTimeout(90_000);
    await open(page, `${harness.url}${CORE_FILE}`);
    await expect(chips(page)).toHaveCount(1);
    await saveFile(page);
    const banner = page.getByRole("status").filter({ hasText: "Saved to sitb NFS" });
    await expect(banner).toBeVisible();
    await expect(banner.getByRole("button", { name: "Notify services" })).toBeVisible();
    await expect(banner.getByRole("button", { name: "Restart services" })).toBeVisible();
    await expect(banner.getByRole("button", { name: "Dismiss" })).toBeVisible();
    // The sentence the page says about a file that services have not picked up.
    await expect(banner).toContainText("Services pick it up after a notify or a restart.");
  });
});

// ---- The production policy ---------------------------------------------------------------------------------------------------------------------
// "As served" under the Content-Security-Policy of the hub (the harness serves the page with the hub's own header code): every state of the
// page, and the steps between them (typing, choosing a tenant, Show, Ask again, a wrong form, Back), draw nothing the policy refuses. The
// served text is drawn by the shared code view and the hidden-value token (Trusted Types is required), and a hostile text is the real test of that.
test.describe("As served under the production policy", () => {
  const STATES = ["design-served-text", "design-served-masked", "design-served-withheld", "design-served-not-text", "design-served-empty-file", "design-served-not-served", "design-served-agent-offline"] as const;

  for (const scenario of STATES) {
    test(`${scenario}: the empty form, the typed form, Show, Ask again, a wrong form and Back, with no violation`, async ({ page, harness, csp }) => {
      test.setTimeout(90_000);
      harness.setScenario(scenario);
      await open(page, `${harness.url}${SERVED_ADDRESS}`);
      await expect(page.getByText("Choose what to ask for")).toBeVisible();
      await page.getByRole("textbox", { name: "File" }).fill("tx-infinity-api/connection-pool.yml");
      await page.getByRole("textbox", { name: "Application" }).fill("tx-infinity-core");
      await page.getByRole("combobox", { name: "Tenant" }).selectOption("sit7");
      await page.getByRole("textbox", { name: "Channel" }).fill("beta");
      await page.getByRole("button", { name: "Show served text" }).click();
      await expect(page).toHaveURL(/application=tx-infinity-core/);
      await expect(page.getByRole("region", { name: "The config-server's answer" })).toBeVisible();
      await page.getByRole("button", { name: /^(Ask again|Retry)$/ }).first().click();
      await expect(page.getByRole("region", { name: "The config-server's answer" })).toBeVisible();
      // A wrong form: the problems are drawn, and nothing is asked.
      await page.getByRole("textbox", { name: "Application" }).fill("not an application!");
      await page.getByRole("button", { name: "Show served text" }).click();
      await expect(page.getByText("Use letters, digits")).toBeVisible();
      await page.goBack();
      await expect(page.locator("h1")).toHaveText("As served");
      expect(await pageViolations(page), "securitypolicyviolation events").toEqual([]);
      expect(csp.report(), "violations").toBe("");
    });
  }

  test("a text that is hostile (markup, hidden characters, a bidirectional override) is drawn as text, with no violation", async ({ page, harness, csp }) => {
    harness.setScenario("design-served-text");
    await freezeDate(page);
    await patchServed(page, (served) => ({
      ...served,
      text: `<img src=x onerror=alert(1)>\n<script>alert(1)</script>\nname: invoice\u202Egnp.exe\nzero: a\u200Bb\njs: javascript:alert(1)\n`,
      masks: undefined,
      sources: [{ path: "<img src=x onerror=alert(1)>.yml", rank: 0 }],
      unresolved_placeholders: ["<img src=x onerror=alert(1)>", "a\u202Eb"],
    }));
    await page.goto(`${harness.url}${servedUrl()}`);
    await expect(page.getByRole("table", { name: "Served text of connection-pool.yml" })).toBeVisible();
    expect(await page.locator("main img, main script, main iframe").count(), "nothing of the text became an element").toBe(0);
    expect(await page.locator("main").innerText()).toContain("<img src=x onerror=alert(1)>");
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });
});

// ---- The loading skeleton, in both themes ---------------------------------------------------------------------------------------------------
// In the dark theme the card is the colour of `muted`, and a skeleton drawn in `muted` on a card is not there: only the status line shows.
for (const scheme of ["light", "dark"] as const) {
  test.describe(`the loading skeleton of As served, ${scheme}`, () => {
    test.use({ scenario: "design-served-text", colorScheme: scheme });

    test("shows: every block is drawn in a colour other than the surface it is on", async ({ page, harness }) => {
      await freezeDate(page);
      await page.route(API.served, () => new Promise(() => undefined));
      await page.goto(`${harness.url}${servedUrl()}`);
      await expect(page.getByRole("group", { name: "Loading the served file" })).toBeVisible();
      await settle(page);
      const blocks = await page.evaluate(() => {
        const parse = (s: string): number[] => (s.match(/[\d.]+/g) ?? []).map(Number);
        const behind = (el: Element): string => {
          for (let a: Element | null = el.parentElement; a !== null; a = a.parentElement) {
            const bg = getComputedStyle(a).backgroundColor;
            const p = parse(bg);
            if (p.length >= 3 && (p.length === 3 || (p[3] ?? 1) > 0.9)) return bg;
          }
          return "none";
        };
        return [...document.querySelectorAll("[data-slot='skeleton']")].map((el) => ({ own: getComputedStyle(el).backgroundColor, behind: behind(el) }));
      });
      expect(blocks.length, "the skeleton has blocks").toBeGreaterThan(5);
      expect(blocks.filter((b) => b.own === b.behind), "blocks that are the colour of their surface").toEqual([]);
    });
  });
}
