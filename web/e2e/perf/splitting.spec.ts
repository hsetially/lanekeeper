// Code splitting in a real browser, behind the hub's header code and the production CSP (P10, S12). What the unit tests cannot
// show: that a dynamic import of a chunk is allowed by the policy, that the first visit fetches the shell and the one screen
// it needs and nothing more (no editor), that the files are the immutable assets the hub marks them as, and what the screen
// of a deploy that took a chunk away looks like, without the page reloading by itself (an unsaved edit is not thrown away).
import { cpSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import { startHarness } from "../harness/serve.ts";
import { expect, test } from "../fixtures/test";
import { settleLive } from "./helpers";

const WEB_ROOT = fileURLToPath(new URL("../../", import.meta.url));
const BUDGET_KB = 250;

type Script = { path: string; status: number; cacheControl: string | undefined; gzipBytes: number };

/** Every script the page fetches, with its size gzipped (what a hub that compressed would send). */
function collectScripts(page: import("@playwright/test").Page): { scripts: Script[]; settled: () => Promise<void> } {
  const scripts: Script[] = [];
  const pending: Array<Promise<void>> = [];
  page.on("response", (response) => {
    const url = new URL(response.url());
    if (!url.pathname.endsWith(".js")) return;
    pending.push(
      response
        .body()
        .then((body) => {
          scripts.push({ path: url.pathname, status: response.status(), cacheControl: response.headers()["cache-control"], gzipBytes: gzipSync(body).length });
        })
        .catch(() => {
          scripts.push({ path: url.pathname, status: response.status(), cacheControl: response.headers()["cache-control"], gzipBytes: 0 });
        }),
    );
  });
  return {
    scripts,
    settled: async () => {
      await Promise.all(pending);
    },
  };
}

// P10 budgets the INITIAL JavaScript: what a first visit needs before the person can use the page. What the browser fetches
// afterwards, when it has nothing else to do (the other screens of the sidebar, the command palette), is its own business and is
// checked in the next test; it is not in the budget, and it grows with every screen that is built.
test("the first visit needs the shell, the one screen and the live engine: immutable files, no editor, under the budget", async ({ page, harness }) => {
  harness.setScenario("viewer");
  // The browser is never idle here, so the idle work (src/routes/prefetch.ts, the palette) does not run.
  await page.addInitScript(() => {
    Object.defineProperty(window, "requestIdleCallback", { configurable: true, value: () => 0 });
  });
  const { scripts, settled } = collectScripts(page);
  await page.goto(`${harness.url}/swimlanes`);
  await expect(page.getByRole("heading", { level: 1, name: "Swimlanes" })).toBeVisible();
  await settleLive(page, harness);
  await settled();

  const assets = scripts.filter((s) => s.path.startsWith("/assets/"));
  const names = assets.map((s) => s.path.replace(/^\/assets\//, "").replace(/-[A-Za-z0-9_-]{8}\.js$/, ""));
  expect(names).toContain("index");
  expect(names).toContain("SwimlanesPage");
  expect(names).toContain("live");
  for (const name of names) {
    expect(name, "the editor is not part of a first visit").not.toMatch(/editor|monaco|jsonMode|worker/i);
    expect(name, "no other screen is part of a first visit").not.toMatch(/^(ComparePage|StatusPage|FindingsPage|ApprovalsPage|ApprovalPage|AuditPage|UsersPage|SwimlanesAdminPage|SectionPage|FilePage|SearchPage)$/);
    expect(name, "the command palette waits for an idle moment").not.toMatch(/^CommandPalette$/);
  }
  for (const script of assets) {
    expect(script.status, script.path).toBe(200);
    expect(script.cacheControl, script.path).toMatch(/immutable/);
  }
  const kb = assets.reduce((n, s) => n + s.gzipBytes, 0) / 1024;
  expect(kb, `the scripts of a first visit, gzipped: ${kb.toFixed(1)} KB`).toBeLessThan(BUDGET_KB);
});

test("when the browser is idle it fetches the sidebar's other screens and the palette: only the ones the role can open, never the editor", async ({ page, harness }) => {
  harness.setScenario("viewer");
  const { scripts, settled } = collectScripts(page);
  await page.goto(`${harness.url}/swimlanes`);
  await expect(page.getByRole("heading", { level: 1, name: "Swimlanes" })).toBeVisible();
  // A signed-in page keeps a live stream open, so the network is never idle: wait for the stream and for what it caused instead.
  await settleLive(page, harness);
  await settled();

  const assets = scripts.filter((s) => s.path.startsWith("/assets/"));
  const names = assets.map((s) => s.path.replace(/^\/assets\//, "").replace(/-[A-Za-z0-9_-]{8}\.js$/, ""));
  // The shell and its screen: the shell (`index`), the screen's own chunk, and the live engine (the SSE client, fetched once a
  // user with a role is signed in, src/live/LiveProvider.tsx, in a chunk of its own).
  expect(names).toContain("index");
  expect(names).toContain("SwimlanesPage");
  expect(names).toContain("live");
  // What the idle moment adds (a Viewer's sidebar destinations, src/routes/prefetch.ts, and the command palette, which the shell
  // loads when idle so that Command-K opens at once): screens a Viewer can open, never one it cannot, and never the editor.
  const SCREEN_CHUNK = /Page$/;
  const NOT_FOR_A_VIEWER = /^(ApprovalsPage|ApprovalPage|AuditPage|UsersPage|SwimlanesAdminPage|SectionPage)$/;
  for (const name of names) {
    expect(name, "the editor is not part of a first visit").not.toMatch(/editor|monaco|jsonMode|worker/i);
    expect(name, "a Viewer never fetches an Admin screen").not.toMatch(NOT_FOR_A_VIEWER);
  }
  expect(
    names.filter((n) => SCREEN_CHUNK.test(n) && !["SwimlanesPage", "ComparePage", "StatusPage", "FindingsPage"].includes(n)),
    "no screen outside the sidebar's destinations",
  ).toEqual([]);

  for (const script of assets) {
    expect(script.status, script.path).toBe(200);
    expect(script.cacheControl, script.path).toMatch(/immutable/);
  }
  // The idle loads are a few hundred small files, all of them lazy: they are reported, not budgeted (P10 budgets the initial load,
  // which the test above holds to 250 KB). Reading the number off the report is how a chunk that grew out of hand gets noticed.
  const kb = assets.reduce((n, s) => n + s.gzipBytes, 0) / 1024;
  test.info().annotations.push({ type: "gzipped KB of the first visit with its idle loads", description: kb.toFixed(1) });
  expect(names, "the idle moment did load the sidebar's other screens").toEqual(expect.arrayContaining(["ComparePage", "StatusPage", "FindingsPage", "CommandPalette"]));
});

test("a screen before the session loads no live engine and opens no stream", async ({ page, harness }) => {
  harness.setScenario("signed-out");
  const { scripts, settled } = collectScripts(page);
  await page.goto(`${harness.url}/signin`);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await page.waitForLoadState("networkidle");
  await settled();
  const names = scripts.filter((s) => s.path.startsWith("/assets/")).map((s) => s.path.replace(/^\/assets\//, "").replace(/-[A-Za-z0-9_-]{8}\.js$/, ""));
  expect(names, "the shell is there").toContain("index");
  expect(names, "the engine is not fetched for someone who is not signed in").not.toContain("live");
  expect(harness.requests().filter((r) => r.kind === "stream"), "and no stream is asked for").toEqual([]);
});

test.describe("on the perf build's route pages (three screens, a chunk each)", () => {
  test.use({ dist: "dist-perf", scenario: "perf" });

  test("a second screen is its own chunk, fetched when it is first used, over the same page", async ({ page, harness }) => {
    const { scripts, settled } = collectScripts(page);
    let documents = 0;
    page.on("request", (request) => {
      if (request.resourceType() === "document") documents += 1;
    });
    await page.goto(`${harness.url}/testpages/perf/nav/a`);
    await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
    await settleLive(page, harness);
    await settled();
    const has = (name: string) => scripts.some((s) => new RegExp(`/assets/${name}-[A-Za-z0-9_-]{8}\\.js$`).test(s.path));
    expect(has("RouteA"), "the first screen's chunk").toBe(true);
    expect(has("RouteB"), "the second screen is not fetched before it is wanted").toBe(false);
    expect(has("RouteC")).toBe(false);

    await page.getByRole("link", { name: "Page B" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Route B" })).toBeVisible();
    await settled();
    expect(has("RouteB"), "the second screen's chunk was fetched when it was used").toBe(true);
    expect(has("RouteC"), "and not the third").toBe(false);
    expect(documents, "one page load, then client-side navigation").toBe(1);
  });
});

test("a chunk that a deploy took away shows the new-version screen, keeps the page, and never reloads by itself", async ({ page }) => {
  // A copy of the production build with the screen's chunk deleted: what a tab opened before a deploy sees when it asks
  // for a file the new build no longer has.
  const copy = mkdtempSync(join(tmpdir(), "lk-stale-dist-"));
  const stale = await (async () => {
    cpSync(resolve(WEB_ROOT, "dist"), copy, { recursive: true });
    const gone = readdirSync(join(copy, "assets")).filter((name) => /^SwimlanesPage-.*\.js$/.test(name));
    expect(gone, "the build has the screen chunk to remove").toHaveLength(1);
    for (const name of gone) rmSync(join(copy, "assets", name));
    return startHarness({ dist: copy, scenario: "viewer" });
  })();
  try {
    let documents = 0;
    page.on("request", (request) => {
      if (request.resourceType() === "document") documents += 1;
    });
    await page.goto(`${stale.url}/swimlanes`);
    await expect(page.getByRole("heading", { level: 1, name: "A new version is available" })).toBeVisible();
    const reload = page.getByRole("button", { name: "Reload" });
    await expect(reload).toBeVisible();
    // One "Reload", not one in a banner and another in the screen.
    await expect(reload).toHaveCount(1);
    // The page is where it was. Nothing reloaded it: not the failure, not a retry, not a timer.
    await page.waitForTimeout(1500);
    expect(documents, "the page was loaded once and not again").toBe(1);
    expect(await page.evaluate(() => window.sessionStorage.length), "no reload marker was left in sessionStorage").toBe(0);
    await expect(page).toHaveURL(`${stale.url}/swimlanes`);
  } finally {
    await stale.close();
    rmSync(copy, { recursive: true, force: true });
  }
});
