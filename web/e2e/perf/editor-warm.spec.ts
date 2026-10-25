// The editor's idle warm-up in a real browser (P10, W3): for an Editor and above the editor's files are FETCHED once the file
// view is up, so that Edit mode finds them in the HTTP cache. What the browser must show: the fetches happen, one per file and
// at low priority, for the roles that can edit and for no one else; nothing was evaluated (no Monaco global, no script tag,
// no worker); and a later load of the editor is answered from the cache instead of the network.
import type { CDPSession, Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";

const META = "lk-warm-editor";

type Seen = { url: string; type: string };

const files = (page: Page): Promise<string[]> => page.evaluate((name) => (document.querySelector(`meta[name="${name}"]`)?.getAttribute("content") ?? "").split(" ").filter(Boolean), META);

function watchRequests(page: Page): Seen[] {
  const seen: Seen[] = [];
  page.on("request", (request) => {
    seen.push({ url: new URL(request.url()).pathname, type: request.resourceType() });
  });
  return seen;
}

async function warmedBy(page: Page, timeout = 30_000): Promise<void> {
  await page.waitForFunction(() => performance.getEntriesByName("lk:editor-warm:done").length > 0, undefined, { timeout });
}

async function openGrid(page: Page, base: string): Promise<void> {
  await page.goto(`${base}/testpages/perf/grid`);
  await page.waitForFunction(() => window.__perf?.phase === "ready", undefined, { timeout: 60_000 });
}

test.describe("an Editor", () => {
  test.use({ dist: "dist-perf", scenario: "perf" });

  test("warms the editor's files once, by fetch, and evaluates none of them", async ({ page, harness }) => {
    const seen = watchRequests(page);
    // The priority the browser gave each request: `priority: "low"` is what keeps the warm-up behind the screen's own requests.
    const session = await page.context().newCDPSession(page);
    await session.send("Network.enable");
    const initial = new Map<string, string>();
    session.on("Network.requestWillBeSent", (e: { request: { url: string; initialPriority?: string } }) => {
      initial.set(new URL(e.request.url).pathname, e.request.initialPriority ?? "?");
    });
    await openGrid(page, harness.url);
    await warmedBy(page);

    const wanted = await files(page);
    expect(wanted.length, "the build named the editor's files").toBeGreaterThanOrEqual(2);
    expect(wanted.length).toBeLessThanOrEqual(8);
    for (const file of wanted) {
      const hits = seen.filter((s) => s.url === file);
      expect(hits, `${file} was asked for once`).toHaveLength(1);
      expect(hits[0]?.type, `${file} by fetch, not as a script, a style or a worker`).toBe("fetch");
    }
    for (const file of wanted) expect(initial.get(file), `${file} at low priority`).toBe("Low");

    // Fetched, not run: nothing of Monaco exists in the page, and no element points at its files.
    const state = await page.evaluate(
      (list) => ({
        monaco: "monaco" in window,
        environment: "MonacoEnvironment" in window,
        elements: [...document.querySelectorAll("script[src], link[href], iframe[src]")].map((e) => e.getAttribute("src") ?? e.getAttribute("href") ?? "").filter((u) => list.some((f) => u.endsWith(f))),
        workers: performance.getEntriesByType("resource").filter((e) => (e as PerformanceResourceTiming).initiatorType === "worker").length,
      }),
      wanted,
    );
    expect(state.monaco).toBe(false);
    expect(state.environment).toBe(false);
    expect(state.elements, "no <script>, <link> or <iframe> for an editor file").toEqual([]);
    expect(state.workers, "no worker was started").toBe(0);
  });

  test("a later load of the editor is served from the cache the warm-up filled", async ({ page, harness, context }) => {
    await openGrid(page, harness.url);
    await warmedBy(page);
    const wanted = await files(page);

    // A new page of the same browser context (so the same HTTP cache), watched at the network level.
    const lab = await context.newPage();
    const session: CDPSession = await context.newCDPSession(lab);
    await session.send("Network.enable");
    const urls = new Map<string, string>();
    const fromCache = new Set<string>();
    const asked = new Set<string>();
    session.on("Network.requestWillBeSent", (e: { requestId: string; request: { url: string } }) => {
      urls.set(e.requestId, new URL(e.request.url).pathname);
    });
    session.on("Network.requestServedFromCache", (e: { requestId: string }) => {
      const url = urls.get(e.requestId);
      if (url) fromCache.add(url);
    });
    session.on("Network.responseReceived", (e: { requestId: string; response: { fromDiskCache?: boolean; fromPrefetchCache?: boolean } }) => {
      const url = urls.get(e.requestId);
      if (!url) return;
      asked.add(url);
      if (e.response.fromDiskCache === true || e.response.fromPrefetchCache === true) fromCache.add(url);
    });
    await lab.goto(`${harness.url}/testpages/editor?k=editor`);
    await lab.waitForFunction(() => {
      const l = (window as unknown as { __lab?: { ready: boolean; error: string | null } }).__lab;
      return l !== undefined && (l.ready || l.error !== null);
    });
    expect(await lab.evaluate(() => (window as unknown as { __lab: { error: string | null } }).__lab.error), "the editor loaded").toBeNull();

    for (const file of wanted.filter((f) => f.endsWith(".js") && !/worker/.test(f))) {
      expect(fromCache.has(file), `${file} came from the cache`).toBe(true);
    }
    expect(wanted.some((f) => asked.has(f) || fromCache.has(f)), "the editor page used at least one of the warmed files").toBe(true);
  });
});

test.describe("a Viewer", () => {
  test.use({ dist: "dist-perf", scenario: "perf-viewer" });

  test("gets no warm-up: none of the editor's files is asked for, however long the page is idle", async ({ page, harness }) => {
    const seen = watchRequests(page);
    await openGrid(page, harness.url);
    const wanted = await files(page);
    expect(wanted.length, "the build still names the files (the page decides who gets them)").toBeGreaterThanOrEqual(2);
    // The warm-up waits for the browser to be idle, at most five seconds. Wait longer than that.
    await page.waitForTimeout(7000);
    for (const file of wanted) expect(seen.filter((s) => s.url === file), `${file} was not asked for`).toEqual([]);
    expect(await page.evaluate(() => performance.getEntriesByName("lk:editor-warm:start").length)).toBe(0);
  });
});
