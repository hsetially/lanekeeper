// The accessibility rows of the T5 screens that are not "axe finds nothing" (that is a11y.spec.ts, over the cases of cases-t5-*.ts), read with
// the real keyboard and the real layout: the attribution chip (a control of at least 24 px that opens its evidence, and gives focus back),
// the one severity badge (the same look wherever it is drawn), the Pull requests page (a radio group, a grid, a held list), the job and
// the duplicate card in the Raise PR dialog, a masked value and its Reveal (the dialog, the revealed value, what is announced and when,
// one at a time), and the dialogs of the four admin sections (focus out and back, the words typed to save a policy that deletes).
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { settle } from "./axe.ts";
import { SIDEBAR, describedText, freezeDate, modal, slowScheduler } from "./helpers.ts";
import { CORE_FILE, frame, goOffline, itemsOf, jobIs, openReveal, patchJson, raisePr, reveal, sendReveal, withPrs } from "./t5.ts";
import { BY_SOURCE } from "../../src/mocks/design/attribution/data.ts";
import { REVEALED_VALUE } from "../../src/mocks/design/masking/data.ts";
import { LINKS } from "../../src/mocks/design/prs/fixtures.ts";

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

const insideDialog = (page: Page): Promise<boolean> =>
  page.evaluate(() => {
    const active = document.activeElement;
    return active !== null && [...document.querySelectorAll("[role='dialog'], [role='alertdialog']")].some((d) => d.contains(active));
  });

// ---- The attribution chip --------------------------------------------------------------------------------------------------------------
test.describe("the attribution chip", () => {
  test.use({ scenario: "design-attribution-history" });

  const panel = (page: Page) => page.getByRole("complementary", { name: "History" });
  const chips = (page: Page) => panel(page).locator("[data-slot='attribution'] button");

  test("is a control of at least 24 by 24 px with its confidence in the name, in the file header and in every history row", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await page.getByRole("button", { name: "History", exact: true }).click();
    await expect(chips(page).first()).toBeVisible();
    const all = await page.evaluate(() =>
      [...document.querySelectorAll("[data-slot='attribution'] button")].map((el) => {
        const r = el.getBoundingClientRect();
        return { name: (el.textContent ?? "").replace(/\s+/g, " ").trim(), width: r.width, height: r.height, where: el.closest("aside") ? "history" : "header", popup: el.getAttribute("aria-haspopup"), expanded: el.getAttribute("aria-expanded") };
      }),
    );
    expect(all.length, "the head of the file and nine versions").toBe(10);
    for (const chip of all) {
      expect(chip.height, `${chip.where} chip "${chip.name}" is ${chip.height}px high (WCAG 2.5.8 asks for 24)`).toBeGreaterThanOrEqual(24);
      expect(chip.width, `${chip.where} chip "${chip.name}" is ${chip.width}px wide`).toBeGreaterThanOrEqual(24);
      expect(chip.name, "the name carries how sure the server is, in a word").toMatch(/Confidence: (Certain|High|Medium|Low|Unknown)/);
      expect(chip.popup, "the control says it opens something").not.toBeNull();
      expect(chip.expanded).toBe("false");
    }
  });

  test("Enter on the chip opens its evidence with focus inside it, Escape closes it and gives focus back to the chip", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await page.getByRole("button", { name: "History", exact: true }).click();
    const chip = chips(page).filter({ hasText: /^dana\.okoye/ }).first();
    // A key has been pressed, so the focus that follows is the keyboard's (a script's focus after a mouse press is not :focus-visible).
    await page.keyboard.press("Shift");
    await chip.focus();
    // Focus is drawn: an outline or a ring, so a person who tabs to it can see where they are (WCAG 2.4.7).
    const ring = await chip.evaluate((el) => {
      const cs = getComputedStyle(el);
      return { outline: cs.outlineStyle !== "none" && parseFloat(cs.outlineWidth) > 0, shadow: cs.boxShadow !== "none" };
    });
    expect(ring.outline || ring.shadow, "the chip draws its focus").toBe(true);
    await page.keyboard.press("Enter");
    const evidence = page.getByRole("dialog", { name: "Attribution evidence" });
    await expect(evidence).toBeVisible();
    await expect(chip).toHaveAttribute("aria-expanded", "true");
    expect(await insideDialog(page), `focus is ${await focused(page)}, outside the evidence`).toBe(true);
    // The evidence is a definition list of what was recorded, and it says so in words.
    await expect(evidence.getByText("Login identity")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(evidence).toHaveCount(0);
    await expect(chip).toBeFocused();
    await expect(chip).toHaveAttribute("aria-expanded", "false");
    // The history panel is still there: Escape closed the evidence and nothing more.
    await expect(panel(page)).toBeVisible();
  });

  test("a press opens it, a press elsewhere closes it, and the chip has focus again", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await page.getByRole("button", { name: "History", exact: true }).click();
    const chip = chips(page).first();
    await chip.click();
    await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toBeVisible();
    await page.getByRole("heading", { level: 1 }).click();
    await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toHaveCount(0);
    // Focus is not left on the body: it is where it was before, or on what was pressed.
    expect(await focused(page)).not.toBe("(nothing)");
  });

  test("only one evidence is open at a time, and Tab from the evidence reaches the next control of the row", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await page.getByRole("button", { name: "History", exact: true }).click();
    await chips(page).nth(0).click();
    await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toHaveCount(1);
    await chips(page).nth(1).click();
    await expect(page.getByRole("dialog", { name: "Attribution evidence" })).toHaveCount(1);
  });

  test("what it says is words: the source, the person and the confidence are text, and the bars are hidden from assistive technology", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await page.getByRole("button", { name: "History", exact: true }).click();
    // The panel opens at once, with its loading state, and the versions come with the answer of the hub: read the rows when they are there.
    await expect(page.locator("aside [data-slot='attribution']")).toHaveCount(9);
    const rows = await page.evaluate(() =>
      [...document.querySelectorAll("aside [data-slot='attribution']")].map((el) => ({
        unknown: el.getAttribute("data-who") === "unknown",
        text: (el.textContent ?? "").replace(/\s+/g, " ").trim(),
        barsHidden: [...el.querySelectorAll("[data-slot='confidence'] [aria-hidden='true']")].length > 0,
        word: (el.querySelector("[data-slot='confidence']")?.textContent ?? "").replace(/\s+/g, " ").trim(),
      })),
    );
    expect(rows.length).toBe(9);
    for (const row of rows) {
      expect(row.barsHidden, `the bars of "${row.text}" are decoration`).toBe(true);
      expect(row.word, `"${row.text}" says its confidence in a word`).toMatch(/Confidence: (Certain|High|Medium|Low|Unknown)/);
    }
    // An author nobody identified is told from a person by a word and a question mark, and carries no score ("Low" would be one the server did not give).
    const unknown = rows.find((r) => r.unknown);
    expect(unknown?.text).toContain("Unknown author");
    expect(unknown?.word).toContain("Unknown");
  });
});

// ---- The one severity badge ----------------------------------------------------------------------------------------------------------------
for (const scheme of ["light", "dark"] as const) {
  test.describe(`the severity badge, ${scheme}`, () => {
    test.use({ colorScheme: scheme });

    type Badge = { severity: string; text: string; icon: string; color: string; background: string; border: string; weight: string; borderStyle: string; height: number };
    const read = (page: Page): Promise<Badge[]> =>
      page.evaluate(() =>
        [...document.querySelectorAll("[data-slot='severity-badge']")].map((el) => {
          const cs = getComputedStyle(el);
          return {
            severity: el.getAttribute("data-severity") ?? "",
            text: (el.textContent ?? "").replace(/\s+/g, " ").trim(),
            icon: el.querySelector("svg")?.getAttribute("class") ?? "(no icon)",
            color: cs.color,
            background: cs.backgroundColor,
            border: cs.borderTopColor,
            weight: cs.fontWeight,
            borderStyle: cs.borderTopStyle,
            height: Math.round(el.getBoundingClientRect().height),
          };
        }),
      );

    test("one badge for the Findings page, a swimlane's Findings and Drift tabs, the History of a file and the drawer of a finding", async ({ page, harness }) => {
      test.setTimeout(120_000);
      const surfaces: Record<string, Badge[]> = {};

      harness.setScenario("design-findings-all-kinds");
      await open(page, `${harness.url}/findings`);
      await expect(page.locator("[data-slot='severity-badge']").first()).toBeVisible();
      surfaces["the Findings page"] = await read(page);
      await page.getByRole("grid", { name: "Findings" }).getByRole("row").nth(1).click();
      await expect(page.getByRole("dialog")).toBeVisible();
      surfaces["the drawer of a finding"] = (await read(page)).slice(-1);

      harness.setScenario("design-swimlanes-six");
      await page.goto(`${harness.url}/swimlanes/sitb/findings`);
      await expect(page.getByRole("tabpanel").locator("[data-slot='severity-badge']").first()).toBeVisible();
      surfaces["the Findings tab"] = await read(page);

      await patchJson(page, /\/api\/v1\/swimlanes\/sitb\/drift(\?.*)?$/, (list) => ({
        ...list,
        items: itemsOf(list).map((file, i) => ({ ...file, attribution: BY_SOURCE.tool, severity: (["critical", "high", "medium", "low"] as const)[i % 4] })),
      }));
      await page.goto(`${harness.url}/swimlanes/sitb/drift`);
      await expect(page.getByRole("tabpanel").locator("[data-slot='severity-badge']").first()).toBeVisible();
      surfaces["the Drift tab"] = await read(page);

      harness.setScenario("design-attribution-history");
      await page.goto(`${harness.url}${CORE_FILE}`);
      await page.getByRole("button", { name: "History", exact: true }).click();
      await expect(page.locator("aside [data-slot='severity-badge']").first()).toBeVisible();
      surfaces["the History of a file"] = await read(page);

      // Each level looks the same wherever it is drawn: the same icon, word, colours, weight and border.
      const look = (b: Badge): string => JSON.stringify({ icon: b.icon, color: b.color, background: b.background, border: b.border, weight: b.weight, borderStyle: b.borderStyle, height: b.height });
      const byLevel = new Map<string, Map<string, string>>();
      for (const [where, badges] of Object.entries(surfaces)) {
        expect(badges.length, `${where} draws severity badges`).toBeGreaterThan(0);
        for (const b of badges) {
          const levels = byLevel.get(b.severity) ?? new Map<string, string>();
          levels.set(where, look(b));
          byLevel.set(b.severity, levels);
          expect(b.icon, `${where}: the ${b.severity} badge has an icon`).not.toBe("(no icon)");
          expect(b.text.toLowerCase(), `${where}: the ${b.severity} badge says the level in a word`).toContain(b.severity);
        }
      }
      for (const [level, places] of byLevel) {
        const looks = new Set(places.values());
        expect(looks.size, `the ${level} badge looks different in different places (${scheme}): ${JSON.stringify([...places.entries()])}`).toBe(1);
      }
      expect([...byLevel.keys()].sort(), "all four levels were looked at").toEqual(["critical", "high", "low", "medium"]);
      // The four levels differ in an icon and a word, so a person who cannot tell the colours apart can tell the levels apart.
      const signatures = new Set([...byLevel.keys()].map((level) => `${[...(surfaces["the Findings page"] ?? []), ...(surfaces["the Drift tab"] ?? [])].find((b) => b.severity === level)?.icon}|${level}`));
      expect(signatures.size).toBe(4);
      const icons = new Set([...byLevel.keys()].map((level) => [...(surfaces["the Findings page"] ?? []), ...(surfaces["the Drift tab"] ?? [])].find((b) => b.severity === level)?.icon));
      expect(icons.size, "each level has an icon of its own").toBe(4);
    });
  });
}

// ---- Pull requests -----------------------------------------------------------------------------------------------------------------------
test.describe("the Pull requests page", () => {
  test.use({ scenario: "design-prs-list" });
  const PAGE = "/swimlanes/sitb/pull-requests";

  test("the state filter is one radio group named State: arrows move through the states, Enter chooses one, the choice is in the address and keeps focus", async ({ page, harness }) => {
    await open(page, `${harness.url}${PAGE}`);
    const group = page.getByRole("radiogroup", { name: "State" });
    await expect(group).toBeVisible();
    await expect(group.getByRole("radio")).toHaveText(["Open", "Merged", "Closed", "All"]);
    await expect(group.getByRole("radio", { name: "Open" })).toBeChecked();
    // Tab arrives on the chosen one (the group is one stop, and a roving focus hands it to the radio that is checked).
    await page.getByRole("link", { name: "sitb", exact: true }).first().focus();
    for (let i = 0; i < 6 && (await focused(page)) !== 'button "Open"'; i++) await page.keyboard.press("Tab");
    expect(await focused(page), "Tab from the swimlane link reaches the filter on the checked state").toBe('button "Open"');
    await page.keyboard.press("Tab");
    expect(await focused(page), "and the next Tab leaves the group (it is one stop)").not.toMatch(/Merged|Closed|All/);
    await group.getByRole("radio", { name: "Open" }).focus();
    await page.keyboard.press("ArrowRight");
    await expect(group.getByRole("radio", { name: "Merged" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/state=merged/);
    await expect(group.getByRole("radio", { name: "Merged" })).toBeChecked();
    await expect(group.getByRole("radio", { name: "Merged" })).toBeFocused();
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Pull requests");
    await expect(page.getByText(/1 merged pull request/)).toBeVisible();
    // Back to the default: the address has no state.
    await group.getByRole("radio", { name: "Open" }).click();
    await expect(page).not.toHaveURL(/state=/);
  });

  test("the table is a named grid with its column headers, rows of 32 px, and a link for the pull request that says where it goes", async ({ page, harness }) => {
    await open(page, `${harness.url}${PAGE}?state=all`);
    const grid = page.getByRole("grid", { name: "Pull requests" });
    await expect(grid).toBeVisible();
    await expect(grid.getByRole("columnheader")).toHaveText(["State", "Pull request", "File", "Opened by", "Opened", "Merged or closed"]);
    const rows = await grid.evaluate((g) => [...g.querySelectorAll("[role='row']")].filter((r) => r.querySelector("[role='gridcell']") !== null).map((r) => Math.round(r.getBoundingClientRect().height)));
    expect(rows.length, "the five links of sitb (the sixth is another swimlane's)").toBe(5);
    expect(new Set(rows), "every row has the one fixed height").toEqual(new Set([32]));
    const links = await grid.locator("a[data-pr-link]").evaluateAll((as) => as.map((a) => ({ name: (a.textContent ?? "").trim(), target: a.getAttribute("target"), rel: a.getAttribute("rel"), href: a.getAttribute("href"), height: a.getBoundingClientRect().height })));
    expect(links.length).toBe(5);
    for (const link of links) {
      expect(link.name, "the link says it leaves the app").toMatch(/^#\d+, on GitHub$/);
      expect(link.target).toBe("_blank");
      expect(link.rel).toContain("noopener");
      expect(link.rel).toContain("noreferrer");
      expect(link.href).toMatch(/^https:\/\/github\.com\//);
    }
    // A state is an icon and a word, never a colour.
    const states = await grid.locator("[data-slot='pr-state']").evaluateAll((els) => els.map((el) => ({ text: (el.textContent ?? "").trim(), icon: el.querySelector("svg") !== null })));
    expect(states.every((s) => s.icon && /^(Open|Merged|Closed)$/.test(s.text))).toBe(true);
  });

  test("the keyboard cursor is on the grid: arrows move it and Enter follows the pull request of the row, on GitHub, in a new tab", async ({ page, harness }) => {
    await open(page, `${harness.url}${PAGE}`);
    const grid = page.getByRole("grid", { name: "Pull requests" });
    await grid.focus();
    await expect(grid).toBeFocused();
    await page.keyboard.press("ArrowDown");
    const active = await grid.getAttribute("aria-activedescendant");
    expect(active, "the cursor is a row of the grid").not.toBeNull();
    expect(await page.evaluate((id) => document.getElementById(id ?? "")?.getAttribute("role"), active)).toBe("row");
    const popup = page.waitForEvent("popup", { timeout: 5000 }).catch(() => null);
    await page.keyboard.press("Enter");
    const opened = await popup;
    expect(opened, "Enter opened the pull request").not.toBeNull();
    expect(opened?.url() ?? "").toMatch(/^https:\/\/github\.com\//);
    await opened?.close();
  });

  test("a list that is held while it is read says so in a polite status, and Show brings the new row in", async ({ page, harness }) => {
    await open(page, `${harness.url}${PAGE}`);
    await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
    const arrived = { ...LINKS[0], id: 99, pr_number: 430, pr_url: "https://github.com/csp-tenant/csp-tenant-data/pull/430", path: "tx-infinity-api/tx-infinity-payments-sit12.yml" };
    await withPrs(page, [arrived as (typeof LINKS)[number], ...LINKS]);
    harness.emit(`id: 900\nevent: pr_state_changed\ndata: ${JSON.stringify({ type: "pr_state_changed", link_id: 99, state: "open" })}\n\n`);
    const status = page.getByRole("status").filter({ hasText: /1 update/ });
    await expect(status).toBeVisible();
    expect(await status.getAttribute("aria-live"), "a status is polite").not.toBe("assertive");
    await expect(page.getByRole("grid", { name: "Pull requests" }).getByRole("row")).toHaveCount(4);
    await status.getByRole("button", { name: "Show" }).click();
    await expect(page.getByRole("grid", { name: "Pull requests" }).getByRole("row")).toHaveCount(5);
  });

  test("while the list loads the region is busy and the skeleton is a named group; when it arrives nothing is busy", async ({ page, harness }) => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route(/\/api\/v1\/prs(\?.*)?$/, async (route) => {
      await gate;
      await route.continue().catch(() => undefined);
    });
    await freezeDate(page);
    await page.goto(`${harness.url}${PAGE}`);
    await expect(page.getByRole("group", { name: "Loading pull requests" })).toBeVisible();
    await expect(page.locator("[aria-busy='true']").first()).toBeVisible();
    release();
    await expect(page.getByRole("grid", { name: "Pull requests" })).toBeVisible();
    await expect(page.locator("[aria-busy='true']")).toHaveCount(0);
  });

  test("a failed list is an alert", async ({ page, harness }) => {
    await page.route(/\/api\/v1\/prs(\?.*)?$/, (route) => route.fulfill({ status: 500, contentType: "application/problem+json", headers: { "x-request-id": "req-t5-0001" }, body: JSON.stringify({ type: "urn:lanekeeper:problem:internal", title: "Boom", status: 500, request_id: "req-t5-0001" }) }));
    await open(page, `${harness.url}${PAGE}`);
    await expect(page.getByRole("alert")).toContainText("req-t5-0001");
  });
});

test.describe("the pull requests the page must not trust", () => {
  test.use({ scenario: "design-prs-hostile" });

  test("an address that is not https with a host and no credentials is text, not a link", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes/sitb/pull-requests`);
    const grid = page.getByRole("grid", { name: "Pull requests" });
    await expect(grid).toBeVisible();
    // All four links are hostile: javascript:, credentials, http, and a very long path. None of the first three is a link.
    const hrefs = await grid.locator("a[data-pr-link]").evaluateAll((as) => as.map((a) => a.getAttribute("href")));
    expect(hrefs, "no hostile address became a link").toEqual([expect.stringMatching(/^https:\/\//)]);
    expect(await grid.locator("a[href^='javascript:']").count()).toBe(0);
    expect(await page.locator("a[href*='evil.example']").count()).toBe(0);
  });
});

// ---- The Raise PR dialog: the job and the duplicate card --------------------------------------------------------------------------------------
test.describe("the Raise PR dialog", () => {
  test.use({ scenario: "design-file-pr" });

  test("Escape closes it and gives focus back to Raise PR, and a running job is a polite list of steps with a word for each", async ({ page, harness }) => {
    await jobIs(page, "committing");
    await open(page, `${harness.url}${CORE_FILE}`);
    const raise = page.getByRole("button", { name: "Raise PR", exact: true });
    await raisePr(page, 3);
    const steps = modal(page).getByRole("list").filter({ hasText: "Preparing" });
    await expect(steps).toBeVisible();
    // (The dialog starts from the job the server accepted, and the state the server then reports replaces it.)
    await expect(steps.locator("li[data-status='active']")).toHaveText("Committing");
    expect(await steps.getAttribute("aria-live")).toBe("polite");
    const states = await steps.locator("li").evaluateAll((lis) => lis.map((li) => ({ text: (li.textContent ?? "").trim(), status: li.getAttribute("data-status"), label: li.querySelector("svg")?.getAttribute("aria-label") })));
    expect(states).toEqual([
      { text: "Preparing", status: "done", label: "Done" },
      { text: "Committing", status: "active", label: "Working" },
      { text: "Opening pull request", status: "waiting", label: "Waiting" },
      { text: "Done", status: "waiting", label: "Waiting" },
    ]);
    await page.keyboard.press("Escape");
    await expect(modal(page)).toHaveCount(0);
    await expect(raise).toBeFocused();
  });

  test("a job that an event moves on changes in place: the step moves, and 'created' is said only when the server reports it completed", async ({ page, harness }) => {
    let state = "preparing";
    await patchJson(page, /\/api\/v1\/pr-jobs\/\d+$/, (job) => ({ ...job, state, pr_url: state === "completed" ? "https://github.com/csp-tenant/csp-tenant-data/pull/419" : null }));
    await open(page, `${harness.url}${CORE_FILE}`);
    await raisePr(page, 3);
    const steps = modal(page).getByRole("list").filter({ hasText: "Preparing" });
    await expect(steps.locator("li[data-status='active']")).toHaveText("Preparing");
    await expect(modal(page).getByText(/created/i)).toHaveCount(0);
    await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
    state = "opening";
    harness.emit(frame(950, { type: "pr_job_progress", job_id: 7, state: "opening" }));
    await expect(steps.locator("li[data-status='active']")).toHaveText("Opening pull request");
    await expect(modal(page).getByText(/created/i)).toHaveCount(0);
    state = "completed";
    harness.emit(frame(951, { type: "pr_job_progress", job_id: 7, state: "completed" }));
    await expect(modal(page).getByText("PR #419 created.")).toBeVisible();
    await expect(steps).toHaveCount(0);
  });

  test("with live updates off the dialog says it may be behind, and Check now reads the job", async ({ page, harness }) => {
    await jobIs(page, "preparing");
    await open(page, `${harness.url}${CORE_FILE}`);
    await raisePr(page, 3);
    await expect(modal(page).getByText("Creating your pull request.")).toBeVisible();
    await goOffline({ page, harness });
    await expect(modal(page).getByText(/Live updates are paused/)).toBeVisible();
    const before = harness.requests().filter((r) => r.path.includes("/pr-jobs/")).length;
    await modal(page).getByRole("button", { name: "Check now" }).click();
    await expect.poll(() => harness.requests().filter((r) => r.path.includes("/pr-jobs/")).length).toBeGreaterThan(before);
  });
});

test.describe("the pull request that exists", () => {
  test.use({ scenario: "design-file-pr-exists" });

  test("the dialog names it, offers no second one, links to the list and opens the pull request in a new tab", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await raisePr(page, 1);
    await page.getByRole("button", { name: "Continue" }).click();
    const dialog = modal(page);
    await expect(dialog.getByText(/PR #418 is already open for this change/)).toBeVisible();
    await expect(dialog.getByRole("button", { name: /Create PR/ })).toHaveCount(0);
    const all = dialog.getByRole("link", { name: "All pull requests for sitb" });
    await expect(all).toHaveAttribute("href", "/swimlanes/sitb/pull-requests");
    const open_ = dialog.getByRole("link", { name: "Open PR #418" });
    await expect(open_).toHaveAttribute("target", "_blank");
    await expect(open_).toHaveAttribute("rel", /noopener/);
  });

  test("the header badge is a link that says it leaves the app, and its colour is not the only thing that says 'open'", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    const badge = page.locator("[data-slot='pr-badge']").first();
    await expect(badge).toBeVisible();
    await expect(badge).toHaveAttribute("href", /^https:\/\/github\.com\//);
    expect((await badge.textContent())?.replace(/\s+/g, " ").trim()).toContain("PR #418 open");
    expect(await badge.locator("svg").count(), "an icon as well as the words").toBeGreaterThan(0);
    expect(await badge.evaluate((el) => el.getBoundingClientRect().height), "the badge is not a target of its own that is too small to press").toBeGreaterThan(0);
  });
});

// ---- A masked value and its Reveal ---------------------------------------------------------------------------------------------------------------
test.describe("a masked value", () => {
  test.use({ scenario: "design-masking-reveal" });

  const dialog = (page: Page) => page.getByRole("alertdialog", { name: "Reveal masked value?" });
  const revealButton = (page: Page) => page.getByRole("button", { name: /^Reveal value of / }).first();
  const live = (page: Page) => page.locator("p[aria-live='polite'].sr-only");

  test("is a token with an icon and the words 'Hidden value', holds nothing of the value, and is never colour alone", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    const token = page.locator("[data-slot='hidden-value']").first();
    await expect(token).toBeVisible();
    expect((await token.textContent())?.replace(/\s+/g, " ").trim()).toMatch(/^Hidden value.*\d+ chars$/);
    expect(await token.locator("svg").count()).toBe(1);
    expect(await token.locator("svg").getAttribute("aria-hidden")).toBe("true");
    // Nothing of the value in the page: not in its text, not in any attribute.
    const html = await page.evaluate(() => document.documentElement.outerHTML);
    expect(html).not.toContain(REVEALED_VALUE);
    // The control that reveals it is named for the setting it reveals.
    await expect(revealButton(page)).toHaveAccessibleName(/^Reveal value of txInfinityOptions\.remote-itm-teller\.clientSecret$/);
  });

  test("the dialog is modal and labelled, starts in the reason field, traps Tab, and Escape gives focus back to Reveal", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    const button = revealButton(page);
    await button.focus();
    await page.keyboard.press("Enter");
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page)).toHaveAccessibleDescription(/recorded in the audit log/);
    await expect(dialog(page).getByRole("textbox", { name: /Reason/ })).toBeFocused();
    expect(await dialog(page).getByRole("textbox", { name: /Reason/ }).getAttribute("aria-required")).toBe("true");
    for (const key of ["Tab", "Shift+Tab"]) {
      for (let i = 0; i < 8; i++) {
        await page.keyboard.press(key);
        expect(await insideDialog(page), `${key} #${i + 1}: focus is ${await focused(page)}`).toBe(true);
      }
    }
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toHaveCount(0);
    await expect(button).toBeFocused();
    // Cancel does the same.
    await button.click();
    await dialog(page).getByRole("button", { name: "Cancel" }).click();
    await expect(button).toBeFocused();
  });

  test("no reason: the field is invalid, its error is attached to it, nothing is sent", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await openReveal(page);
    const before = harness.requests().filter((r) => r.path.endsWith("/files/reveal")).length;
    await dialog(page).getByRole("button", { name: "Reveal value", exact: true }).click();
    const reason = dialog(page).getByRole("textbox", { name: /Reason/ });
    await expect(reason).toHaveAttribute("aria-invalid", "true");
    await expect(dialog(page).getByText("Enter a reason")).toBeVisible();
    await expect.poll(() => describedText(reason), { message: "the error is part of what the field is described by" }).toContain("Enter a reason");
    expect(harness.requests().filter((r) => r.path.endsWith("/files/reveal")).length).toBe(before);
  });

  // The read above waits for the page; this one does not, and the page is made slow. The error is on the page and the field is invalid
  // in one commit, and the field names the error in that same commit, not in a render that the scheduler gets to a task later (which
  // is what a slow machine, a busy tab or a screen reader asking in between would find: a field that is invalid and described by nothing).
  test("no reason: the error is tied to the field in the commit that shows it, not a task later", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await openReveal(page);
    await slowScheduler(page);
    await dialog(page).getByRole("button", { name: "Reveal value", exact: true }).click();
    const reason = dialog(page).getByRole("textbox", { name: /Reason/ });
    await expect(reason).toHaveAttribute("aria-invalid", "true");
    await expect(dialog(page).getByText("Enter a reason")).toBeVisible();
    expect(await describedText(reason), "the error is part of what the field is described by, as soon as it is shown").toContain("Enter a reason");
  });

  test("revealed: Hide has focus, the announcement is polite and never holds the value, and Hide gives focus back to Reveal", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    const button = revealButton(page);
    await reveal(page);
    const hide = page.getByRole("button", { name: /^Hide value of / });
    await expect(hide).toBeFocused();
    await expect(page.locator("[data-slot='revealed-value']")).toHaveText(REVEALED_VALUE);
    await expect(live(page)).toContainText("Value revealed");
    expect(await live(page).textContent()).not.toContain(REVEALED_VALUE);
    // The value is text in the one element that shows it, and nowhere else: no attribute of any element holds it (a title, a label, a
    // data attribute), nor does any text outside that element (the live region, a tooltip).
    const copies = await page.evaluate(
      (secret) => ({
        attributes: [...document.querySelectorAll("*")].filter((el) => [...el.attributes].some((a) => a.value.includes(secret))).length,
        elsewhere: [...document.querySelectorAll("*")].filter((el) => !el.closest("[data-slot='revealed-value']") && [...el.childNodes].some((n) => n.nodeType === 3 && (n.nodeValue ?? "").includes(secret))).length,
      }),
      REVEALED_VALUE,
    );
    expect(copies).toEqual({ attributes: 0, elsewhere: 0 });
    await page.keyboard.press("Enter");
    await expect(page.locator("[data-slot='revealed-value']")).toHaveCount(0);
    await expect(live(page)).toHaveText("Value hidden.");
    await expect(button).toBeFocused();
  });

  test("leaving the window hides it and says so", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await reveal(page);
    await page.evaluate(() => window.dispatchEvent(new Event("blur")));
    await expect(page.locator("[data-slot='revealed-value']")).toHaveCount(0);
    await expect(live(page)).toHaveText("Value hidden.");
  });

  test("after 30 seconds it hides itself and says so", async ({ page, harness }) => {
    test.setTimeout(90_000);
    await open(page, `${harness.url}${CORE_FILE}`);
    await reveal(page);
    await expect(page.locator("[data-slot='revealed-value']")).toBeVisible();
    await expect(page.locator("[data-slot='revealed-value']")).toHaveCount(0, { timeout: 40_000 });
    await expect(live(page)).toHaveText("Value hidden.");
  });
});

test.describe("masked values, three at once", () => {
  test.use({ scenario: "design-masking-multi" });

  test("only one value is shown at a time: revealing another hides the first", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    await expect(page.locator("[data-slot='hidden-value']")).toHaveCount(3);
    // Only the mask that names its setting has a Reveal.
    await expect(page.getByRole("button", { name: /^Reveal value of / })).toHaveCount(1);
    await reveal(page);
    await expect(page.locator("[data-slot='revealed-value']")).toHaveCount(1);
    await expect(page.locator("[data-slot='hidden-value']")).toHaveCount(2);
    await page.getByRole("button", { name: /^Hide value of / }).click();
    await expect(page.locator("[data-slot='hidden-value']")).toHaveCount(3);
  });
});

test.describe("masked values for a person who may not reveal them", () => {
  test.use({ scenario: "design-masking-viewer" });

  test("Reveal is there, says it is off, can be focused, sends nothing, and says which role it needs", async ({ page, harness }) => {
    await open(page, `${harness.url}${CORE_FILE}`);
    const button = page.getByRole("button", { name: /^Reveal value of / });
    await expect(button).toBeVisible();
    await expect(button).toHaveAttribute("aria-disabled", "true");
    await button.focus();
    await expect(button).toBeFocused();
    await expect(button).toHaveAccessibleDescription(/Editor/);
    await page.keyboard.press("Enter");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(harness.requests().filter((r) => r.path.endsWith("/files/reveal")).length).toBe(0);
  });
});

test.describe("the hub's masks cannot be trusted", () => {
  test.use({ scenario: "design-masking-reveal" });
  const CONTENT = /\/api\/v1\/swimlanes\/sitb\/files\/content/;

  test("a line whose mask is not a marker is hidden and says so to a screen reader; none of it is in the page", async ({ page, harness }) => {
    await patchJson(page, CONTENT, (content, url) => (url.searchParams.get("source") === "nfs" && Array.isArray(content["masks"]) ? { ...content, masks: (content["masks"] as Array<Record<string, number>>).map((m) => ({ ...m, start: (m["start"] ?? 0) + 1 })) } : content));
    await open(page, `${harness.url}${CORE_FILE}`);
    const line = page.locator("[data-slot='hidden-line']").first();
    await expect(line).toBeVisible();
    await expect(line).toContainText("Hidden line");
    await expect(line.locator(".sr-only")).toContainText("could not be checked");
    expect(await page.locator("[data-slot='hidden-value']").count()).toBe(0);
    expect(await page.evaluate(() => document.body.innerText)).not.toContain("clientSecret: [REDACTED");
  });

  test("a mask that names a line the text does not have refuses the whole text, in words", async ({ page, harness }) => {
    await patchJson(page, CONTENT, (content, url) => (url.searchParams.get("source") === "nfs" && Array.isArray(content["masks"]) ? { ...content, masks: (content["masks"] as Array<Record<string, number>>).map((m) => ({ ...m, line: 9_999 })) } : content));
    await open(page, `${harness.url}${CORE_FILE}`);
    await expect(page.getByText("This text can't be shown safely")).toBeVisible();
    expect(await page.locator("[role='table'] [role='row']").count(), "no line of the text is drawn").toBe(0);
  });
});

// ---- The Reveal answers are in the dialog ----------------------------------------------------------------------------------------------------------
for (const [scenario, what] of [
  ["design-masking-throttled", "429 over the limit"],
  ["design-masking-unavailable", "503 the audit log is down"],
  ["design-masking-gone", "404 nothing to reveal"],
] as const) {
  test.describe(`a refused reveal, ${what}`, () => {
    test.use({ scenario });

    test("stays in the dialog as an alert, keeps the reason, shows no value, and sends exactly one request", async ({ page, harness }) => {
      await open(page, `${harness.url}${CORE_FILE}`);
      await openReveal(page);
      await sendReveal(page);
      const d = page.getByRole("alertdialog", { name: "Reveal masked value?" });
      await expect(d.getByRole("alert")).toBeVisible();
      await expect(d.getByRole("textbox", { name: /Reason/ })).not.toHaveValue("");
      await expect(page.locator("[data-slot='revealed-value']")).toHaveCount(0);
      expect(harness.requests().filter((r) => r.path.endsWith("/files/reveal")).length, "nothing sends a second request by itself").toBe(1);
      // The dialog can be left with the keyboard, and focus goes back to Reveal.
      await page.keyboard.press("Escape");
      await expect(d).toHaveCount(0);
      await expect(page.getByRole("button", { name: /^Reveal value of / }).first()).toBeFocused();
    });
  });
}

// ---- The Admin sections ---------------------------------------------------------------------------------------------------------------------------
test.describe("the dialogs of the admin sections", () => {
  test("OS Login: a dialog gives focus back to the button that opened it, from Escape and from Cancel, for Add, Edit and Delete", async ({ page, harness }) => {
    harness.setScenario("design-admin-t5-oslogin");
    await open(page, `${harness.url}/admin/oslogin`);
    for (const name of ["Add mapping", "Edit mapping marco_diaz_contractor_example", "Delete mapping lena_b_corp_example"]) {
      const trigger = page.getByRole("button", { name, exact: true });
      await trigger.focus();
      await page.keyboard.press("Enter");
      await expect(page.getByRole("alertdialog")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("alertdialog")).toHaveCount(0);
      await expect(trigger, `${name}: focus after Escape`).toBeFocused();
      await trigger.click();
      await page.getByRole("alertdialog").getByRole("button", { name: "Cancel" }).click();
      await expect(trigger, `${name}: focus after Cancel`).toBeFocused();
    }
  });

  test("OS Login: the fields of Add are labelled, the first has focus, an error is attached to its field and the dialog is not left", async ({ page, harness }) => {
    harness.setScenario("design-admin-t5-oslogin");
    await open(page, `${harness.url}/admin/oslogin`);
    await page.getByRole("button", { name: "Add mapping" }).click();
    const d = page.getByRole("alertdialog");
    await expect(d.getByRole("textbox", { name: "OS Login username" })).toBeFocused();
    await d.getByRole("button", { name: "Save mapping" }).click();
    await expect(d).toBeVisible();
    await expect(d.getByRole("combobox", { name: "Lanekeeper user" })).toHaveAttribute("aria-invalid", "true");
    await expect(d.getByRole("combobox", { name: "Lanekeeper user" })).toHaveAccessibleDescription(/Choose the user/);
  });

  test("Severity rules: 'Not applied yet' is a word in the table and in the dialog, not a colour", async ({ page, harness }) => {
    harness.setScenario("design-admin-t5-severity");
    await open(page, `${harness.url}/admin/severity`);
    const row = page.getByRole("grid", { name: "Severity rules" }).getByRole("row", { name: /removed-settings/ });
    await expect(row).toContainText("Not applied yet");
    await expect(row).toContainText("On");
    // The rule that is switched off says so in a word as well.
    await expect(page.getByRole("grid", { name: "Severity rules" }).getByRole("row", { name: /payments-high/ })).toContainText("Off");
    await row.getByRole("button", { name: /^Edit rule/ }).click();
    await expect(page.getByRole("alertdialog").getByText("Not applied yet")).toBeVisible();
  });

  test("Severity rules: the dialog's checkboxes are in two labelled groups and the matches summary is a named status", async ({ page, harness }) => {
    harness.setScenario("design-admin-t5-severity");
    await open(page, `${harness.url}/admin/severity`);
    await page.getByRole("button", { name: "Add rule" }).click();
    const d = page.getByRole("alertdialog");
    await expect(d.getByRole("group", { name: "Finding types" })).toBeVisible();
    await expect(d.getByRole("group", { name: "Change types" })).toBeVisible();
    await expect(d.getByRole("status", { name: "What this rule matches" })).toContainText("This rule matches every change and every finding");
    await d.getByRole("textbox", { name: "Path glob" }).fill("security/**");
    await expect(d.getByRole("status", { name: "What this rule matches" })).toContainText("security/**");
  });

  test("Sentinels: a disconnected sentinel is a word and an icon, the alert names the VM, and the table says nothing of a heartbeat", async ({ page, harness }) => {
    harness.setScenario("design-admin-t5-sentinels");
    await open(page, `${harness.url}/admin/sentinels`);
    const row = page.getByRole("grid", { name: "Sentinels" }).getByRole("row", { name: /nfs-presitb-01/ });
    await expect(row).toContainText("Disconnected");
    expect(await row.locator("[data-slot='sentinel-status'] svg").count()).toBe(1);
    await expect(page.getByRole("alert").filter({ hasText: "nfs-presitb-01 is disconnected" })).toBeVisible();
    expect(await page.locator("main").innerText()).not.toMatch(/heartbeat/i);
  });

  test("Retention: the policy that deletes is saved only by typing the words, Enter in the field does not save, and the button says why it is off", async ({ page, harness }) => {
    harness.setScenario("design-admin-t5-retention");
    await open(page, `${harness.url}/admin/retention`);
    // The API does not report the job's own dry-run switch (a recorded residual risk, D118), so the page says to read every policy as deleting.
    await expect(page.getByText(/Read every policy here as one that deletes/)).toBeVisible();
    await expect(page.getByText("Retention permanently deletes stored versions")).toBeVisible();
    const save = page.getByRole("button", { name: "Save policy" });
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await expect(save).toHaveAccessibleDescription(/Nothing has changed/);
    await page.getByRole("textbox", { name: "Days to keep versions" }).fill("90");
    await expect(save).toHaveAccessibleDescription(/dry run/i);
    await page.getByRole("button", { name: "Run dry run" }).click();
    await expect(page.getByText(/^This policy:/)).toBeVisible();
    await save.click();
    const d = page.getByRole("alertdialog", { name: /Save a policy that deletes versions/ });
    await expect(d).toBeVisible();
    const confirm = d.getByRole("button", { name: "Save and delete" });
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    const words = d.getByRole("textbox");
    await words.fill("delete versions");
    await expect(confirm).not.toHaveAttribute("aria-disabled", "true");
    await words.fill("delete");
    await words.press("Enter");
    const puts = harness.requests().filter((r) => r.method === "PUT" && r.path.includes("/admin/retention") && !r.path.includes("dry_run"));
    expect(puts.length, "Enter in the field with the wrong words saved nothing").toBe(0);
    await page.keyboard.press("Escape");
    await expect(d).toHaveCount(0);
    await expect(save).toBeFocused();
  });
});

test.describe("the sidebar and the landmarks still hold on the T5 screens", () => {
  for (const [id, scenario, url] of [
    ["the Pull requests page", "design-prs-list", "/swimlanes/sitb/pull-requests"],
    ["a masked file", "design-masking-reveal", CORE_FILE],
    ["the Sentinels section", "design-admin-t5-sentinels", "/admin/sentinels"],
    ["the Audit log with attribution", "design-attribution-audit", "/audit"],
  ] as const) {
    test(`${id}: one banner, one main, the primary sidebar, one h1 and a place for notifications`, async ({ page, harness }) => {
      harness.setScenario(scenario);
      await open(page, `${harness.url}${url}`);
      // The landmarks as the accessibility tree has them (a <header> inside <main> is not a banner).
      await expect(page.getByRole("banner")).toHaveCount(1);
      await expect(page.getByRole("main")).toHaveCount(1);
      await expect(page.getByRole("complementary", { name: "Primary" })).toHaveCount(1);
      await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
      await expect(page.getByRole("region", { name: "Notifications" })).toHaveCount(1);
      // The counts in the sidebar are named for a screen reader (the number and the word are one name).
      await expect(SIDEBAR(page).getByRole("navigation", { name: "Sections" }).getByRole("link", { name: "Findings 11 open", exact: true })).toBeVisible();
    });
  }
});

// ---- Forced colours (high contrast) on the T5 screens ------------------------------------------------------------------------------------------
test.describe("forced colours: the T5 badges and tokens say what they say without their colours", () => {
  const SCREENS = [
    ["the History of a file (chips, confidence, severity)", "design-attribution-history", CORE_FILE, async (page: Page) => { await page.getByRole("button", { name: "History", exact: true }).click(); }],
    ["the Pull requests page", "design-prs-list", "/swimlanes/sitb/pull-requests?state=all", () => undefined],
    ["the Drift tab with PR badges", "design-prs-drift-badges", "/swimlanes/sitb/drift", () => undefined],
    ["a masked file", "design-masking-reveal", CORE_FILE, () => undefined],
    ["the Sentinels section", "design-admin-t5-sentinels", "/admin/sentinels", () => undefined],
    ["the Severity rules section", "design-admin-t5-severity", "/admin/severity", () => undefined],
    ["the Audit log with attribution", "design-attribution-audit", "/audit", () => undefined],
  ] as const;

  for (const [id, scenario, url, prepare] of SCREENS) {
    test(id, async ({ page, harness }) => {
      harness.setScenario(scenario);
      await page.emulateMedia({ forcedColors: "active" });
      await open(page, `${harness.url}${url}`);
      await prepare(page);
      await settle(page, 300);
      expect(await page.evaluate(() => matchMedia("(forced-colors: active)").matches)).toBe(true);
      const bad = await page.evaluate(() => {
        const found: string[] = [];
        const label = (el: Element): string => `${el.getAttribute("data-slot")} "${(el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 30)}"`;
        // A state is a word, and (for the ones that mean a state of something) an icon: never a colour alone.
        for (const el of document.querySelectorAll("[data-slot='severity-badge'], [data-slot='pr-state'], [data-slot='pr-badge'], [data-slot='sentinel-status']")) {
          if ((el.textContent ?? "").replace(/\s+/g, "") === "") found.push(`no word: ${label(el)}`);
          if (el.querySelector("svg") === null) found.push(`no icon: ${label(el)}`);
          const cs = getComputedStyle(el);
          if (cs.color === cs.backgroundColor && cs.backgroundColor !== "rgba(0, 0, 0, 0)") found.push(`same colour as its background: ${label(el)}`);
        }
        // The confidence is a word; the bars are decoration.
        for (const el of document.querySelectorAll("[data-slot='confidence']")) {
          if (!/Confidence: \w+/.test((el.textContent ?? "").replace(/\s+/g, " "))) found.push(`no confidence word: ${label(el)}`);
        }
        // A hidden value keeps its outline, so it is a thing and not a gap in the line.
        for (const el of document.querySelectorAll("[data-slot='hidden-value'], [data-slot='hidden-line']")) {
          const cs = getComputedStyle(el);
          if (cs.borderTopStyle === "none" || parseFloat(cs.borderTopWidth) === 0) found.push(`no outline: ${label(el)}`);
          if ((el.textContent ?? "").trim() === "") found.push(`no word: ${label(el)}`);
        }
        return found;
      });
      expect(bad).toEqual([]);
    });
  }
});

// ---- Zoom to 200% at 1280 wide on the T5 screens ----------------------------------------------------------------------------------------------------
test.describe("zoom to 200% at 1280 wide (a 640 by 450 window) on the T5 screens", () => {
  test.use({ viewport: { width: 640, height: 450 } });

  const noSidewaysScroll = (page: Page) => page.evaluate(() => ({ doc: document.documentElement.scrollWidth - document.documentElement.clientWidth, main: (document.querySelector("main")?.scrollWidth ?? 0) - (document.querySelector("main")?.clientWidth ?? 0) }));

  for (const [id, scenario, url] of [
    ["the Pull requests page", "design-prs-list", "/swimlanes/sitb/pull-requests?state=all"],
    ["a masked file", "design-masking-reveal", CORE_FILE],
    ["Retention", "design-admin-t5-retention", "/admin/retention"],
    ["Sentinels", "design-admin-t5-sentinels", "/admin/sentinels"],
    ["Severity rules", "design-admin-t5-severity", "/admin/severity"],
  ] as const) {
    test(`${id}: the landmarks and the one h1 are there and the page does not scroll sideways`, async ({ page, harness }) => {
      harness.setScenario(scenario);
      await open(page, `${harness.url}${url}`);
      await expect(page.getByRole("main")).toHaveCount(1);
      await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
      const scroll = await noSidewaysScroll(page);
      expect(scroll.doc).toBeLessThanOrEqual(1);
      expect(scroll.main).toBeLessThanOrEqual(1);
    });
  }

  for (const [id, scenario, url, press, button] of [
    ["the Reveal dialog", "design-masking-reveal", `${CORE_FILE}&line=19`, /^Reveal value of /, "Reveal value"],
    ["the severity rule dialog", "design-admin-t5-severity", "/admin/severity", "Add rule", "Save rule"],
    ["the OS Login dialog", "design-admin-t5-oslogin", "/admin/oslogin", "Add mapping", "Save mapping"],
  ] as const) {
    test(`${id}: fits the window or scrolls inside itself, and its buttons can be reached`, async ({ page, harness }) => {
      harness.setScenario(scenario);
      await open(page, `${harness.url}${url}`);
      await page.getByRole("button", { name: press }).first().click();
      const d = page.getByRole("alertdialog");
      await expect(d).toBeVisible();
      await settle(page, 300);
      const fit = await d.evaluate((el) => {
        const r = el.getBoundingClientRect();
        // Taller than the window: the overlay that holds the dialog (or the dialog) scrolls, so every part of it can be brought into view.
        let scrolls = false;
        for (let a: Element | null = el; a !== null && !scrolls; a = a.parentElement) scrolls = ["auto", "scroll"].includes(getComputedStyle(a).overflowY) && a.scrollHeight > a.clientHeight;
        return { top: r.top, bottom: r.bottom, left: r.left, right: r.right, vw: window.innerWidth, vh: window.innerHeight, scrolls };
      });
      expect(fit.left, "the dialog starts inside the window").toBeGreaterThanOrEqual(-1);
      expect(fit.right, "the dialog ends inside the window").toBeLessThanOrEqual(fit.vw + 1);
      expect(fit.top >= -1 && fit.bottom <= fit.vh + 1 || fit.scrolls, `the dialog is ${Math.round(fit.top)}..${Math.round(fit.bottom)} in a window ${fit.vh} high and does not scroll`).toBe(true);
      // The action can be pressed with the keyboard from inside the dialog, and it can be scrolled into view.
      const action = d.getByRole("button", { name: button, exact: true });
      await action.scrollIntoViewIfNeeded();
      const box = await action.boundingBox();
      expect(box, "the action is on the screen").not.toBeNull();
      expect((box?.y ?? -1) >= 0 && (box?.y ?? 0) + (box?.height ?? 0) <= fit.vh + 1).toBe(true);
    });
  }
});

// ---- prefers-reduced-motion on the T5 screens -------------------------------------------------------------------------------------------------
test.describe("prefers-reduced-motion on the T5 screens", () => {
  /** What moves: every running keyframe animation, and every transition of a property that moves something (a colour that fades does not). */
  const motion = (page: Page) =>
    page.evaluate(() =>
      document
        .getAnimations()
        .filter((a) => a.playState === "running")
        .map((a) => {
          const timing = a.effect?.getComputedTiming();
          const kind = a instanceof CSSTransition ? "transition" : "animation";
          const name = a instanceof CSSAnimation ? a.animationName : a instanceof CSSTransition ? a.transitionProperty : "";
          return { kind, name, duration: Number(timing?.duration ?? 0) };
        })
        .filter((m) => m.kind === "animation" || /^(transform|translate|rotate|scale|top|left|right|bottom|inset|margin|width|height)/.test(m.name)),
    );

  test("a running pull request job turns its spinner slowly and moves nothing else", async ({ page, harness }) => {
    harness.setScenario("design-file-pr");
    await page.emulateMedia({ reducedMotion: "reduce" });
    await jobIs(page, "committing");
    await open(page, `${harness.url}${CORE_FILE}`);
    await raisePr(page, 3);
    await expect(modal(page).getByText("Creating your pull request.")).toBeVisible();
    const moving = await motion(page);
    expect(moving.filter((m) => m.name !== "lk-spin"), JSON.stringify(moving)).toEqual([]);
    expect(moving.length, "the spinner is on the page").toBeGreaterThan(0);
    expect(moving.every((m) => m.duration >= 2400), JSON.stringify(moving)).toBe(true);
  });

  test("the Reveal dialog and the revealed value appear without animation", async ({ page, harness }) => {
    harness.setScenario("design-masking-reveal");
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page, `${harness.url}${CORE_FILE}`);
    await openReveal(page);
    await sendReveal(page);
    await expect(page.locator("[data-slot='revealed-value']")).toBeVisible();
    await settle(page, 400);
    expect(await motion(page), "nothing moves while a value is shown").toEqual([]);
  });
});
