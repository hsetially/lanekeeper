// The accessibility rows of the plan that are not "axe finds nothing" (plan, "Keyboard, accessibility, time and browsers"):
//   one h1 and the landmarks; the names of the sidebar's counts; where focus goes after navigation and after a dialog; live regions and
//   their politeness; prefers-reduced-motion; forced-colors; a focus indicator on every control that takes focus; zoom to 200%.
// axe on every screen and state is a11y.spec.ts.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { FROZEN_NOW } from "../visual/render-references.mjs";
import { settle } from "./axe.ts";
import { SIDEBAR, freezeDate } from "./helpers.ts";

test.use({ dist: process.env["LK_E2E_DIST"] ?? process.env["LK_DIST"] ?? "dist" });

async function open(page: Page, url: string): Promise<void> {
  await freezeDate(page);
  await page.goto(url);
  await expect(page.locator("h1").first()).toBeVisible();
  await settle(page);
}

// ---- Landmarks ------------------------------------------------------------------------------------------------------------------------
test.describe("landmarks", () => {
  test.use({ scenario: "design-shell-admin" });

  test("a signed-in screen has one banner, one main, the primary sidebar, two named navigations and a place for notifications", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes`);
    const landmarks = await page.evaluate(() => {
      const names = (selector: string): string[] => [...document.querySelectorAll(selector)].map((e) => e.getAttribute("aria-label") ?? "");
      return {
        main: document.querySelectorAll("main, [role='main']").length,
        banner: document.querySelectorAll("header, [role='banner']").length,
        contentinfo: document.querySelectorAll("footer, [role='contentinfo']").length,
        complementary: names("aside, [role='complementary']"),
        navigation: names("nav, [role='navigation']"),
        regions: names("[role='region']"),
        h1: document.querySelectorAll("h1").length,
        skip: document.querySelector("a[href='#main']")?.textContent ?? null,
      };
    });
    expect(landmarks.main).toBe(1);
    expect(landmarks.banner).toBe(1);
    expect(landmarks.contentinfo).toBe(0);
    expect(landmarks.complementary).toEqual(["Primary"]);
    expect(landmarks.navigation.sort()).toEqual(["Breadcrumb", "Sections"]);
    expect(landmarks.regions).toContain("Notifications");
    expect(landmarks.h1).toBe(1);
    expect(landmarks.skip).toBe("Skip to content");
    // The banner is the app's header, and it is not inside the main region; the sidebar is not either.
    expect(await page.evaluate(() => document.querySelector("main")?.contains(document.querySelector("header")) ?? false)).toBe(false);
    expect(await page.evaluate(() => document.querySelector("main")?.contains(document.querySelector("aside")) ?? false)).toBe(false);
  });

  // A page that has just loaded has focus on the document, so the skip link is the first stop of the first Tab (WCAG 2.4.1): focus goes to
  // the h1 only for a page the person moved to inside the app (the "focus" rows below).
  test("on a page that has just loaded the first Tab is the skip link, it shows when it has focus, and it moves focus to the main region", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes`);
    expect(await page.evaluate(() => document.activeElement === document.body)).toBe(true);
    await page.keyboard.press("Tab");
    const skip = page.getByRole("link", { name: "Skip to content" });
    await expect(skip).toBeFocused();
    await expect(skip).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(page.locator("main")).toBeFocused();
  });

  test("the skip link is the first stop of the page and the last one backwards: going back from it leaves the page", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes`);
    const stop = (): Promise<string> =>
      page.evaluate(() => {
        const el = document.activeElement;
        return el === null || el === document.body ? "" : (el.getAttribute("aria-label") ?? el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 40);
      });
    let first = "";
    let left = false;
    for (let i = 0; i < 120 && !left; i++) {
      await page.keyboard.press("Shift+Tab");
      const name = await stop();
      if (name === "") left = true;
      else first = name;
    }
    expect(left, "going back through the page ends at its first stop").toBe(true);
    expect(first).toBe("Skip to content");
  });

  test("a public page has the one main and no sidebar or banner", async ({ page, harness }) => {
    await page.goto(`${harness.url}/403?req=req-1`);
    await expect(page.locator("h1")).toHaveCount(1);
    await expect(page.locator("main")).toHaveCount(1);
    await expect(page.locator("aside")).toHaveCount(0);
    await expect(page.locator("header")).toHaveCount(0);
  });
});

// ---- The sidebar's counts: what a screen reader says -------------------------------------------------------------------------------------
test.describe("the sidebar's counts have names", () => {
  test.use({ scenario: "design-shell-admin" });

  const threeRequests = async (page: Page): Promise<void> => {
    await page.route(/\/api\/v1\/admin\/access-requests/, async (route) => {
      const user = (n: number) => ({ id: `0000000${n}-0000-4000-8000-000000000000`, user: `0000000${n}-0000-4000-8000-000000000001`, email: `person${n}@example.test`, requested_role: "editor", reason: "I need to edit.", created_at: FROZEN_NOW - n * 60_000, status: "pending" });
      await route.fulfill({ json: { items: [user(1), user(2), user(3)], next: null } });
    });
  };

  test("expanded: the number and the word are one name, for Findings, Approvals and Admin", async ({ page, harness }) => {
    await threeRequests(page);
    await open(page, `${harness.url}/swimlanes`);
    const nav = SIDEBAR(page).getByRole("navigation", { name: "Sections" });
    await expect(nav.getByRole("link", { name: "Findings 11 open", exact: true })).toBeVisible();
    await expect(nav.getByRole("link", { name: "Approvals 6 pending", exact: true })).toBeVisible();
    await expect(nav.getByRole("link", { name: "Admin 3 pending requests", exact: true })).toBeVisible();
  });

  test("collapsed: the name carries the count, and the badge itself is hidden from a screen reader", async ({ page, harness }) => {
    await threeRequests(page);
    await open(page, `${harness.url}/swimlanes`);
    await page.keyboard.press("ControlOrMeta+b");
    await expect(SIDEBAR(page)).toHaveAttribute("data-collapsed", "true");
    const nav = SIDEBAR(page).getByRole("navigation", { name: "Sections" });
    await expect(nav.getByRole("link", { name: "Findings, 11 open", exact: true })).toBeVisible();
    await expect(nav.getByRole("link", { name: "Approvals, 6 pending", exact: true })).toBeVisible();
    await expect(nav.getByRole("link", { name: "Admin, 3 pending requests", exact: true })).toBeVisible();
    // The drawn badge is not read as well (aria-hidden), so the name is not "Admin, 3 pending requests 3".
    expect(await nav.getByRole("link", { name: "Admin, 3 pending requests" }).locator("[aria-hidden='true']").count()).toBeGreaterThan(0);
  });

  test("one waiting request is singular", async ({ page, harness }) => {
    await page.route(/\/api\/v1\/admin\/access-requests/, (route) =>
      route.fulfill({ json: { items: [{ id: "00000001-0000-4000-8000-000000000000", user: "00000001-0000-4000-8000-000000000001", email: "a@example.test", requested_role: "editor", reason: "x", created_at: FROZEN_NOW, status: "pending" }], next: null } }),
    );
    await open(page, `${harness.url}/swimlanes`);
    await expect(SIDEBAR(page).getByRole("link", { name: "Admin 1 pending request", exact: true })).toBeVisible();
  });

  test("a viewer's sidebar has no Approvals, no Audit and no Admin, and no count in their names", async ({ page, harness }) => {
    harness.setScenario("design-shell-viewer");
    await open(page, `${harness.url}/swimlanes`);
    const nav = SIDEBAR(page).getByRole("navigation", { name: "Sections" });
    for (const name of ["Approvals", "Audit", "Admin"]) await expect(nav.getByRole("link", { name: new RegExp(`^${name}`) })).toHaveCount(0);
    await expect(nav.getByRole("link", { name: "Findings 11 open", exact: true })).toBeVisible();
  });
});

// ---- Focus after navigation and after a dialog --------------------------------------------------------------------------------------------
test.describe("focus", () => {
  test.use({ scenario: "design-swimlanes-six" });

  test("after navigating to another screen, focus is on its one h1", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes`);
    await SIDEBAR(page).getByRole("link", { name: /^Compare/ }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Compare" })).toBeFocused();
    await SIDEBAR(page).getByRole("link", { name: /^Findings/ }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Findings" })).toBeFocused();
  });

  test("a dialog opened from a button gives focus back to that button when it closes (Escape and Cancel)", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes/sitb/services`);
    const restart = page.getByRole("button", { name: "Restart tx-infinity-core" });
    await restart.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("alertdialog")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await expect(restart).toBeFocused();
    await restart.click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Cancel" }).click();
    await expect(restart).toBeFocused();
  });

  test("a dialog traps Tab and Shift+Tab: focus never reaches the page behind it", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes/sitb/services`);
    await page.getByRole("button", { name: "Restart tx-infinity-core" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toBeVisible();
    for (const key of ["Tab", "Shift+Tab"]) {
      for (let i = 0; i < 12; i++) {
        await page.keyboard.press(key);
        const inside = await page.evaluate(() => {
          const d = document.querySelector("[role='alertdialog']");
          return d !== null && document.activeElement !== null && d.contains(document.activeElement);
        });
        expect(inside, `${key} #${i + 1}: focus left the dialog`).toBe(true);
      }
    }
  });
});

// ---- Live regions -------------------------------------------------------------------------------------------------------------------------
test.describe("live regions", () => {
  const regionsOf = (page: Page) =>
    page.evaluate(() =>
      [...document.querySelectorAll("[aria-live], [role='status'], [role='alert'], [role='log']")].map((el) => ({
        role: el.getAttribute("role"),
        live: el.getAttribute("aria-live"),
        label: el.getAttribute("aria-label") ?? (el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 40),
        insideAlert: el.parentElement?.closest("[role='alert']") !== null,
        tag: el.tagName.toLowerCase(),
      })),
    );

  for (const [id, scenario, url] of [
    ["swimlanes", "design-swimlanes-six", "/swimlanes"],
    ["file view", "design-file-core", "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml"],
    ["findings", "design-findings-open", "/findings"],
    ["approvals", "design-approvals-pending", "/approvals"],
    ["admin users", "design-admin-users", "/admin/users"],
    // T5
    ["pull requests", "design-prs-list", "/swimlanes/sitb/pull-requests"],
    ["a masked file (the Reveal announcements)", "design-masking-reveal", "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml"],
    ["sentinels", "design-admin-t5-sentinels", "/admin/sentinels"],
    ["retention", "design-admin-t5-retention", "/admin/retention"],
    ["the audit log with attribution", "design-attribution-audit", "/audit"],
  ] as const) {
    test(`${id}: only an alert is assertive, and every other live region is polite`, async ({ page, harness }) => {
      harness.setScenario(scenario);
      await open(page, `${harness.url}${url}`);
      const regions = await regionsOf(page);
      expect(regions.length, "a screen with live data has at least the live pill").toBeGreaterThan(0);
      for (const r of regions) {
        if (r.role === "alert") continue; // implicit assertive: an error or a warning that needs attention
        expect(r.live === null || r.live === "polite" || r.live === "off", `${r.tag}[role=${String(r.role)}] "${r.label}" is aria-live=${String(r.live)}`).toBe(true);
      }
      // The connection pill says whether live updates are on, and it is polite.
      const pill = regions.find((r) => /Live updates/.test(r.label));
      expect(pill, "the live indicator is a live region").toBeDefined();
    });
  }

  test.describe("what is announced, and when", () => {
    test.use({ scenario: "design-swimlanes-six" });

    test("a region that announces exists before its text arrives (the copy confirmation)", async ({ page, harness }) => {
      await open(page, `${harness.url}/swimlanes/sitb/services`);
      const copy = page.getByRole("button", { name: "Copy swimlane name" });
      const status = copy.locator("xpath=following::*[@role='status'][1]");
      await expect(status).toBeAttached();
      expect((await status.textContent())?.trim() ?? "").toBe("");
    });

    test("an error is an alert (assertive)", async ({ page, harness }) => {
      await page.route(/\/api\/v1\/swimlanes\/sitb$/, (route) => route.fulfill({ status: 500, contentType: "application/problem+json", body: JSON.stringify({ type: "urn:lanekeeper:problem:internal", title: "Boom", status: 500, request_id: "req-1" }) }));
      await open(page, `${harness.url}/swimlanes/sitb/services`);
      await expect(page.getByRole("alert").first()).toBeVisible();
    });
  });
});

// ---- prefers-reduced-motion ---------------------------------------------------------------------------------------------------------------
test.describe("prefers-reduced-motion", () => {
  test.use({ scenario: "design-access-onboarding" });

  /** A computed `animation-duration` ("0.01ms", "0s", "1s", or a list of them) as the longest, in milliseconds. */
const ms = (value: string): number => Math.max(0, ...value.split(",").map((v) => (v.trim().endsWith("ms") ? parseFloat(v) : parseFloat(v) * 1000)));
const stills = (v: { name: string; duration: string }): boolean => v.name === "none" || ms(v.duration) <= 1;

/** Every keyframe animation in the page that runs, and every transition that moves something (a colour that fades is not movement). */
  const motion = (page: Page) =>
    page.evaluate(() =>
      document
        .getAnimations()
        .filter((a) => a.playState === "running")
        .map((a) => {
          const timing = a.effect?.getComputedTiming();
          const target = a.effect instanceof KeyframeEffect ? a.effect.target : null;
          const kind = a instanceof CSSTransition ? "transition" : "animation";
          const name = a instanceof CSSAnimation ? a.animationName : a instanceof CSSTransition ? a.transitionProperty : "";
          return { kind, name, duration: Number(timing?.duration ?? 0), iterations: timing?.iterations ?? 1, where: target ? `${target.tagName.toLowerCase()}.${(target.getAttribute("class") ?? "").slice(0, 40)}` : "" };
        })
        .filter((m) => m.kind === "animation" || /^(transform|translate|rotate|scale|top|left|right|bottom|inset|margin|width|height)/.test(m.name)),
    );

  async function verifying(page: Page, url: string): Promise<void> {
    await page.route("**/api/v1/me/github-token", () => new Promise(() => undefined));
    await open(page, `${url}/access/onboarding`);
    await page.getByRole("textbox", { name: "2 · Paste the token" }).fill("github_pat_11A0000000_x");
    await page.getByRole("button", { name: "Verify and save" }).click();
    await expect(page.getByRole("button", { name: "Verifying…" })).toBeVisible();
  }

  // base.css: "The spinner slows and the pulse stops; a frozen spinner would say nothing. Fade and pop-in skip."
  test("with reduce, the spinner turns slowly (2.4 s a turn) and nothing else moves: no pulse, no fade, no pop-in, no sliding", async ({ page, harness }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await verifying(page, harness.url);
    const moving = await motion(page);
    expect(moving.filter((m) => m.name !== "lk-spin"), JSON.stringify(moving)).toEqual([]);
    expect(moving.length, "the spinner is on the page").toBeGreaterThan(0);
    expect(
      moving.every((m) => m.duration >= 2400),
      JSON.stringify(moving),
    ).toBe(true);
  });

  test("without the preference the same spinner turns faster (the check above is not vacuous)", async ({ page, harness }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await verifying(page, harness.url);
    const spins = (await motion(page)).filter((m) => m.name === "lk-spin");
    expect(spins.length, JSON.stringify(spins)).toBeGreaterThan(0);
    expect(
      spins.every((m) => m.duration < 2400),
      JSON.stringify(spins),
    ).toBe(true);
  });

  test("with reduce, a pop-in does not animate, and nothing relies on motion to say something", async ({ page, harness }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    harness.setScenario("design-shell-admin");
    await open(page, `${harness.url}/swimlanes`);
    await SIDEBAR(page).getByRole("button", { name: /Priya Shah/ }).click();
    const menu = page.getByRole("menu");
    await expect(menu).toBeVisible();
    const style = await menu.evaluate((el) => ({ name: getComputedStyle(el).animationName, duration: getComputedStyle(el).animationDuration }));
    expect(stills(style), JSON.stringify(style)).toBe(true);
  });
});

// ---- A focus indicator, in normal and in forced colours ------------------------------------------------------------------------------------
type Focused = { where: string; outline: boolean; shadow: boolean; border: boolean };

/** Tab through the page and say, for each stop, whether anything is drawn to show where focus is (outline, box-shadow ring, or a changed border). */
async function tabStops(page: Page, count: number): Promise<Focused[]> {
  const stops: Focused[] = [];
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  for (let i = 0; i < count; i++) {
    await page.keyboard.press("Tab");
    stops.push(
      await page.evaluate(() => {
        const el = document.activeElement;
        if (el === null || el === document.body) return { where: "body", outline: false, shadow: false, border: false };
        const cs = getComputedStyle(el);
        const outline = cs.outlineStyle !== "none" && parseFloat(cs.outlineWidth) > 0 && !/rgba\(.*, 0\)$/.test(cs.outlineColor);
        const shadow = cs.boxShadow !== "none" && cs.boxShadow !== "";
        // A focused field changes its border (hover and focus tokens): compare with the same element's own non-focus rule is not possible here, so a border counts only for fields.
        const border = ["INPUT", "SELECT", "TEXTAREA"].includes(el.tagName) && cs.borderTopStyle !== "none";
        const name = `${el.tagName.toLowerCase()}${el.getAttribute("role") ? `[${el.getAttribute("role")}]` : ""} "${(el.getAttribute("aria-label") ?? el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 30)}"`;
        return { where: name, outline, shadow, border };
      }),
    );
  }
  return stops;
}

const TAB_PAGES = [
  ["swimlanes", "design-swimlanes-six", "/swimlanes"],
  ["services", "design-swimlanes-six", "/swimlanes/sitb/services"],
  ["findings", "design-findings-open", "/findings"],
  ["admin users", "design-admin-users", "/admin/users"],
  ["onboarding", "design-access-onboarding", "/access/onboarding"],
  // T5: the pages whose controls are new (the state filter and the grid of pull requests, the chips of a file's history, the Reveal buttons,
  // the buttons of the OS Login, severity-rule and retention sections).
  ["pull requests", "design-prs-list", "/swimlanes/sitb/pull-requests"],
  ["a masked file", "design-masking-reveal", "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml"],
  ["the audit log with attribution", "design-attribution-audit", "/audit"],
  ["OS Login mappings", "design-admin-t5-oslogin", "/admin/oslogin"],
  ["severity rules", "design-admin-t5-severity", "/admin/severity"],
  ["retention", "design-admin-t5-retention", "/admin/retention"],
] as const;

for (const colours of ["normal", "forced"] as const) {
  test.describe(`a focus indicator on every stop, ${colours === "forced" ? "in forced colours (high contrast)" : "in normal colours"}`, () => {
    for (const [id, scenario, url] of TAB_PAGES) {
      test(id, async ({ page, harness }) => {
        test.setTimeout(120_000);
        harness.setScenario(scenario);
        if (colours === "forced") await page.emulateMedia({ forcedColors: "active" });
        await open(page, `${harness.url}${url}`);
        if (colours === "forced") expect(await page.evaluate(() => matchMedia("(forced-colors: active)").matches)).toBe(true);
        const stops = await tabStops(page, 24);
        const real = stops.filter((s) => s.where !== "body");
        expect(real.length, "Tab reached controls").toBeGreaterThan(5);
        const bare = real.filter((s) => !(s.outline || s.shadow || s.border));
        expect(bare.map((s) => s.where), `controls that take focus and draw nothing to show it (${colours} colours)`).toEqual([]);
      });
    }
  });
}

test.describe("forced colours: nothing disappears", () => {
  test.use({ scenario: "design-swimlanes-six" });

  test("text keeps a colour, status is not colour alone, and a control keeps a boundary", async ({ page, harness }) => {
    await page.emulateMedia({ forcedColors: "active" });
    await open(page, `${harness.url}/swimlanes`);
    const result = await page.evaluate(() => {
      const bad: string[] = [];
      const label = (el: Element): string => `${el.tagName.toLowerCase()} "${(el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 24)}"`;
      // Text drawn in a colour that is the same as what is behind it would vanish; in forced colours the browser picks, so any text whose
      // computed colour equals the computed colour of its box's background is a finding.
      for (const el of document.querySelectorAll("main *")) {
        if (el.children.length > 0 || (el.textContent ?? "").trim() === "") continue;
        const cs = getComputedStyle(el);
        if (cs.visibility === "hidden" || cs.display === "none") continue;
        if (cs.color === cs.backgroundColor && cs.backgroundColor !== "rgba(0, 0, 0, 0)") bad.push(`same colour: ${label(el)}`);
      }
      // A badge with a state is an icon or a word as well as a colour.
      for (const el of document.querySelectorAll("[data-slot='badge'], [data-slot='status-badge']")) {
        if ((el.textContent ?? "").trim() === "" && el.querySelector("svg") === null) bad.push(`badge with no word and no icon: ${label(el)}`);
      }
      // A button or a field draws a border, or has an icon and a name (a ghost button in forced colours is text on the canvas).
      const bordered = (el: Element): boolean => {
        const cs = getComputedStyle(el);
        return cs.borderTopStyle !== "none" && parseFloat(cs.borderTopWidth) > 0;
      };
      for (const el of document.querySelectorAll("main input:not([type='checkbox']):not([type='radio']), main select, main textarea")) {
        // The field, or the box that draws it (a search field is an input in a bordered box), has a border.
        let ok = bordered(el);
        for (let a = el.parentElement, depth = 0; !ok && a !== null && depth < 3; a = a.parentElement, depth++) ok = bordered(a) && a.getBoundingClientRect().height < el.getBoundingClientRect().height + 16;
        if (!ok) bad.push(`field without a border: ${label(el)}`);
      }
      return bad;
    });
    expect(result).toEqual([]);
  });
});

// ---- Zoom to 200% at 1280 wide ----------------------------------------------------------------------------------------------------------------
test.describe("zoom to 200% at 1280 wide (a 640 by 450 window)", () => {
  test.use({ scenario: "design-swimlanes-six", viewport: { width: 640, height: 450 } });

  test("the page still has its landmarks, its one h1 and a way to the content", async ({ page, harness }) => {
    await open(page, `${harness.url}/swimlanes`);
    await expect(page.locator("main")).toHaveCount(1);
    await expect(page.locator("h1")).toHaveCount(1);
    const h1 = await page.locator("h1").boundingBox();
    expect(h1, "the heading is in the window").not.toBeNull();
    expect((h1?.x ?? -1) >= 0 && (h1?.x ?? 0) + (h1?.width ?? 0) <= 640 + 1).toBe(true);
    // The window is not a second scroller: the document does not scroll sideways, and main's content does not either, except inside a table.
    const scroll = await page.evaluate(() => ({ doc: document.documentElement.scrollWidth - document.documentElement.clientWidth, main: (document.querySelector("main")?.scrollWidth ?? 0) - (document.querySelector("main")?.clientWidth ?? 0) }));
    expect(scroll.doc).toBeLessThanOrEqual(1);
    expect(scroll.main).toBeLessThanOrEqual(1);
  });
});
