// What the T5 journeys (pull requests, attribution, masking, the admin sections) share, on top of ./support and ./scenarios:
// the frames the hub's event stream writes, a way to put the stream out of order quickly (a fake clock), a scenario registered
// from a whole `Scenario` (not only a list of changed responses), and the hub of the pull-request journeys, which remembers the
// state of one pull-request job so that a journey can move it along and say what the server says.
//
// Nothing here is a hub of its own and nothing is registered outside the test process: it is data (src/mocks/scenario.ts) served
// by the harness (e2e/harness/serve.ts), behind the Rust fixture's headers and CSP.
import type { Page } from "@playwright/test";
import type { Attribution, FileEntry, Finding, Mask, PrJob, PrJobState, PrLink, Role, Severity, Version } from "../../src/api/generated/model/index.ts";
import { BY_SOURCE, USERS, historyVersions } from "../../src/mocks/design/attribution/data.ts";
import { fileScenario } from "../../src/mocks/design/file/build.ts";
import { CORE_PATH, H_NFS, LANE, MARCO, NOW, PRIYA, coreFile, coreLines, joinLines, lineStart, prJob, type FileKit } from "../../src/mocks/design/file/data.ts";
import { SECRET_SETTING } from "../../src/mocks/design/masking/data.ts";
import { findingPage } from "../../src/mocks/design/findings/server.ts";
import { fileEntry, finding } from "../../src/mocks/design/swimlanes/fixtures.ts";
import { meBody } from "../../src/mocks/me.ts";
import { scenarios } from "../../src/mocks/scenarios.ts";
import type { MockContext, Scenario } from "../../src/mocks/scenario.ts";
import type { Harness } from "../harness/serve.ts";
import { expect } from "../fixtures/test";
import { derive, responseOf, type MockResponse } from "./scenarios";

let counter = 0;

/** Register a whole scenario under a fresh id (a journey builds scenarios from the design builders, not only from a base). */
export function register(scenario: Omit<Scenario, "id">, label = "journey-t5"): string {
  const id = `${label}-${String((counter += 1))}`;
  scenarios[id] = { ...scenario, id };
  return id;
}

/** One frame of the hub's stream, in the hub's format (`id: N`, the event's name and its JSON). */
export const frame = (id: number, event: Record<string, unknown>): string => `id: ${String(id)}\nevent: ${String(event["type"])}\ndata: ${JSON.stringify(event)}\n\n`;

let eventId = 9000;
/** Write one event to every open stream. */
export function emit(harness: Harness, event: Record<string, unknown>): void {
  harness.emit(frame((eventId += 1), event));
}

export const pill = (page: Page) => page.locator("[data-slot='live-indicator']");

/** Wait for the stream to be open, so that an event written next reaches the page. */
export async function live(page: Page): Promise<void> {
  await expect(pill(page)).toHaveAttribute("data-status", "live");
}

/**
 * Put the stream out of order, as a hub that is down does: the next attempts are refused (503), the open stream is closed, and the
 * clock is moved on until the pill says Offline (three failures in a row, with a back-off of seconds). The page must have been
 * opened after `page.clock.install()`, which the journey calls before its first navigation; time still runs by itself, and this
 * only brings the retries forward.
 */
export async function goOffline(page: Page, harness: Harness): Promise<void> {
  harness.refuseStreams(503, 1000);
  harness.closeStreams();
  await expect
    .poll(
      async () => {
        await page.clock.runFor(7_000);
        return pill(page).getAttribute("data-status");
      },
      { timeout: 40_000, intervals: [250] },
    )
    .toBe("offline");
}

// -- The pull-request job ----------------------------------------------------------------------------------------------------

export const JOB_ID = 7;
export const PR_NUMBER = 419;
export const PR_ADDRESS = `https://github.com/csp/csp-tenant-data/pull/${String(PR_NUMBER)}`;

/** The link the hub lists once the job has opened the pull request: this file, the content the person looked at (`H_NFS`), open. */
export const NEW_LINK: PrLink = { id: 15, swimlane: LANE, path: CORE_PATH, nfs_hash: H_NFS, state: "open", pr_url: PR_ADDRESS, pr_number: PR_NUMBER, author: PRIYA, created_at: NOW + 4_000, closed_at: null };

export type PrHub = {
  readonly scenario: string;
  /** What the server says the job is now. */
  job: PrJobState;
  /** The links the hub lists, whatever state they are in; the job's own link is added when it is completed. */
  readonly extra: PrLink[];
  /** How many times the job was read. */
  reads(): number;
};

/**
 * A hub for the pull-request journeys: the File page of the design with a job whose state the journey moves (`hub.job = "committing"`),
 * and a list of pull requests that has the new one only when the job is completed, as the real hub's has it (the link is made when
 * the pull request is). Everything else is the design scenario's.
 */
export function prHub(options: { base?: string; job?: PrJobState; extra?: PrLink[] } = {}): PrHub {
  const base = options.base ?? "design-file-pr";
  let reads = 0;
  const hub: PrHub = { scenario: "", job: options.job ?? "pending", extra: options.extra ?? [], reads: () => reads };
  const changes: MockResponse[] = [
    {
      method: "GET",
      path: "/api/v1/prs",
      sample: { query: new URLSearchParams({ swimlane: LANE, state: "open", limit: "100" }) },
      samples: [{ query: new URLSearchParams({ swimlane: LANE, state: "merged", limit: "100" }) }],
      body: ({ query }: MockContext) => {
        const all = [...hub.extra, ...(hub.job === "completed" ? [NEW_LINK] : [])];
        return { items: all.filter((l) => (query.get("state") === null || l.state === query.get("state")) && (query.get("swimlane") === null || l.swimlane === query.get("swimlane"))).sort((a, b) => b.id - a.id), next: null };
      },
    },
    { method: "POST", path: "/api/v1/prs", status: 202, body: () => prJob(hub.job) },
    {
      method: "GET",
      path: "/api/v1/pr-jobs/{id}",
      sample: { params: { id: String(JOB_ID) } },
      body: (): PrJob => {
        reads += 1;
        return prJob(hub.job, hub.job === "failed" ? { error: "GitHub refused the push." } : {});
      },
    },
  ];
  return Object.assign(hub, { scenario: derive(base, changes, "journey-pr-hub") });
}

// -- Attribution -------------------------------------------------------------------------------------------------------------

const FILES = "/api/v1/swimlanes/{id}/files";
const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

/** The File page of the design for the core file (editable: no masked value) with a given history and a given author at its head, as an Admin whose user list names Priya and Marco. */
function filePage(label: string, versions: readonly Version[], head: Attribution): string {
  const core = coreFile({ masked: false });
  const base = fileScenario({ id: label, description: label, file: { ...core, entry: { ...core.entry, attribution: head } } });
  return register(
    {
      description: label,
      responses: [
        { method: "GET", path: `${FILES}/history`, sample: { params: { id: LANE } }, body: { items: versions, next: null } },
        ...base.responses.filter((r) => !(r.method === "GET" && r.path === `${FILES}/history`)),
        { method: "GET", path: "/api/v1/admin/users", body: { items: USERS, next: null }, sample: { query: new URLSearchParams("limit=500") } },
      ],
    },
    label,
  );
}

/**
 * Two answers of one hub for the same file: before, nobody is known to have changed it (the newest version and the head say
 * "Unknown author"); after, the sentinel's evidence has come in and an OS login is named. A journey serves the first, then the
 * second, and writes an `attribution_updated` event: the page must read the history and the head again by itself.
 */
export function upgradeHub(): { readonly before: string; readonly after: string } {
  const after = historyVersions();
  const first = after[0];
  if (first === undefined) throw new Error("the history of the attribution scenario is empty");
  after[0] = { ...first, attribution: BY_SOURCE.login };
  return { before: filePage("journey-attribution-before", historyVersions(), BY_SOURCE.unknown), after: filePage("journey-attribution-after", after, BY_SOURCE.login) };
}

/** The attribution scenario for a person with a role of their own: the same file and history, `me` replaced. The user list is still answered, to prove it is not asked. */
export function asRole(base: string, role: Role, who: { id: string; name: string; email: string }): string {
  const me = responseOf(base, "GET", "/api/v1/me");
  return derive(base, [{ ...me, body: meBody(role, { id: who.id, display_name: who.name, email: who.email }), headers: { "set-cookie": csrfCookie } }], `journey-as-${role}`);
}

export { MARCO, PRIYA };

// -- Severity -----------------------------------------------------------------------------------------------------------------

/** The files of the Drift tab: two groups, and in the second the four levels and a file nobody rated, in an order that is not the right one. */
function driftFiles(): FileEntry[] {
  const nfsAhead = (path: string, severity: Severity | null): FileEntry => fileEntry(path, { drift: "nfs_ahead", severity, attribution: BY_SOURCE.login });
  return [
    nfsAhead("services/low.yml", "low"),
    nfsAhead("services/unrated.yml", null),
    nfsAhead("services/critical.yml", "critical"),
    nfsAhead("services/medium.yml", "medium"),
    nfsAhead("services/high.yml", "high"),
    fileEntry("limits/limits.yml", { drift: "conflict", severity: "high", attribution: BY_SOURCE.unknown }),
  ];
}

/** The open findings: the four levels, newest (highest id) first as the hub lists them, which is not the order of severity. */
export const SEVERITY_FINDINGS: readonly Finding[] = [
  finding(4, { kind: "duplicate_keys", severity: "low", path: "services/low.yml", message: "a is set twice" }),
  finding(3, { kind: "environment_host_mismatch", severity: "critical", path: "services/critical.yml", message: "a prod host in a SIT file", locations: [] }),
  finding(2, { kind: "orphan_file", severity: "medium", path: "services/medium.yml", message: "the file is on NFS only", locations: [] }),
  finding(1, { kind: "served_mismatch", severity: "high", path: "services/high.yml", message: "the config-server serves other content", locations: [] }),
];

/**
 * One hub for the three places that draw a severity (the History panel of a file, the Drift tab of its swimlane, the Findings
 * screen and the swimlane's Findings tab): the history of `design-attribution-history`, a drift list and a findings list that carry
 * all four levels, and the findings served as the hub serves them (newest first, `min_severity` honoured).
 */
export function severityHub(): string {
  return derive(
    "design-attribution-history",
    [
      { method: "GET", path: "/api/v1/swimlanes/{id}/drift", sample: { params: { id: LANE } }, body: { items: driftFiles(), next: null } },
      { method: "GET", path: "/api/v1/findings", sample: { query: new URLSearchParams("limit=100") }, samples: [{ query: new URLSearchParams("min_severity=high&limit=100") }], body: (context: MockContext) => findingPage(SEVERITY_FINDINGS, context.query) },
    ],
    "journey-severity",
  );
}

// -- Masking -----------------------------------------------------------------------------------------------------------------

/** A value that the hub must never have sent as text. The malformed-mask scenarios put it where a broken hub could. */
export const RAW_SECRET = "hunter2-correct-horse";

export type Malformed = "not-a-marker" | "outside-line" | "missing-line";

/**
 * The core file as a broken hub could send it: line 19 holds a RAW value instead of a marker, and the mask for it does not describe a
 * marker. `not-a-marker`: the span is inside the line and is plain text. `outside-line`: the span runs past the end of its line.
 * `missing-line`: the mask names a line the text does not have. Whatever the hub meant, the page must show none of the value.
 */
export function malformedMaskScenario(kind: Malformed): string {
  const lines = coreLines(412, { secret: "none" });
  lines[18] = `    clientSecret: ${RAW_SECRET}-and-some-padding-after-it`;
  const text = joinLines(lines, "\r\n");
  const start = lineStart(lines, 19, "\r\n") + "    clientSecret: ".length;
  const mask: Mask = kind === "not-a-marker" ? { line: 19, start, end: start + 36, setting: SECRET_SETTING } : kind === "outside-line" ? { line: 19, start, end: start + 400, setting: SECRET_SETTING } : { line: 5000, start, end: start + 36, setting: SECRET_SETTING };
  const base = coreFile({ masked: false });
  const content = { ...base.content, text, size: new TextEncoder().encode(text).length, masks: [mask] };
  const file: FileKit = { ...base, content, entry: { ...base.entry, size: content.size } };
  return register(fileScenario({ id: `journey-malformed-${kind}`, description: `A masked file whose mask is ${kind}`, file }), `journey-malformed-${kind}`);
}

/** Where a secret must not be on a page that hides it: the document's markup, its text, the accessible tree (names and descriptions), the titles, and the clipboard after a select-all and a copy. */
export async function whereIs(page: Page, secret: string): Promise<string[]> {
  const hits: string[] = [];
  const dom = await page.evaluate((needle) => {
    const found: string[] = [];
    if (document.documentElement.outerHTML.includes(needle)) found.push("the markup");
    if (document.body.innerText.includes(needle)) found.push("the text");
    for (const el of document.querySelectorAll("*")) {
      for (const name of ["title", "aria-label", "aria-description", "alt", "placeholder", "value", "data-value", "aria-valuetext"]) if (el.getAttribute(name)?.includes(needle)) found.push(`${name} of <${el.tagName.toLowerCase()}>`);
    }
    return found;
  }, secret);
  hits.push(...dom);
  if ((await page.locator("body").ariaSnapshot()).includes(secret)) hits.push("the accessible tree");
  // What a person gets by selecting everything and copying it: the clipboard is set to something else first, so a failed copy is told from a clean one.
  await page.evaluate(() => navigator.clipboard.writeText("nothing copied yet"));
  await page.evaluate(() => {
    const selection = window.getSelection();
    selection?.selectAllChildren(document.body);
  });
  await page.keyboard.press("ControlOrMeta+c");
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  if (copied.includes(secret)) hits.push("the clipboard");
  await page.evaluate(() => window.getSelection()?.removeAllRanges());
  return hits;
}
