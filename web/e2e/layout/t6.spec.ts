// The geometry of the T6 screens, read with boxes and scroll sizes (never a screenshot), at 1024, 1280, 1440 and 2560 wide, with the
// words as they are and 40% longer: where the cases of the accessibility list are already measured (layout.spec.ts, at 1024 and 1440 for
// a state that needs steps, and pseudo.spec.ts at 1024 and 1280) this adds what they leave out.
//
//   - the Pickup column of the swimlane list is 88 px at least, holds its chips side by side and never lets one reach past it;
//   - a service's row on the Services tab keeps its height, and a chip's qualifier ("(client cache)") gives way with an ellipsis before the
//     badge does; the words of the badge itself are never cut;
//   - a row of the file tree keeps some of its name next to the marks at its end, and its marks stay inside the row, at the narrowest pane;
//   - the strip of the Services tab (the filter, the count and "Notify services"), the auto-notify line and the config-server banner stay
//     inside the 360 px pane of a 1024 px window, wrapping rather than being cut off;
//   - the file page's "Saved" banner holds its buttons inside the page, and the pickup chip is at the end of the title row;
//   - every dialog of the notify and restart pieces fits the window, with its buttons inside it, however long what it lists;
//   - "As served" at 2560 and with every label 40% longer: the form, the facts, the placeholders, the sources and the text.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { settle } from "../a11y/axe.ts";
import { freezeDate, hold, modal } from "../a11y/helpers.ts";
import {
  API,
  CORE_FILE,
  SERVICES_URL,
  bannerRestart,
  confirmNotify,
  longText,
  manyFiles,
  openConfigServerRestart,
  openNotify,
  patchLane,
  patchServed,
  saveFile,
  servedUrl,
  svc,
  typeToConfirm,
} from "../a11y/t6.ts";
import { auditLayout, describeReport, hasDefects, requestsQuiet } from "./audit.ts";
import { pseudoCount, pseudoLocalise } from "./pseudo.ts";
import { auditTables } from "./tables.ts";

test.use({ dist: process.env["LK_E2E_DIST"] ?? process.env["LK_DIST"] ?? "dist", colorScheme: "light" });

const SIZES = [
  { width: 1024, height: 768 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
  { width: 2560, height: 1440 },
] as const;
const CAP = 1680;
const SERVED_ADDRESS = "/swimlanes/sitb/served";

type Harness = Parameters<typeof requestsQuiet>[1];

/** Open `url` in the state `prepare` makes, and hand each size to `check` (the words as they are, then 40% longer where `stretch`). */
async function atEverySize(
  page: Page,
  harness: Harness,
  options: { url: string; before?: () => Promise<void>; prepare?: () => Promise<void>; sizes?: ReadonlyArray<{ width: number; height: number }>; stretchAt?: ReadonlyArray<number> },
  check: (label: string, size: { width: number; height: number }, stretched: boolean) => Promise<void>,
): Promise<void> {
  await freezeDate(page);
  await options.before?.();
  await page.goto(`${harness.url}${options.url}`);
  await expect(page.locator("h1").first()).toBeVisible();
  await options.prepare?.();
  await requestsQuiet(page, harness);
  const sizes = options.sizes ?? SIZES;
  for (const size of sizes) {
    await page.setViewportSize(size);
    await settle(page, 250);
    await check(`${size.width}x${size.height}`, size, false);
  }
  const stretched = sizes.filter((s) => (options.stretchAt ?? [1024, 1280, 2560]).includes(s.width));
  if (stretched.length === 0) return;
  await pseudoLocalise(page);
  expect(await pseudoCount(page), "the words were stretched").toBeGreaterThan(0);
  for (const size of stretched) {
    await page.setViewportSize(size);
    await settle(page, 300);
    await check(`${size.width}x${size.height} (labels +40%)`, size, true);
  }
}

/** The defects the audit and the table check find on the page as it is now. */
async function noDefects(page: Page, label: string): Promise<void> {
  const report = await auditLayout(page, { skip: ".monaco-editor" });
  expect.soft(hasDefects(report), describeReport(label, report)).toBe(false);
  const tables = await auditTables(page);
  expect.soft(tables.defects, `${label}: ${JSON.stringify(tables.defects)}`).toEqual([]);
}

// ---- The Pickup column -------------------------------------------------------------------------------------------------------------------
test.describe("the Pickup column of the swimlane list", () => {
  test.use({ scenario: "design-pickup-states" });

  test("is 88 px or more, holds its chips side by side inside the cell, in a row of one height, at every width", async ({ page, harness }) => {
    test.setTimeout(120_000);
    await atEverySize(page, harness, { url: "/swimlanes" }, async (label) => {
      const cells = await page.evaluate(() => {
        const grid = document.querySelector("[role='grid']");
        if (grid === null) return null;
        // The column is the one whose cells hold the chips (its header's word is stretched in the second half of the run).
        const first = [...grid.querySelectorAll("[role='gridcell']")].find((c) => c.querySelector("[data-slot='pickup-chip'], [data-slot='pickup-counts']") !== null);
        const index = first?.getAttribute("aria-colindex") ?? null;
        const head = index === null ? undefined : grid.querySelector(`[role='columnheader'][aria-colindex='${index}']`);
        if (head === undefined || head === null) return null;
        const headBox = head.getBoundingClientRect();
        const rows = [...grid.querySelectorAll("[role='row']")].filter((r) => r.querySelector("[role='gridcell']") !== null);
        return {
          header: { left: headBox.left, width: headBox.width },
          rows: rows.map((row) => {
            const cell = row.querySelector(`[role='gridcell'][aria-colindex='${index ?? ""}']`);
            const box = row.getBoundingClientRect();
            const cellBox = cell?.getBoundingClientRect();
            return {
              row: { height: box.height },
              cell: cellBox === undefined ? null : { left: cellBox.left, right: cellBox.right, width: cellBox.width, top: cellBox.top, bottom: cellBox.bottom },
              chips: [...(cell?.querySelectorAll("[data-slot='pickup-chip']") ?? [])].map((c) => {
                const r = c.getBoundingClientRect();
                return { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
              }),
            };
          }),
        };
      });
      expect(cells, `${label}: the Pickup column is on the list`).not.toBeNull();
      if (cells === null) return;
      expect.soft(cells.header.width, `${label}: the column is 88 px at least`).toBeGreaterThanOrEqual(87.5);
      const heights = new Set(cells.rows.map((r) => Math.round(r.row.height)));
      expect.soft(heights.size, `${label}: every row has the one height (${[...heights].join(", ")})`).toBe(1);
      let withChips = 0;
      for (const r of cells.rows) {
        const cell = r.cell;
        if (cell === null) continue;
        expect.soft(Math.abs(cell.left - cells.header.left) < 1.5 && Math.abs(cell.width - cells.header.width) < 1.5, `${label}: the cell is under its header`).toBe(true);
        if (r.chips.length > 0) withChips += 1;
        const sorted = [...r.chips].sort((a, b) => a.left - b.left);
        sorted.forEach((chip, i) => {
          expect.soft(chip.left >= cell.left - 1 && chip.right <= cell.right + 1, `${label}: a chip (${Math.round(chip.left)}..${Math.round(chip.right)}) is inside its cell (${Math.round(cell.left)}..${Math.round(cell.right)})`).toBe(true);
          expect.soft(chip.top >= cell.top - 1 && chip.bottom <= cell.bottom + 1, `${label}: a chip is inside its row`).toBe(true);
          const next = sorted[i + 1];
          if (next !== undefined) {
            expect.soft(next.left >= chip.right - 0.5, `${label}: two chips side by side do not cover each other`).toBe(true);
            expect.soft(Math.abs(next.top - chip.top) < 2, `${label}: two chips share a line`).toBe(true);
          }
        });
      }
      expect(withChips, `${label}: a swimlane with something waiting draws its chips`).toBeGreaterThanOrEqual(1);
      await noDefects(page, label);
    });
  });

  test("the legend names the four states and none of them is cut off", async ({ page, harness }) => {
    // (With every label 40% longer the legend is measured by pseudo.spec.ts, over the case of the swimlane list that draws it.)
    await atEverySize(page, harness, { url: "/swimlanes", sizes: [SIZES[0], SIZES[2]], stretchAt: [] }, async (label) => {
      const legend = page.getByText("Needs config-server restart", { exact: true });
      await expect(legend.first(), `${label}: the legend has Needs config-server restart`).toBeVisible();
      const clipped = await legend.first().evaluate((el) => {
        const r = el.getBoundingClientRect();
        return r.right > window.innerWidth || r.left < 0;
      });
      expect.soft(clipped, `${label}: the legend entry is inside the window`).toBe(false);
      await noDefects(page, label);
    });
  });
});

// ---- The Services tab -------------------------------------------------------------------------------------------------------------------------
test.describe("the Services tab", () => {
  test.use({ scenario: "design-pickup-states" });

  test("a service's row keeps its height and the cells their place; the qualifier of a chip gives way with an ellipsis, never the badge", async ({ page, harness }) => {
    test.setTimeout(120_000);
    await atEverySize(page, harness, { url: SERVICES_URL }, async (label, _size, stretched) => {
      const rows = await page.getByRole("table", { name: "Services" }).evaluate((table) =>
        [...table.querySelectorAll("[role='row']")]
          .filter((r) => r.querySelector("[role='cell']") !== null)
          .map((row) => {
            const rowBox = row.getBoundingClientRect();
            const cells = [...row.querySelectorAll("[role='cell']")].map((c) => c.getBoundingClientRect());
            const chip = row.querySelector("[data-slot='pickup-chip']");
            const badge = chip?.querySelector("[data-slot='status-badge']") ?? null;
            const badgeBox = badge?.getBoundingClientRect();
            const qualifier = chip === null ? null : ([...chip.querySelectorAll("span")].find((s) => /^\((client cache|time passed)\)$/.test((s.textContent ?? "").trim())) ?? null);
            const action = row.querySelector("button")?.getBoundingClientRect();
            return {
              height: rowBox.height,
              bottom: rowBox.bottom,
              top: rowBox.top,
              cell: cells[1] ? { left: cells[1].left, right: cells[1].right } : null,
              badge: badgeBox ? { left: badgeBox.left, right: badgeBox.right, text: (badge?.textContent ?? "").trim(), scrolls: (badge?.scrollWidth ?? 0) > (badge?.clientWidth ?? 0) + 1 } : null,
              qualifier: qualifier === null ? null : { text: (qualifier.textContent ?? "").trim(), ellipsis: getComputedStyle(qualifier).textOverflow === "ellipsis" || getComputedStyle(qualifier).overflow === "hidden", clipped: qualifier.scrollWidth > qualifier.clientWidth + 1, right: qualifier.getBoundingClientRect().right, left: qualifier.getBoundingClientRect().left, width: qualifier.getBoundingClientRect().width },
              action: action ? { left: action.left, right: action.right, top: action.top, bottom: action.bottom } : null,
            };
          }),
      );
      expect(rows.length, `${label}: seven services`).toBe(7);
      for (const r of rows) {
        expect.soft(Math.round(r.height), `${label}: a row of the Services tab is 52 px`).toBe(52);
        if (r.cell !== null && r.badge !== null) {
          expect.soft(r.badge.right <= r.cell.right + 1 && r.badge.left >= r.cell.left - 1, `${label}: the badge "${r.badge.text}" is inside its cell`).toBe(true);
          expect.soft(r.badge.scrolls, `${label}: the words of the badge "${r.badge.text}" are not cut`).toBe(false);
        }
        if (r.qualifier !== null && r.cell !== null) {
          // It is inside the cell, or it has given way completely (no width at all: it is the first thing to go, before the badge's words).
          expect.soft(r.qualifier.right <= r.cell.right + 1 || r.qualifier.width < 1, `${label}: the qualifier is inside the cell or has given way`).toBe(true);
          if (r.qualifier.clipped && r.qualifier.width >= 1) expect.soft(r.qualifier.ellipsis, `${label}: a qualifier that is cut short says so with an ellipsis`).toBe(true);
          expect.soft(r.qualifier.text, `${label}: what it says is still in the page (a screen reader, the tooltip) whatever is drawn`).toMatch(/^\((client cache|time passed)\)$/);
        }
        if (r.action !== null) expect.soft(r.action.top >= r.top - 1 && r.action.bottom <= r.bottom + 1, `${label}: the Restart button is inside its row`).toBe(true);
      }
      if (stretched) expect.soft(rows.every((r) => r.badge !== null), `${label}: every service has a chip`).toBe(true);
      await noDefects(page, label);
    });
  });

  test("the rows of the file tree keep part of the name beside the marks at their end, and the marks stay inside the row, in the narrowest pane", async ({ page, harness }) => {
    test.setTimeout(120_000);
    await atEverySize(
      page,
      harness,
      {
        url: SERVICES_URL,
        async prepare() {
          const rows = page.getByRole("tree").getByRole("treeitem");
          const before = await rows.count();
          await page.getByRole("button", { name: "Open tx-infinity-api" }).click();
          await expect.poll(() => rows.count()).toBeGreaterThan(before);
        },
      },
      async (label) => {
        const rows = await page.getByRole("tree").evaluate((tree) =>
          [...tree.querySelectorAll("[role='treeitem']")].map((item) => {
            const box = item.getBoundingClientRect();
            const inner = item.firstElementChild as HTMLElement | null;
            const kids = inner === null ? [] : [...inner.children];
            const labelBox = kids.find((k) => k.classList.contains("truncate"))?.getBoundingClientRect();
            const trailing = kids.find((k) => k.classList.contains("shrink-0") && k.querySelector("[data-slot='pickup-chip'], [data-slot='status-badge']") !== null)?.getBoundingClientRect();
            const chip = item.querySelector("[data-slot='pickup-chip']")?.getBoundingClientRect();
            return {
              name: (item.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 40),
              row: { left: box.left, right: box.right, top: box.top, bottom: box.bottom },
              label: labelBox ? { right: labelBox.right, width: labelBox.width } : null,
              trailing: trailing ? { left: trailing.left, right: trailing.right } : null,
              chip: chip ? { left: chip.left, right: chip.right, top: chip.top, bottom: chip.bottom } : null,
            };
          }),
        );
        const marked = rows.filter((r) => r.chip !== null);
        expect(marked.length, `${label}: rows with a pickup mark`).toBeGreaterThanOrEqual(4);
        for (const r of marked) {
          if (r.trailing !== null) {
            expect.soft(r.trailing.right <= r.row.right + 1, `${label}: the marks of "${r.name}" are inside the row (${Math.round(r.trailing.right)} against ${Math.round(r.row.right)})`).toBe(true);
            if (r.label !== null) expect.soft(r.label.right <= r.trailing.left + 1, `${label}: the name of "${r.name}" ends before the marks begin`).toBe(true);
          }
          const c = r.chip;
          if (c !== null) expect.soft(c.right <= r.row.right + 1 && c.top >= r.row.top - 1 && c.bottom <= r.row.bottom + 1, `${label}: the mark of "${r.name}" is inside its row`).toBe(true);
        }
        await noDefects(page, label);
      },
    );
  });

  test("the strip (the filter, the count and Notify services), the auto-notify line and the banner stay inside the pane: they wrap, they are not cut off", async ({ page, harness }) => {
    test.setTimeout(120_000);
    harness.setScenario("design-notify-config-server");
    await atEverySize(page, harness, { url: SERVICES_URL }, async (label, size) => {
      const geometry = await page.evaluate(() => {
        const panel = document.querySelector("[role='tabpanel']");
        const strip = document.querySelector("[data-slot='services-strip']");
        const box = (el: Element | null | undefined) => {
          const r = el?.getBoundingClientRect();
          return r === undefined ? null : { left: r.left, right: r.right, top: r.top, bottom: r.bottom, height: r.height };
        };
        const notify = [...document.querySelectorAll("button")].find((b) => (b.textContent ?? "").includes("Notify services"));
        const restart = [...document.querySelectorAll("button")].find((b) => (b.getAttribute("aria-label") ?? "").startsWith("Restart config-server"));
        const filter = document.querySelector("input[aria-label='Filter services']");
        const line = document.querySelector("[role='group'][aria-label='Auto-notify']");
        const banner = restart?.closest("[data-slot='alert']") ?? null;
        return {
          panel: box(panel),
          strip: box(strip),
          notify: box(notify),
          restart: box(restart),
          filter: box(filter),
          line: box(line),
          lineKids: [...(line?.children ?? [])].map((k) => box(k)),
          banner: box(banner),
          bannerKids: [...(banner?.querySelectorAll("button, a, [data-slot='alert-title']") ?? [])].map((k) => box(k)),
        };
      });
      const { panel } = geometry;
      expect(panel, `${label}: the Services tab is on screen`).not.toBeNull();
      if (panel === null) return;
      for (const [what, b] of [["the filter", geometry.filter], ["the Notify services button", geometry.notify], ["the banner's button", geometry.restart], ["the strip", geometry.strip], ["the auto-notify line", geometry.line], ["the banner", geometry.banner]] as const) {
        if (b === null) continue;
        expect.soft(b.left >= panel.left - 1 && b.right <= panel.right + 1, `${label}: ${what} (${Math.round(b.left)}..${Math.round(b.right)}) is inside the pane (${Math.round(panel.left)}..${Math.round(panel.right)})`).toBe(true);
      }
      for (const k of geometry.lineKids) if (k !== null) expect.soft(k.right <= panel.right + 1, `${label}: a part of the auto-notify line is inside the pane`).toBe(true);
      for (const k of geometry.bannerKids) if (k !== null) expect.soft(k.right <= panel.right + 1, `${label}: a part of the banner is inside the pane`).toBe(true);
      if (geometry.strip !== null) expect.soft(geometry.strip.height >= 32 - 0.5, `${label}: the strip is 32 px or taller (it wraps, and never gets shorter)`).toBe(true);
      // A pane of 360 px is where the strip has to wrap; with room it is one line of 32 px.
      if (size.width >= 1440 && geometry.strip !== null) expect.soft(geometry.strip.height, `${label}: with room the strip is one line (32 px and its border)`).toBeLessThanOrEqual(34);
      await noDefects(page, label);
    });
  });
});

// ---- The file page ----------------------------------------------------------------------------------------------------------------------------
test.describe("the file page after a write", () => {
  test.use({ scenario: "design-notify-file-saved" });

  test("the banner holds its buttons inside the page, and the pickup chip is at the end of the title row", async ({ page, harness }) => {
    test.setTimeout(150_000);
    await atEverySize(
      page,
      harness,
      {
        url: CORE_FILE,
        async prepare() {
          await saveFile(page);
        },
      },
      async (label, size) => {
        const geometry = await page.evaluate(() => {
          const main = document.querySelector("main")?.getBoundingClientRect();
          const banner = [...document.querySelectorAll("[data-slot='alert']")].find((a) => a.querySelector("button[aria-label='Dismiss']") !== null);
          const buttons = [...(banner?.querySelectorAll("button") ?? [])].map((b) => {
            const r = b.getBoundingClientRect();
            return { name: (b.getAttribute("aria-label") ?? b.textContent ?? "").trim(), left: r.left, right: r.right, top: r.top, bottom: r.bottom };
          });
          const h1 = document.querySelector("h1")?.getBoundingClientRect();
          const chip = document.querySelector("[data-slot='pickup-chip']")?.getBoundingClientRect();
          const facts = [...document.querySelectorAll("section[aria-label='File'] button")].find((b) => (b.getAttribute("aria-label") ?? "").startsWith("Copy path"))?.getBoundingClientRect();
          const bannerBox = banner?.getBoundingClientRect();
          return {
            main: main ? { left: main.left, right: main.right } : null,
            banner: bannerBox ? { left: bannerBox.left, right: bannerBox.right, bottom: bannerBox.bottom } : null,
            buttons,
            h1: h1 ? { top: h1.top, bottom: h1.bottom } : null,
            chip: chip ? { top: chip.top, bottom: chip.bottom, left: chip.left, right: chip.right } : null,
            path: facts ? { top: facts.top } : null,
          };
        });
        if (geometry.main === null || geometry.banner === null) throw new Error(`${label}: the page has no banner`);
        expect.soft(geometry.buttons.length, `${label}: Notify services, Restart services and Dismiss`).toBe(3);
        for (const b of geometry.buttons) expect.soft(b.left >= geometry.main.left - 1 && b.right <= geometry.main.right + 1, `${label}: "${b.name}" is inside the page`).toBe(true);
        expect.soft(geometry.banner.left >= geometry.main.left - 1 && geometry.banner.right <= geometry.main.right + 1, `${label}: the banner is inside the page`).toBe(true);
        // The chip is in the head of the file, on the row of the title when the words are as they are and there is room, and never below the path.
        if (geometry.chip !== null && geometry.h1 !== null && geometry.path !== null) {
          if (size.width >= 1440) expect.soft(geometry.chip.top < geometry.h1.bottom + 8, `${label}: the chip is on the row of the title`).toBe(true);
          expect.soft(geometry.chip.bottom <= geometry.path.top + 2, `${label}: the chip is above the row of the path`).toBe(true);
        }
        await noDefects(page, label);
      },
    );
  });
});

// ---- The dialogs ------------------------------------------------------------------------------------------------------------------------------------
/** The dialog is inside the window and so are the buttons of its footer, whatever it lists. */
async function dialogFits(page: Page, label: string): Promise<void> {
  const geometry = await page.evaluate(() => {
    const dialog = [...document.querySelectorAll("[role='dialog'], [role='alertdialog']")].find((d) => d.getClientRects().length > 0);
    if (dialog === undefined) return null;
    const r = dialog.getBoundingClientRect();
    return {
      box: { left: r.left, right: r.right, top: r.top, bottom: r.bottom },
      window: { width: window.innerWidth, height: window.innerHeight },
      buttons: [...dialog.querySelectorAll("button")]
        .filter((b) => b.getClientRects().length > 0)
        .map((b) => {
          const br = b.getBoundingClientRect();
          return { name: (b.getAttribute("aria-label") ?? b.textContent ?? "").trim().slice(0, 30), left: br.left, right: br.right, top: br.top, bottom: br.bottom };
        }),
      scrollX: dialog.scrollWidth > dialog.clientWidth + 1,
    };
  });
  if (geometry === null) throw new Error(`${label}: no dialog is open`);
  const { box, window: w } = geometry;
  expect.soft(box.left >= -0.5 && box.right <= w.width + 0.5 && box.top >= -0.5 && box.bottom <= w.height + 0.5, `${label}: the dialog (${JSON.stringify(box)}) is inside the window (${w.width}x${w.height})`).toBe(true);
  for (const b of geometry.buttons) expect.soft(b.left >= -0.5 && b.right <= w.width + 0.5 && b.top >= -0.5 && b.bottom <= w.height + 0.5, `${label}: the button "${b.name}" is inside the window`).toBe(true);
  expect.soft(geometry.scrollX, `${label}: the dialog does not scroll sideways`).toBe(false);
  await noDefects(page, label);
}

const DIALOG_SIZES = [SIZES[0], { width: 1024, height: 600 }, SIZES[2], SIZES[3]] as const;

test.describe("the dialogs of the notify and restart pieces", () => {
  test("Notify services with a hundred paths and a long list of services, confirming and answered", async ({ page, harness }) => {
    test.setTimeout(150_000);
    harness.setScenario("design-notify-services-tab");
    await atEverySize(
      page,
      harness,
      {
        url: SERVICES_URL,
        sizes: DIALOG_SIZES,
        stretchAt: [1024, 1280],
        async before() {
          await patchLane(page, (lane) => ({
            ...lane,
            services: Array.from({ length: 30 }, (_, i) => svc(`service-with-a-long-name-${String(i).padStart(2, "0")}`, { pickup: "needs_notify_or_restart", pending_since: 1, notifications_enabled: i % 2 === 0, pending_files: manyFiles(150).slice(i * 5, i * 5 + 8) })),
          }));
        },
        async prepare() {
          await openNotify(page);
        },
      },
      async (label) => {
        await dialogFits(page, `${label}: confirm`);
      },
    );
  });

  test("Notify services: the answer, the partial answer and a refusal", async ({ page, harness }) => {
    test.setTimeout(150_000);
    for (const scenario of ["design-notify-services-tab", "design-notify-partial", "design-notify-approval", "design-notify-locked"]) {
      harness.setScenario(scenario);
      await page.goto(`${harness.url}${SERVICES_URL}`);
      await freezeDate(page);
      await expect(page.locator("h1").first()).toBeVisible();
      await openNotify(page);
      await confirmNotify(page);
      await expect(modal(page).getByRole("button", { name: /Close|Notify services/ }).first()).toBeVisible();
      await requestsQuiet(page, harness);
      for (const size of DIALOG_SIZES) {
        await page.setViewportSize(size);
        await settle(page, 200);
        await dialogFits(page, `${scenario} @${size.width}x${size.height}`);
      }
      await page.keyboard.press("Escape");
      await expect(modal(page)).toHaveCount(0);
    }
  });

  test("Restart config-server: the dialog with four services, the truncated one, no offer, and the answers", async ({ page, harness }) => {
    test.setTimeout(150_000);
    for (const scenario of ["design-notify-config-server", "design-notify-config-server-truncated", "design-notify-config-server-none"]) {
      harness.setScenario(scenario);
      await page.goto(`${harness.url}${SERVICES_URL}`);
      await freezeDate(page);
      await expect(bannerRestart(page)).toBeVisible();
      await openConfigServerRestart(page);
      await expect(modal(page)).toBeVisible();
      await requestsQuiet(page, harness);
      for (const size of DIALOG_SIZES) {
        await page.setViewportSize(size);
        await settle(page, 200);
        await dialogFits(page, `${scenario} @${size.width}x${size.height}`);
      }
      await pseudoLocalise(page);
      for (const size of [SIZES[0], SIZES[1]]) {
        await page.setViewportSize(size);
        await settle(page, 300);
        await dialogFits(page, `${scenario} (labels +40%) @${size.width}x${size.height}`);
      }
      if (scenario === "design-notify-config-server") {
        await typeToConfirm(page, "csp-configuration-server");
        await modal(page).getByRole("button").last().click();
        await expect(modal(page).getByText(/Restart requested/)).toBeVisible();
        await dialogFits(page, `${scenario}: accepted (labels +40%)`);
      }
      await page.reload();
    }
  });

  test("Restart services on the file page: the offer, the run held at its first request, and a run that stopped early", async ({ page, harness }) => {
    test.setTimeout(180_000);
    harness.setScenario("design-notify-file-saved");
    await freezeDate(page);
    await page.goto(`${harness.url}${CORE_FILE}`);
    await expect(page.locator("h1").first()).toBeVisible();
    await saveFile(page);
    await page.getByRole("button", { name: "Restart services" }).click();
    await expect(modal(page).getByText("Restart 2 services in sitb?")).toBeVisible();
    await requestsQuiet(page, harness);
    for (const size of DIALOG_SIZES) {
      await page.setViewportSize(size);
      await settle(page, 200);
      await dialogFits(page, `restart all: confirm @${size.width}x${size.height}`);
    }
    await pseudoLocalise(page);
    for (const size of [SIZES[0], SIZES[1]]) {
      await page.setViewportSize(size);
      await settle(page, 300);
      await dialogFits(page, `restart all: confirm (labels +40%) @${size.width}x${size.height}`);
    }
    await typeToConfirm(page, "sitb");
    const gate = await hold(page, API.restart);
    await modal(page).getByRole("button").last().click();
    // (Its words are stretched: it is found by being the status line of the dialog.)
    await expect(modal(page).locator("p[role='status']")).toBeVisible();
    await dialogFits(page, "restart all: the first request is out (labels +40%)");
    gate.release();
  });
});

// ---- As served ------------------------------------------------------------------------------------------------------------------------------------
test.describe("As served", () => {
  test.use({ scenario: "design-served-text" });

  const longLists = (page: Page) => async (): Promise<void> => {
    await patchServed(page, (served) => ({
      ...served,
      text: longText(400),
      masks: undefined,
      sources: Array.from({ length: 16 }, (_, i) => ({ path: `tx-infinity-api/channels/beta/regions/eu-west/applications/tx-infinity-core/overrides/override-${String(i).padStart(2, "0")}-sit7.yml`, rank: i })),
      unresolved_placeholders: Array.from({ length: 40 }, (_, i) => `INTEGRATION_${String(i).padStart(2, "0")}_ENDPOINT_BASE_URL_FOR_THE_PARTNER_GATEWAY`),
    }));
  };

  test("the form, the facts, sixteen sources and forty placeholders and a long text, at 2560 and with every label 40% longer", async ({ page, harness }) => {
    test.setTimeout(150_000);
    await atEverySize(page, harness, { url: servedUrl(), before: longLists(page), stretchAt: [1024, 1280, 1440, 2560] }, async (label, size) => {
      const geometry = await page.evaluate(() => {
        const box = (el: Element | null | undefined) => {
          const r = el?.getBoundingClientRect();
          return r === undefined ? null : { left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width };
        };
        const form = document.querySelector("form[aria-label='What to ask the config-server for']");
        const section = document.querySelector("section[aria-label=\"The config-server's answer\"]");
        const sources = document.querySelector("ul[aria-label='Files the engine reads for this selection']");
        const text = document.querySelector("[role='table'][aria-label^='Served text']");
        return {
          content: box(document.querySelector("main > div")),
          form: box(form),
          fields: [...(form?.querySelectorAll("input, select, button[type='submit']") ?? [])].map((f) => box(f)),
          section: box(section),
          sources: box(sources),
          sourceItems: [...(sources?.children ?? [])].map((k) => box(k)),
          text: box(text),
        };
      });
      if (geometry.form === null || geometry.section === null) throw new Error(`${label}: the page has no form or no answer`);
      for (const f of geometry.fields) if (f !== null) expect.soft(f.left >= geometry.form.left - 1 && f.right <= geometry.form.right + 1, `${label}: a field of the form is inside the form`).toBe(true);
      for (const k of geometry.sourceItems) if (k !== null && geometry.sources !== null) expect.soft(k.right <= geometry.sources.right + 1, `${label}: a source is inside its list`).toBe(true);
      if (geometry.text !== null) expect.soft(geometry.text.right <= geometry.section.right + 1, `${label}: the text is inside the answer`).toBe(true);
      if (size.width >= 2560 && geometry.content !== null) expect.soft(geometry.content.width, `${label}: the content stops at the design's cap of ${CAP}px`).toBeLessThanOrEqual(CAP + 1);
      await noDefects(page, label);
    });
  });

  for (const stretched of [false, true]) {
    test(`the form does not move when a request goes out: the button keeps its width and the fields their places, at every width${stretched ? ", with every label 40% longer" : ""}`, async ({ page, harness }) => {
      test.setTimeout(120_000);
      await freezeDate(page);
      await page.goto(`${harness.url}${SERVED_ADDRESS}`);
      await expect(page.getByText("Choose what to ask for")).toBeVisible();
      await page.getByRole("textbox", { name: "File" }).fill("tx-infinity-api/connection-pool.yml");
      await page.getByRole("textbox", { name: "Application" }).fill("tx-infinity-core");
      await page.getByRole("combobox", { name: "Tenant" }).selectOption("sit7");
      if (stretched) await pseudoLocalise(page);
      const fields = () =>
        page.evaluate(() => {
          const form = document.querySelector("form[aria-label='What to ask the config-server for']");
          const box = (el: Element) => {
            const r = el.getBoundingClientRect();
            return { left: Math.round(r.left * 10) / 10, top: Math.round(r.top * 10) / 10, width: Math.round(r.width * 10) / 10, height: Math.round(r.height * 10) / 10 };
          };
          return { form: form === null ? null : box(form), controls: [...(form?.querySelectorAll("input, select, button[type='submit']") ?? [])].map(box) };
        });
      const sizes = stretched ? [SIZES[0], SIZES[1], SIZES[3]] : SIZES;
      const idle = new Map<number, Awaited<ReturnType<typeof fields>>>();
      for (const size of sizes) {
        await page.setViewportSize(size);
        await settle(page, 250);
        idle.set(size.width, await fields());
      }
      const gate = await hold(page, API.served);
      await page.locator("form button[type='submit']").click();
      await expect(page.locator("form button[type='submit'][aria-busy='true']")).toBeVisible();
      for (const size of sizes) {
        await page.setViewportSize(size);
        await settle(page, 250);
        expect.soft(await fields(), `${size.width}${stretched ? " (labels +40%)" : ""}: the form and its controls are where they were before the request went out`).toEqual(idle.get(size.width));
      }
      gate.release();
    });
  }

  for (const [scenario, url] of [
    ["design-served-masked", servedUrl()],
    ["design-served-withheld", servedUrl({ file: "security/keystore.jks" })],
    ["design-served-not-text", servedUrl({ file: "tx-infinity-api/logo.png" })],
    ["design-served-empty-file", servedUrl()],
    ["design-served-not-served", servedUrl()],
    ["design-served-agent-offline", servedUrl()],
  ] as const) {
    test(`${scenario}: fits at every width and with every label 40% longer`, async ({ page, harness }) => {
      test.setTimeout(120_000);
      harness.setScenario(scenario);
      await atEverySize(page, harness, { url, stretchAt: [1024, 2560] }, async (label) => {
        await noDefects(page, `${scenario} @${label}`);
      });
    });
  }

  test("the form with every wrong field (messages under the fields) at every width and with every label 40% longer", async ({ page, harness }) => {
    test.setTimeout(120_000);
    await atEverySize(page, harness, { url: servedUrl({ file: "../../etc/passwd", application: "not an application!", tenant: "SIT 7", channel: "no channel!" }), stretchAt: [1024, 1280, 2560] }, async (label) => {
      await noDefects(page, label);
      const messages = await page.locator("form [data-slot='field-error'], form [role='alert']").count();
      expect.soft(messages, `${label}: a message under each wrong field`).toBeGreaterThanOrEqual(3);
    });
  });
});

// ---- The notify button, when it is off --------------------------------------------------------------------------------------------------------------
test.describe("the buttons that are off", () => {
  test.use({ scenario: "design-notify-viewer" });

  test("Notify services is as wide as it is when it is on, with its tooltip inside the window", async ({ page, harness }) => {
    test.setTimeout(90_000);
    await atEverySize(page, harness, { url: SERVICES_URL, sizes: [SIZES[0], SIZES[2]], stretchAt: [1024] }, async (label) => {
      // (Found by its place in the strip: with every label 40% longer its name is not "Notify services".)
      const off = page.locator("[data-slot='services-strip'] button[aria-disabled='true']");
      await off.hover();
      const tip = page.getByRole("tooltip").first();
      await expect(tip).toBeVisible();
      const [button, box] = await Promise.all([off.boundingBox(), tip.boundingBox()]);
      expect.soft(button !== null && button.width > 60, `${label}: the button has its width`).toBe(true);
      const size = page.viewportSize();
      if (box !== null && size !== null) expect.soft(box.x >= -0.5 && box.x + box.width <= size.width + 0.5, `${label}: the tooltip is inside the window`).toBe(true);
      await page.mouse.move(0, 0);
      await noDefects(page, label);
    });
  });
});
