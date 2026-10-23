// What the T6 journeys and keyboard specs (pickup chips, notify and restarts, "As served") share, on top of ./support, ./scenarios
// and ./t5: holding one request in the browser so that a dialog can be looked at while the write is out, a restart answered
// differently on each request of a run, the swimlane as another role sees it, and the answers a journey changes in the middle of
// its walk.
//
// Nothing here is a hub of its own and nothing is registered outside the test process: scenarios are data (src/mocks/scenario.ts)
// served by the harness (e2e/harness/serve.ts) behind the Rust fixture's headers and CSP.
import type { Page, Route } from "@playwright/test";
import type { CspWatch } from "../fixtures/test";
import type { FileEntry, Role, ServedFile, ServiceRef, ServiceState, SwimlaneDetail, SwimlaneSummary } from "../../src/api/generated/model/index.ts";
import { meBody } from "../../src/mocks/me.ts";
import { problemBody } from "../../src/mocks/scenario.ts";
import { PRIYA } from "../../src/mocks/design/file/data.ts";
import type { Harness } from "../harness/serve.ts";
import { expect } from "../fixtures/test";
import { bodyOf, derive, responseOf, type MockContext, type MockResponse } from "./scenarios";

export { asRole, emit, goOffline, live, pill } from "./t5";
import { pill as pillOf } from "./t5";
export { MARCO, PRIYA } from "./t5";

export const SWIMLANE_API = "/api/v1/swimlanes/{id}";
export const NOTIFY_PATH = "/api/v1/swimlanes/sitb/notify";
export const RESTART_PATH = "/api/v1/swimlanes/sitb/restart";
export const RESTART_OFFER_PATH = "/api/v1/swimlanes/sitb/restart-offer";
export const CONFIG_SERVER_OFFER_PATH = "/api/v1/swimlanes/sitb/config-server-restart";
export const SERVED_API = "/api/v1/swimlanes/sitb/served";

export const SERVICES_TAB = "/swimlanes/sitb/services";
export const CORE_FILE = "tx-infinity-api/tx-infinity-core-sit7.yml";
export const CORE_FILE_PAGE = `/swimlanes/sitb/files?path=${encodeURIComponent(CORE_FILE)}`;
export const SERVED_FILE = "tx-infinity-api/connection-pool.yml";
/** The address the hub's `get_served_file` tool sends people to (`LinkTemplates::default().served`, crates/hub-mcp/src/output/links.rs). */
export const MCP_SERVED_LINK = `/swimlanes/sitb/served?application=tx-infinity-core&file=${encodeURIComponent(SERVED_FILE)}&tenant=sit7`;

export const WHO = { id: PRIYA, name: "Ada Lovelace", email: "ada.lovelace@example.test" } as const;

// -- A request held in the browser --------------------------------------------------------------------------------------------

export type Held = {
  /** Resolves when the held request has left the page (the hub has NOT seen it yet). */
  readonly reached: Promise<void>;
  /** How many requests of this method and address the page has made, held or not. */
  count(): number;
  /** Let the held request go to the hub. */
  release(): Promise<void>;
};

/**
 * Hold the `nth` request (the first by default) the page makes with this method to an address: it waits in the browser, so the
 * dialog that sent it can be looked at while the answer is out. The ones before it go through. The hub does not see a held
 * request until it is released, so `harness.requests()` is what the hub was asked, not what the page tried.
 */
export async function holdRequest(page: Page, address: RegExp, options: { method?: string; nth?: number } = {}): Promise<Held> {
  const method = options.method ?? "POST";
  const nth = options.nth ?? 1;
  let seen = 0;
  let held: Route | null = null;
  let reach: () => void = () => undefined;
  const reached = new Promise<void>((resolve) => {
    reach = resolve;
  });
  await page.route(address, async (route) => {
    if (route.request().method() !== method) {
      await route.fallback();
      return;
    }
    seen += 1;
    if (seen !== nth) {
      await route.fallback();
      return;
    }
    held = route;
    reach();
  });
  return {
    reached,
    count: () => seen,
    release: async () => {
      const route = held;
      held = null;
      if (route !== null) await route.fallback();
    },
  };
}

// -- Answers that differ from one request to the next -------------------------------------------------------------------------

export type Answer = { readonly status: number; readonly body: unknown };

/**
 * A `POST` response whose answer follows the request count: the first request gets the first answer, the second the second, and
 * the last answer repeats. The status is read once for each request, after the body (src/mocks/scenario.ts), so the two stay in step.
 */
export function inSequence(path: string, answers: readonly Answer[]): MockResponse {
  let at = 0;
  const current = (): Answer => {
    const answer = answers[Math.min(at, answers.length - 1)];
    if (answer === undefined) throw new Error("a sequence needs at least one answer");
    return answer;
  };
  return {
    method: "POST",
    path,
    sample: {},
    body: () => current().body,
    get status(): number {
      const status = current().status;
      at += 1;
      return status;
    },
  };
}

export const APPLIED: Answer = { status: 200, body: { outcome: "applied", hash: null, audit_id: 77 } };
export const LOCKED: Answer = { status: 423, body: problemBody(423, "locked", "Locked", { until: null }) };
export const UNAVAILABLE: Answer = { status: 503, body: problemBody(503, "service_unavailable", "Service unavailable") };

/** The hub's restart-all offer with these services, in the swimlane's own namespace. */
export function offerOf(names: readonly string[]): { services: ServiceRef[]; truncated: false } {
  return { services: names.map((name) => ({ namespace: "sitb", name })), truncated: false };
}

/** The same scenario with another restart-all offer and these answers to the restarts that follow, one for each request. */
export function restartAll(base: string, names: readonly string[], answers: readonly Answer[]): string {
  const offer = responseOf(base, "GET", RESTART_OFFER_PATH.replace("sitb", "{id}"));
  return derive(base, [{ ...offer, body: offerOf(names) }, inSequence(`${SWIMLANE_API}/restart`, answers)], "journey-restart-all");
}

// -- The signed-in person -----------------------------------------------------------------------------------------------------

/** The same scenario for a person with this role, optionally one whose writes need approval. */
export function signedInAs(base: string, role: Role, over: { requires_approval?: boolean } = {}): string {
  const me = responseOf(base, "GET", "/api/v1/me");
  return derive(base, [{ ...me, body: meBody(role, { id: WHO.id, email: WHO.email, display_name: WHO.name, ...over }) }], `journey-as-${role}`);
}

// -- The swimlane, changed in the middle of a walk ----------------------------------------------------------------------------

/** What to do to the services of sitb. Return the service as it should be (a new object), or the same one to leave it alone. */
export type ServiceChange = (service: ServiceState) => ServiceState;

/**
 * The scenario with the swimlane's services changed, and its tree entries changed too, the way the hub answers once services
 * have picked a change up. `counts` replaces `pickup_counts` of the detail. The change is made on the answers of `base`, so
 * everything else on the screen is the base's.
 */
export function afterPickup(base: string, options: { service?: ServiceChange; entry?: (entry: FileEntry) => FileEntry; counts?: SwimlaneDetail["pickup_counts"]; lane?: (lane: SwimlaneSummary) => SwimlaneSummary }): string {
  const detail = responseOf(base, "GET", SWIMLANE_API);
  const tree = responseOf(base, "GET", `${SWIMLANE_API}/tree`);
  const list = responseOf(base, "GET", "/api/v1/swimlanes");
  const changes: MockResponse[] = [];
  const { service, entry, counts, lane } = options;
  if (lane !== undefined) {
    changes.push({
      ...list,
      body: (context: MockContext) => {
        const before = bodyOf(list, context) as { items: SwimlaneSummary[]; next: string | null };
        return { ...before, items: before.items.map(lane) };
      },
    });
  }
  if (service !== undefined || counts !== undefined) {
    changes.push({
      ...detail,
      body: (context: MockContext) => {
        const before = bodyOf(detail, context) as SwimlaneDetail;
        return { ...before, ...(service === undefined ? {} : { services: before.services.map(service) }), ...(counts === undefined ? {} : { pickup_counts: counts }) };
      },
    });
  }
  if (entry !== undefined) {
    changes.push({
      ...tree,
      body: (context: MockContext) => {
        const before = bodyOf(tree, context) as { items: FileEntry[]; next: string | null };
        return { ...before, items: before.items.map(entry) };
      },
    });
  }
  return derive(base, changes, "journey-pickup-after");
}

// -- Reading the screen -------------------------------------------------------------------------------------------------------

/** The row of a service in the Services tab's table. */
export const serviceRow = (page: Page, name: string) => page.getByRole("table", { name: "Services" }).getByRole("row").filter({ has: page.getByRole("cell", { name, exact: true }) });

/** The pickup chip inside a locator, with the state it stands for. */
export const chipOf = (scope: ReturnType<typeof serviceRow>) => scope.locator("[data-slot='pickup-chip']");

/** The calls the hub saw, as `METHOD path`, without the reads that every page makes. */
export const hubWrites = (harness: Harness): string[] => harness.requests().filter((r) => r.method !== "GET" && r.kind !== "stream").map((r) => `${r.method} ${r.path.split("?")[0] ?? ""}`);

/** Wait until the page has made no further request for a while (a retry or a second ask would show). */
export async function quiet(page: Page, ms = 1_200): Promise<void> {
  await page.waitForTimeout(ms);
}

/**
 * Wait until the hub has seen no new request for a moment. The first connection of the event stream has no id to resume from, so the
 * live layer reads every query on screen again once (a resync, src/live/sse-client.ts): a journey that changes the hub's answers or
 * counts its requests waits for that first, or the resync is what it measures.
 */
export async function settled(harness: Harness, quietMs = 500, timeout = 8_000): Promise<void> {
  let last = -1;
  let since = Date.now();
  await expect
    .poll(
      () => {
        const n = harness.requests().length;
        if (n !== last) {
          last = n;
          since = Date.now();
          return false;
        }
        return Date.now() - since >= quietMs;
      },
      { timeout, intervals: [100] },
    )
    .toBe(true);
}

export { expect };

/**
 * Put the event stream out of order WITHOUT a fake clock: the next attempts are refused and the open stream is closed, and the page
 * goes offline when its back-off has run out (a few seconds). For a journey that has the editor on screen: Monaco's animations read
 * the page's clock, and a fake one makes it throw (an uncaught error that is the test's doing, not the app's).
 */
export async function goOfflineInRealTime(page: Page, harness: Harness): Promise<void> {
  harness.refuseStreams(503, 1000);
  harness.closeStreams();
  await expect(pillOf(page)).toHaveAttribute("data-status", "offline", { timeout: 40_000 });
}

// -- "As served" --------------------------------------------------------------------------------------------------------------

/**
 * The scenario `base` (one of `design-served-*`) with another answer for the served file: `make` gets the query the page sent and
 * returns the file the hub would send for it. The hub echoes the application, tenant, channel and file it was asked for; `make` is
 * given them so that it can do the same.
 */
export function servedAnswer(base: string, make: (query: URLSearchParams) => ServedFile, status = 200): string {
  const served = responseOf(base, "GET", `${SWIMLANE_API}/served`);
  return derive(base, [{ ...served, status, body: (context: MockContext) => make(context.query) }], "journey-served");
}

/** What the hub echoes of a request: the four things the page asked for. */
export function echoed(query: URLSearchParams, file: ServedFile): ServedFile {
  return { ...file, application: query.get("application") ?? file.application, tenant: query.get("tenant") ?? file.tenant, channel: query.get("channel"), file: query.get("file") ?? file.file };
}

/** The requests for the served file the hub has seen, as `application/tenant/channel file`. */
export const servedCalls = (harness: Harness) =>
  harness
    .requests()
    .filter((r) => r.method === "GET" && r.path.split("?")[0] === SERVED_API)
    .map((r) => {
      const q = new URL(r.path, "http://hub.test").searchParams;
      return { status: r.status, application: q.get("application"), tenant: q.get("tenant"), channel: q.get("channel"), file: q.get("file") };
    });

/**
 * A journey that drops connections on purpose (`route.abort`) provokes one console error for each, which the browser words without a
 * status. This takes exactly that many of them out of the console log before `finish` reads it, and fails when there were more or fewer:
 * any other console error, and any policy report, is still a failure.
 */
export function acceptDroppedConnections(csp: CspWatch, count: number): void {
  const dropped = csp.other.filter((line) => /^error: Failed to load resource: net::ERR_CONNECTION_RESET$/.test(line));
  if (dropped.length !== count) throw new Error(`expected ${String(count)} dropped connections in the console, found ${String(dropped.length)}`);
  for (const line of dropped) csp.other.splice(csp.other.indexOf(line), 1);
}
