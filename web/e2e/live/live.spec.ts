// T3, in a real browser against the hub's own header code (the Rust fixture) and the harness's stream: one stream per tab and
// none before sign-in, the pill and the banner as the stream goes down and comes back, resuming with Last-Event-ID, and a live
// event that reaches the screen by refetching exactly what it names (P13).
//
// The frames the harness writes are byte-exact, in the hub's format (`id: N` with a space, a heartbeat, a `resync` with no
// id): see e2e/fixtures/sse-golden/README.md. THOSE FILES WERE WRITTEN BY HAND from crates/hub-platform/src/events/sse.rs by
// T1; they were not captured from a running hub, and neither are the frames built below.
import { readFileSync } from "node:fs";
import { expect, test } from "../fixtures/test";
import type { Harness } from "../harness/serve.ts";

const golden = (name: string): string => readFileSync(new URL(`../fixtures/sse-golden/${name}`, import.meta.url), "utf8");
const frame = (id: number | null, event: Record<string, unknown>): string => `${id === null ? "" : `id: ${String(id)}\n`}event: ${String(event["type"])}\ndata: ${JSON.stringify(event)}\n\n`;
const streams = (harness: Harness) => harness.requests().filter((r) => r.kind === "stream");
const asked = (harness: Harness, path: string) => harness.requests().filter((r) => r.kind !== "stream" && r.method === "GET" && r.path.split("?")[0] === path).length;
const pill = (page: import("@playwright/test").Page) => page.locator("[data-slot='live-indicator']");

test.describe("the stream of a signed-in user", () => {
  test("opens one, only after sign-in, with Accept: text/event-stream and no Last-Event-ID, and the pill says Live", async ({ page, harness }) => {
    await page.goto(`${harness.url}/swimlanes`);
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    await expect(pill(page)).toContainText("Live");
    await page.waitForTimeout(500);
    expect(streams(harness)).toHaveLength(1);
    expect(streams(harness)[0]).toMatchObject({ method: "GET", path: "/api/v1/events", status: 200, lastEventId: null, accept: "text/event-stream" });
    expect(harness.streamCount()).toBe(1);
  });

  for (const path of ["/signin", "/access-requested", "/access-disabled", "/404", "/403"]) {
    test(`opens none on ${path}, a screen before the session`, async ({ page, harness }) => {
      harness.setScenario("signed-out");
      await page.goto(`${harness.url}${path}`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await page.waitForTimeout(700);
      expect(streams(harness)).toHaveLength(0);
      expect(harness.streamCount()).toBe(0);
      await expect(pill(page)).toHaveCount(0);
    });
  }

  test("opens none for a user with no role yet, and the hub's own page for them shows no stream either", async ({ page, harness }) => {
    harness.setScenario("pending");
    await page.goto(`${harness.url}/swimlanes`);
    await expect(page).toHaveURL(/\/access-requested$/);
    await page.waitForTimeout(700);
    expect(streams(harness)).toHaveLength(0);
  });

  test("is closed when the person signs out", async ({ page, harness }) => {
    await page.goto(`${harness.url}/swimlanes`);
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    await page.getByRole("button", { name: "Sign out" }).click();
    await expect(page).toHaveURL(/\/signin$/);
    await expect.poll(() => harness.streamCount()).toBe(0);
    await page.waitForTimeout(500);
    expect(streams(harness)).toHaveLength(1);
  });

  test("goes to sign-in when the stream answers 401, like any other request, and does not ask again", async ({ page, harness }) => {
    harness.refuseStreams(401, 1);
    await page.goto(`${harness.url}/swimlanes`);
    await expect(page).toHaveURL(/\/signin\?next=%2Fswimlanes$/);
    await expect(page.getByRole("heading", { level: 1, name: "Sign in" })).toBeVisible();
    await page.waitForTimeout(800);
    expect(streams(harness)).toHaveLength(1);
  });
});

test.describe("losing the stream and getting it back", () => {
  test("resumes with the id of the last event after the hub closes the stream, and the pill says Reconnecting meanwhile", async ({ page, harness }) => {
    await page.goto(`${harness.url}/swimlanes`);
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    harness.emit(golden("heartbeat.txt") + golden("file-observed.txt") + golden("findings-changed.txt"));
    // Keep the next connection from opening at once, so that the pill can be seen between the two.
    harness.closeStreams();
    await expect(pill(page)).toHaveAttribute("data-status", "reconnecting");
    await expect(pill(page)).toContainText(/Reconnecting…/);
    // The hub told it to wait 3 s; with jitter the next connection is 3 to 6 s away.
    await expect.poll(() => streams(harness).length, { timeout: 15_000 }).toBe(2);
    expect(streams(harness)[1]?.lastEventId).toBe("42");
    await expect(pill(page)).toHaveAttribute("data-status", "live");
  });

  test("a heartbeat is not an event: with only heartbeats seen, the next connection has nothing to resume from", async ({ page, harness }) => {
    await page.goto(`${harness.url}/swimlanes`);
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    harness.emit(golden("heartbeat.txt") + golden("heartbeat.txt"));
    harness.closeStreams();
    await expect.poll(() => streams(harness).length, { timeout: 15_000 }).toBe(2);
    expect(streams(harness)[1]?.lastEventId).toBeNull();
  });

  test("a resync frame does not move the resume point (it has no id)", async ({ page, harness }) => {
    await page.goto(`${harness.url}/swimlanes`);
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    harness.emit(golden("file-observed.txt") + golden("resync.txt"));
    harness.closeStreams();
    await expect.poll(() => streams(harness).length, { timeout: 15_000 }).toBe(2);
    expect(streams(harness)[1]?.lastEventId).toBe("41");
  });

  test("goes Offline after three failures with the banner, Retry now tries at once, and it is Live again when the stream returns", async ({ page, harness }) => {
    test.setTimeout(120_000);
    await page.goto(`${harness.url}/swimlanes`);
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    harness.emit(golden("file-observed.txt"));
    harness.refuseStreams(503, 3);
    harness.closeStreams();
    await expect(pill(page)).toHaveAttribute("data-status", "offline", { timeout: 60_000 });
    await expect(pill(page)).toContainText(/Offline · data from \d\d:\d\d UTC/);
    const banner = page.getByText("Live updates paused, retrying");
    await expect(banner).toBeVisible();
    await expect(page.getByText(/Showing data from \d\d:\d\d UTC\. Saving and restarts are off until the connection returns\./)).toBeVisible();

    // Retry now makes an attempt at once, whatever the backoff chose (and the hub still says no).
    harness.refuseStreams(503, 1000);
    const before = streams(harness).length;
    await page.getByRole("button", { name: "Retry now" }).click();
    await expect.poll(() => streams(harness).length, { timeout: 3000 }).toBe(before + 1);

    // The stream returns: Live, the banner is gone, and the attempt resumed from the last id.
    harness.refuseStreams(503, 0);
    await page.getByRole("button", { name: "Retry now" }).click({ timeout: 20_000 }).catch(() => undefined);
    await expect(pill(page)).toHaveAttribute("data-status", "live", { timeout: 30_000 });
    await expect(banner).toHaveCount(0);
    expect(streams(harness).at(-1)?.lastEventId).toBe("41");
  });

  test("waits at least what Retry-After says on a 429 (a sixth tab), then connects", async ({ page, harness }) => {
    harness.refuseStreams(429, 1, { retryAfter: 4 });
    const start = Date.now();
    await page.goto(`${harness.url}/swimlanes`);
    await expect.poll(() => streams(harness).length).toBe(1);
    await expect(pill(page)).toHaveAttribute("data-status", "live", { timeout: 20_000 });
    expect(Date.now() - start).toBeGreaterThanOrEqual(3800);
    expect(streams(harness)).toHaveLength(2);
  });
});

test.describe("a live event reaches the screen by refetching what it names", () => {
  test.use({ dist: "dist-perf", scenario: "live" });

  // The scenario counts how often the list was asked for in the first row's finding count. The counter belongs to the module,
  // which a worker shares between tests, so a test reads where it starts and never assumes a number.
  const cell = (page: import("@playwright/test").Page) => page.locator("[role='row']", { hasText: "Lane 1" }).first().getByRole("gridcell").last();
  const count = async (page: import("@playwright/test").Page): Promise<number> => Number(await cell(page).textContent());

  /** Open the page, and wait until the list has been asked for twice: when it opened, and when the stream first connected. */
  async function open(page: import("@playwright/test").Page, harness: Harness): Promise<number> {
    await page.goto(`${harness.url}/testpages/perf/nav/a`);
    await expect(page.getByRole("heading", { level: 1, name: "Route A" })).toBeVisible();
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    await expect.poll(() => asked(harness, "/api/v1/swimlanes")).toBe(2);
    await page.waitForTimeout(300);
    harness.clearRequests();
    return count(page);
  }

  test("refetches the list on first connect (nothing to resume from) and then only for events that name it", async ({ page, harness }, info) => {
    await page.goto(`${harness.url}/testpages/perf/nav/a`);
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    // The list is asked for when the page opens, and once more when the stream first connects: a connection without an id is not resumed.
    await expect.poll(() => asked(harness, "/api/v1/swimlanes")).toBe(2);
    await page.waitForTimeout(500);
    expect(asked(harness, "/api/v1/swimlanes")).toBe(2);
    harness.clearRequests();
    const n = await count(page);

    // An event that names the list: one refetch, and the number on screen changes. P13 asks for this within a second.
    const t0 = Date.now();
    harness.emit(frame(100, { type: "findings_changed", swimlane: "lane-0001" }));
    await expect(cell(page)).toHaveText(String(n + 1));
    info.annotations.push({ type: "P13: emit to the screen (ms, informational)", description: String(Date.now() - t0) });
    expect(asked(harness, "/api/v1/swimlanes")).toBe(1);

    // Events that name nothing on this screen refetch nothing.
    harness.clearRequests();
    harness.emit(frame(101, { type: "doc_indexed", doc: 7 }) + frame(102, { type: "pr_state_changed", link_id: 5, state: "merged" }) + frame(103, { type: "access_request_created", user: "u1" }) + frame(104, { type: "pr_job_progress", job_id: 9, state: "preparing" }));
    await page.waitForTimeout(700);
    expect(harness.requests().filter((r) => r.kind !== "stream" && r.path.startsWith("/api/v1/"))).toEqual([]);
    await expect(cell(page)).toHaveText(String(n + 1));
  });

  test("a drift event names the list, once", async ({ page, harness }) => {
    const n = await open(page, harness);
    harness.emit(frame(110, { type: "drift_changed", swimlane: "lane-0002", paths: ["core/a.yml"] }));
    await expect(cell(page)).toHaveText(String(n + 1));
    await page.waitForTimeout(400);
    expect(asked(harness, "/api/v1/swimlanes")).toBe(1);
  });

  test("coalesces a burst: fifty events that name the list are one refetch", async ({ page, harness }) => {
    const n = await open(page, harness);
    const burst = Array.from({ length: 50 }, (_, i) => frame(200 + i, { type: "file_observed", swimlane: "lane-0001", path: `core/f${i}.yml`, hash: String(i).padStart(64, "0"), source: "scan" })).join("");
    harness.emit(burst);
    await expect(cell(page)).toHaveText(String(n + 1));
    await page.waitForTimeout(600);
    expect(asked(harness, "/api/v1/swimlanes")).toBe(1);
  });

  test("a resync refetches what is on screen once, the list and the signed-in user, and nothing that is not", async ({ page, harness }) => {
    const n = await open(page, harness);
    harness.emit(golden("resync.txt"));
    await expect(cell(page)).toHaveText(String(n + 1));
    await page.waitForTimeout(500);
    const api = harness.requests().filter((r) => r.kind !== "stream" && r.path.startsWith("/api/v1/"));
    expect(api.map((r) => r.path.split("?")[0]).sort()).toEqual(["/api/v1/me", "/api/v1/swimlanes"]);
  });

  test("an event split across two writes, and a frame with a CRLF line end, still arrive", async ({ page, harness }) => {
    const n = await open(page, harness);
    const text = frame(300, { type: "findings_changed", swimlane: "lane-0001" });
    harness.emit(text.slice(0, 25));
    await page.waitForTimeout(150);
    harness.emit(text.slice(25).replaceAll("\n", "\r\n"));
    await expect(cell(page)).toHaveText(String(n + 1));
  });

  test("keeps one stream across route changes", async ({ page, harness }) => {
    await page.goto(`${harness.url}/testpages/perf/nav/a`);
    await expect(pill(page)).toHaveAttribute("data-status", "live");
    for (const name of ["Page B", "Page C", "Page A", "Page B"]) {
      await page.getByRole("link", { name }).click();
      await page.waitForTimeout(120);
    }
    expect(streams(harness)).toHaveLength(1);
    expect(harness.streamCount()).toBe(1);
  });
});
