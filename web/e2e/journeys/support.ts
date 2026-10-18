// What the journeys share. A journey is a short walk through one flow of a screen in a real browser, against the hub's own header
// code (the Rust fixture) and a mock scenario (src/mocks); it ends by saying the page was clean: no request the scenario could not
// answer, no write the hub's CSRF check would have refused, no console error, and no Content-Security-Policy or Trusted Types
// violation.
//
// The CSP listener is the harness's (e2e/fixtures/test.ts) and is never relaxed here: `finish` only reads what it collected.
import type { BrowserContext, Page } from "@playwright/test";
import { expect, pageViolations, test as base, type CspWatch } from "../fixtures/test";
import type { Harness, HarnessRequest } from "../harness/serve.ts";

export { derive, responseOf, responsesOf, bodyOf } from "./scenarios";
export type { MockContext, MockResponse } from "./scenarios";
export { expect } from "../fixtures/test";

/** The `test` of the journeys: the harness's, with its hub, its scenarios and its CSP watch. */
export const test = base;
/** Which build the specs serve: `dist`, or the folder `LK_E2E_DIST` names. Each spec file says `test.use({ dist: DIST })`. */
export const DIST = process.env["LK_E2E_DIST"] ?? "dist";

/** The clock of the design's data (the mocks date everything from it), fixed so that "in 5 days" and "2 min ago" mean what they say. */
export const DESIGN_NOW = 1_760_000_000_000;

/** Fix `Date.now()` at the design's clock. Timers still run. Call before the first navigation. */
export async function designClock(page: Page): Promise<void> {
  await page.clock.setFixedTime(new Date(DESIGN_NOW));
}

export const h1 = (page: Page) => page.getByRole("heading", { level: 1 });
export const sidebar = (page: Page) => page.getByRole("complementary", { name: "Primary" });

/** Open a path of the app and wait for the heading of the page (and the sidebar, for a page inside the shell). */
export async function open(page: Page, origin: string, path: string, heading: string | RegExp, options: { shell?: boolean } = {}): Promise<void> {
  await page.goto(`${origin}${path}`);
  await expect(page.getByRole("heading", { level: 1, name: heading })).toBeVisible();
  if (options.shell !== false) await expect(sidebar(page)).toBeVisible();
}

export async function allowClipboard(context: BrowserContext, origin: string): Promise<void> {
  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin });
}
export const clipboard = (page: Page): Promise<string> => page.evaluate(() => navigator.clipboard.readText());

/** The requests the hub saw that are not reads (and not the event stream), in order. */
export const writesOf = (harness: Harness): HarnessRequest[] => harness.requests().filter((r) => r.method !== "GET" && r.kind !== "stream");
export const callsTo = (harness: Harness, method: string, path: string | RegExp): HarnessRequest[] =>
  harness.requests().filter((r) => r.method === method && r.kind !== "stream" && (typeof path === "string" ? r.path.split("?")[0] === path : path.test(r.path)));

/** The status codes a console message names: the browser logs a refused request ("Failed to load resource ... status of 409"). */
const STATUS = /status of (\d{3})/;

/** The console errors of the page that are not an answer the journey asked for (`expected` lists the statuses it provoked). */
export function consoleErrors(csp: CspWatch, expected: readonly number[] = []): string[] {
  return csp.other
    .filter((line) => /^(?:worker )?error:/.test(line))
    .filter((line) => {
      const status = STATUS.exec(line)?.[1];
      return status === undefined || !expected.includes(Number(status));
    });
}

/**
 * The end of a journey: the page was clean. `expected` lists the HTTP statuses the journey provoked on purpose (a 409, a 423),
 * which the browser logs as console errors of their own; anything else it said is a failure.
 */
export async function finish(page: Page, csp: CspWatch, harness: Harness, options: { expected?: readonly number[]; unmocked?: readonly string[] } = {}): Promise<void> {
  expect(await pageViolations(page), "securitypolicyviolation events").toEqual([]);
  expect(csp.report(), "Content-Security-Policy and Trusted Types reports").toBe("");
  const requests = harness.requests();
  const unanswered = requests.filter((r) => r.unmocked === true && !(options.unmocked ?? []).includes(r.path.split("?")[0] ?? "")).map((r) => `${r.method} ${r.path}`);
  expect(unanswered, "requests the scenario had no answer for").toEqual([]);
  expect(consoleErrors(csp, options.expected), "console errors").toEqual([]);
  expect(
    requests.filter((r) => r.csrf === "refused").map((r) => `${r.method} ${r.path}`),
    "writes the CSRF check refused",
  ).toEqual([]);
}

/**
 * A secret must not be anywhere the page keeps or says things, after it has been used: its document, every storage, the cookies,
 * the console, and the address of every request (the body of the one request that carried it is the only place it ever was).
 */
export async function expectNoTrace(page: Page, csp: CspWatch, harness: Harness, secret: string): Promise<void> {
  const where = await page.evaluate(async (needle) => {
    const hits: string[] = [];
    if (document.documentElement.outerHTML.includes(needle)) hits.push("the document");
    if (document.body.innerText.includes(needle)) hits.push("the text of the page");
    for (const input of document.querySelectorAll("input, textarea")) if ((input as HTMLInputElement).value.includes(needle)) hits.push("a field");
    for (const [name, store] of [["localStorage", localStorage], ["sessionStorage", sessionStorage]] as const) {
      let all = "";
      for (let i = 0; i < store.length; i += 1) all += `${store.key(i) ?? ""}=${store.getItem(store.key(i) ?? "") ?? ""};`;
      if (all.includes(needle)) hits.push(name);
    }
    if (document.cookie.includes(needle)) hits.push("the cookies");
    const databases = await indexedDB.databases();
    if (JSON.stringify(databases).includes(needle)) hits.push("IndexedDB names");
    if (performance.getEntriesByType("resource").some((e) => e.name.includes(needle))) hits.push("a request address");
    return hits;
  }, secret);
  expect(where, "where the secret is").toEqual([]);
  expect(
    csp.other.filter((line) => line.includes(secret)),
    "the console",
  ).toEqual([]);
  expect(
    harness.requests().filter((r) => r.path.includes(secret)),
    "the address of a request",
  ).toEqual([]);
}
