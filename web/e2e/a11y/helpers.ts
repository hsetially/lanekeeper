// Small things the accessibility and layout specs share: a typed case, holding or failing an API call so that a loading or an
// error state can be looked at, and the names of the page's landmarks.
import type { Locator, Page, Route } from "@playwright/test";
import type { CspWatch } from "../fixtures/test.ts";
import type { Harness } from "../harness/serve.ts";
import { FROZEN_NOW } from "../visual/render-references.mjs";

export type Scheme = "light" | "dark";
export type Ctx = { page: Page; harness: Harness; csp: CspWatch; scheme: Scheme };

/** One state of one screen, reached from a scenario, an address and (optionally) a few clicks. */
export type Case = {
  /** Unique, readable: "swimlanes / six / filter menu open". It is the test title. */
  id: string;
  scenario: string;
  url: string;
  /** Runs before the page opens: hold or fail an answer, set a cookie. */
  before?: (ctx: Ctx) => Promise<void>;
  /** Runs once the page has drawn its h1: brings the page into the state, and asserts that it is in it. */
  open?: (ctx: Ctx) => Promise<void>;
  /** The page is a dialog-free screen (the default) or one where the h1 is not the first thing (a public page). */
  noChrome?: boolean;
  /** For a state with no `h1` at all (the page behind a modal keeps its own, so this is for a page with none). */
  h1?: RegExp | string;
};

/** The open modal: a dialog or an alert dialog. */
export const modal = (page: Page) => page.getByRole("dialog").or(page.getByRole("alertdialog"));

export const SIDEBAR = (page: Page) => page.getByRole("complementary", { name: "Primary" });

/** Hold every request that matches until `release()`; the page shows its loading state meanwhile. */
export async function hold(page: Page, pattern: string | RegExp): Promise<{ release(): void; count(): number }> {
  let open: () => void = () => undefined;
  const gate = new Promise<void>((resolve) => {
    open = resolve;
  });
  let held = 0;
  await page.route(pattern, async (route: Route) => {
    held += 1;
    await gate;
    await route.continue().catch(() => undefined);
  });
  return { release: open, count: () => held };
}

/** Answer every request that matches with a problem+json (or any status), so that an error state can be looked at. */
export async function fail(page: Page, pattern: string | RegExp, status = 500, title = "Something went wrong on the server"): Promise<void> {
  await page.route(pattern, (route) =>
    route.fulfill({
      status,
      contentType: "application/problem+json",
      headers: { "x-request-id": "req-a11y-0001" },
      body: JSON.stringify({ type: "urn:lanekeeper:problem:internal", title, status, request_id: "req-a11y-0001" }),
    }),
  );
}

/**
 * Stand the page's `Date` still at the instant the scenarios derive their times from, so that "ends in 5 days" and "4 min ago" read as
 * drawn. Only `Date`: Playwright's `clock.setFixedTime` also replaces `performance.mark` and friends, and Monaco's input-latency probe
 * reads `performance.getEntriesByName(...)[0].duration` on every key, so a page that types into the editor throws under it. Call it
 * before the first navigation.
 */
export async function freezeDate(page: Page): Promise<void> {
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
 * What a control is described by, as text: the elements its `aria-describedby` names, in that order, read in one go (one state of
 * the page, not the attribute at one moment and the elements at another). An id with no element in the page reads as nothing.
 */
export function describedText(control: Locator): Promise<string> {
  return control.evaluate((el) => (el.getAttribute("aria-describedby") ?? "").split(/\s+/).map((id) => document.getElementById(id)?.textContent ?? "").join(" "));
}

/**
 * Make the tasks that React's scheduler posts for itself run late, as they do on a slow machine or in a busy tab: a render that is
 * not urgent (the follow-up of an effect, a refetched list) waits `ms` more, while everything else in the page (the commit of a click,
 * a layout effect) runs when it did. A page must not show one half of a change for that long, so a check that reads the page right
 * after a change and does not wait for it, can be run with this and no longer depends on the speed of the machine. Call it after the
 * page has loaded (it patches the port the scheduler posts through); it lasts until the page is left.
 */
export async function slowScheduler(page: Page, ms = 150): Promise<void> {
  await page.evaluate((delay) => {
    const port = MessagePort.prototype as unknown as { postMessage: (this: MessagePort, ...args: unknown[]) => void };
    const post = port.postMessage;
    port.postMessage = function slowPost(this: MessagePort, ...args: unknown[]): void {
      setTimeout(() => post.apply(this, args), delay);
    };
  }, ms);
}
