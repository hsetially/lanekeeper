// What the keyboard specs of the screens share (the specs of the shell's own rows are keymap.spec.ts): the landmarks, getting a
// screen ready, and the guard that makes a test "keyboard only".
//
// `row(id)` marks a test as the one for a row of the keymap. It returns the id so a title can start with it. The coverage spec
// (coverage.spec.ts) reads the SOURCE of every spec in this folder for `row("...")` and fails when a row of the handoff's keymap
// is neither marked here nor in its reviewed list of rows that do not apply, so a title that names a row is a claim the build
// checks, and a row that the keymap gains has to be tested or explained.
import type { Locator, Page } from "@playwright/test";
import { expect, test as base } from "../fixtures/test";

/** The `test` of these specs: the build in `dist`, or the one `LK_E2E_DIST` names (a build made on the side). */
export const test = base;
/** Which build the specs serve: `dist`, or the folder `LK_E2E_DIST` names. Each spec file says `test.use({ dist: DIST })`. */
export const DIST = process.env["LK_E2E_DIST"] ?? "dist";
export { expect };

export const row = (id: string): string => id;

export const sidebar = (page: Page) => page.getByRole("complementary", { name: "Primary" });
export const h1 = (page: Page) => page.getByRole("heading", { level: 1 });
export const pathOf = (page: Page): string => new URL(page.url()).pathname;
export const searchOf = (page: Page): string => new URL(page.url()).search;

/** Open a screen and wait for its title and the sidebar, so that the keyboard has something to work on. */
export async function ready(page: Page, origin: string, path: string, name?: string | RegExp): Promise<void> {
  await page.goto(`${origin}${path}`);
  await expect(name === undefined ? h1(page) : page.getByRole("heading", { level: 1, name })).toBeVisible();
  await expect(sidebar(page)).toBeVisible();
}

/**
 * Wait until a table has a row of data (its header is the first row). A key pressed before that finds a list with nothing to move
 * through and is lost, which only shows when the machine is slow; so every test that presses a key on a table waits here first.
 */
export async function loaded(table: Locator): Promise<void> {
  await expect(table.getByRole("row").nth(1)).toBeVisible();
}

/** The element that has focus, described so that a failure says where focus went. */
export function focused(page: Page): Promise<string> {
  return page.evaluate(() => {
    const el = document.activeElement;
    if (el === null || el === document.body) return "body";
    const name = el.getAttribute("aria-label") ?? el.textContent?.trim().slice(0, 40) ?? "";
    return `${el.tagName.toLowerCase()}${el.getAttribute("role") ? `[role=${el.getAttribute("role")}]` : ""} "${name}"`;
  });
}

export type PointerLog = () => Promise<string[]>;

/**
 * Make a test keyboard only. Every pointer event that reaches the page (a click, a press, a touch) is recorded, and the returned
 * function lists them: a test that claims to be keyboard only asserts that the list is empty. Call it before the first
 * navigation. Events from the page's own code (`el.click()`) are not trusted and are not recorded; only the browser's are.
 */
export async function keyboardOnly(page: Page): Promise<PointerLog> {
  await page.addInitScript(() => {
    const w = window as unknown as { __lkPointer?: string[] };
    w.__lkPointer = [];
    for (const type of ["pointerdown", "mousedown", "click", "touchstart"]) {
      window.addEventListener(
        type,
        (event) => {
          // A click that a key made (Enter or Space on a button) has no click count; a mouse click has at least one.
          const byKey = type === "click" && (event as MouseEvent).detail === 0;
          if (event.isTrusted && !byKey) w.__lkPointer?.push(`${type} on ${(event.target as Element | null)?.tagName ?? "?"}`);
        },
        true,
      );
    }
  });
  return () => page.evaluate(() => (window as unknown as { __lkPointer?: string[] }).__lkPointer ?? []);
}
