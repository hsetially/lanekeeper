// Getting the real app into the state of a frame, deterministically: the colour scheme and reduced motion pinned, the clock
// frozen at the instant the scenarios derive their times from (so "4 min ago" is the same every run), the fonts in, the first
// fetches answered, the pointer parked away from everything.
import type { Page } from "@playwright/test";
import { FROZEN_NOW } from "../../render-references.mjs";
import type { Harness } from "../../../harness/serve.ts";
import type { Theme } from "./reference";

export type Viewport = { width: number; height: number };
export type Prepare = (page: Page, harness: Harness) => Promise<void>;

/** The regions that are live by nature and are masked in every app-owned baseline: the connection pill. */
export const LIVE_REGIONS = ["[data-slot='live-indicator']"];

/**
 * How time is frozen. "fixed" is Playwright's clock (`Date.now` and `new Date()` stand still, timers run), which is what every
 * other spec uses. It also replaces `performance.mark` and friends, and Monaco's input-latency probe reads
 * `performance.getEntriesByName(...)[0].duration` on every key, so a page that types into the editor throws under it: such a page
 * freezes `Date` only ("date"), which is all the relative times of a scenario need.
 */
export type Clock = "fixed" | "date";

/** Pin what a screenshot depends on, before the first navigation. */
export async function pin(page: Page, theme: Theme, viewport: Viewport, clock: Clock = "fixed"): Promise<void> {
  await page.setViewportSize(viewport);
  await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
  if (clock === "fixed") {
    await page.clock.setFixedTime(FROZEN_NOW);
    return;
  }
  await page.addInitScript((fixed) => {
    const RealDate = Date;
    class FrozenDate extends RealDate {
      constructor(...args: unknown[]) {
        if (args.length === 0) super(fixed);
        else super(...(args as [number]));
      }
      static override now(): number {
        return fixed;
      }
    }
    window.Date = FrozenDate as unknown as DateConstructor;
  }, FROZEN_NOW);
}

/**
 * The page has drawn what its state shows: one h1 (every page renders it at once), no skeleton left unless the state IS the
 * skeleton, the connection pill live when the page has one, and the fonts are in. `park: false` keeps the pointer where a step put it.
 */
export async function settle(page: Page, { loading = false, live = true, park = true }: { loading?: boolean; live?: boolean; park?: boolean } = {}): Promise<void> {
  // By tag, not by role: a modal menu or dialog makes the page behind it inert for the accessibility tree.
  await page.locator("h1").first().waitFor({ state: "visible" });
  if (!loading) await page.waitForFunction(() => document.querySelector("[data-slot='skeleton']") === null);
  const pill = page.locator("[data-slot='live-indicator']");
  if (live && (await pill.count()) > 0) await pill.waitFor({ state: "visible" });
  if (live && (await pill.count()) > 0) await page.waitForFunction(() => document.querySelector("[data-slot='live-indicator']")?.getAttribute("data-status") === "live");
  await page.evaluate(() => document.fonts.ready);
  const size = page.viewportSize();
  // The pointer is parked in the corner, away from every hover state, unless the state IS a hover (a tooltip) and a step put it there.
  if (size && park) await page.mouse.move(size.width - 1, size.height - 1);
  // Let a row that fades or a tooltip that closes finish; animations are off, so this is only layout and image decode.
  await page.waitForTimeout(300);
}
