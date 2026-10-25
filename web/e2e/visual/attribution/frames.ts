// The frames of attribution (T5, slice A): who changed a file, how Lanekeeper knows and how sure it is, in every place the chip is drawn:
// the File page's header and History panel (board "File View", frame 1j), the Drift tab (board "Swimlane Detail", frames 1e and 1f), the
// swimlane's Activity tab (1i and 1j) and the Audit log (board "Approvals and Audit", frames 1v, 1ab, 1ag to 1aj and 1an), with the evidence
// popover open, the hostile rows, the severity filter's note and the narrow wrap.
//
// None of them is a pixel comparison with the design (see attribution.spec.ts for the one part that is). Where the design has a frame, the
// app's differs from it by the rows of design/DEVIATIONS.md named in each frame's `why`, and the scenario is the attribution slice's
// (`design-attribution-*`), which has every source and every level of confidence and the design's example has four. So each frame is the
// app against its own baseline, which CI generates; the ids of the design's frames are kept where the frame is the same screen in the same
// state ("approvals" 1v, 1ab, 1ag to 1aj, 1an), and the others are A-codes.
import type { Page } from "@playwright/test";
import type { Attribution, FileEntry, Severity } from "../../../src/api/generated/model/index.ts";
import { BY_SOURCE } from "../../../src/mocks/design/attribution/data.ts";
import { api, rewrite } from "../access/lib/steps";
import type { FrameDef, Step } from "../search/kit";

const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";
export const FILE = `/swimlanes/sitb/files?path=${CORE}`;
export const HISTORY = `${FILE}&panel=history`;
const AUDIT = "/audit";
const SIX = "design-swimlanes-six";

const FILE_ROWS = "File header and History (design/DEVIATIONS.md: \"Attribution chip\", \"Attribution chip: evidence\", \"Attribution chip: long names\", \"Severity badge (every screen)\", \"File header and History\"), and the dataset: the scenario's nine versions, not the design's four";
const DRIFT_ROWS = "Swimlane detail: Drift tab (design/DEVIATIONS.md: \"Attribution chip\", \"Severity badge (every screen)\", \"Swimlane detail: Drift tab\", \"Swimlane detail: Drift tab, row links\", \"Severity menu on a list the server does not filter\"): the app's rows say who, how and how sure, and have the shared severity badge";
const ACTIVITY_ROWS = "Swimlane detail: Activity and Audit tabs (design/DEVIATIONS.md: \"Attribution chip\", \"Swimlane detail: Activity and Audit tabs\")";
const AUDIT_ROWS = "Audit: table and drawer (design/DEVIATIONS.md: \"Attribution chip\", \"Audit: columns\", \"Audit: confidence and via\", \"Audit: filters\", \"Audit: drawer evidence\"): no via, source or confidence filter, and the drawer's evidence is the shared one";

// ----- steps -----------------------------------------------------------------------------------------------------------------------

/** The chip of an attribution inside `scope`, by what it says (a name, or a source's words). */
const chip = (scope: Page | ReturnType<Page["locator"]>, text: RegExp) => scope.locator("[data-slot='attribution'] button", { hasText: text }).first();

/** Open the evidence popover of a chip, and wait for it. */
const openEvidence = (scope: (page: Page) => Page | ReturnType<Page["locator"]>, text: RegExp): Step => async ({ page }) => {
  await chip(scope(page), text).click();
  await page.getByRole("dialog", { name: "Attribution evidence" }).waitFor();
};

const history = (page: Page) => page.getByRole("complementary", { name: "History" });
const header = (page: Page) => page.locator("[data-slot='file-changed-by']");
const everywhere = (page: Page) => page;

/** The drift of sitb as the design's frame 1e draws it: each file with who changed it, how sure, and how serious (the scenario sends none). */
const DRIFT: Readonly<Record<string, { attribution: Attribution; severity: Severity }>> = {
  "limits/limits.yml": { attribution: BY_SOURCE.tool, severity: "critical" },
  "application.properties": { attribution: BY_SOURCE.client, severity: "high" },
  "tx-infinity-api/tx-infinity-core-sit7.yml": { attribution: BY_SOURCE.login, severity: "high" },
  "ui/remote-itm-teller/teller-ui.yml": { attribution: BY_SOURCE.sync, severity: "low" },
  "document-service/resources/receipt-logo.jpeg": { attribution: BY_SOURCE.unknown, severity: "medium" },
};

export const attributedDrift: Step = async ({ page }) => {
  await rewrite(page, api("/swimlanes/sitb/drift"), (body) => ({
    ...body,
    items: (body["items"] as FileEntry[]).map((file) => ({ ...file, ...(DRIFT[file.path] ?? {}) })),
  }));
};

/**
 * The same, with a next page for ever (the hub's list that never ends, as Audit's T02): the app reads its 50 pages, is cut off, and the
 * severity filter says that it reached only the files that are loaded.
 */
const attributedDriftMore: Step = async ({ page }) => {
  await page.route(api("/swimlanes/sitb/drift"), async (route) => {
    const cursor = new URL(route.request().url()).searchParams.get("cursor");
    if (cursor !== null) {
      await route.fulfill({ json: { items: [], next: String(Number(cursor) + 1) } });
      return;
    }
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    await route.fulfill({
      response,
      json: { ...body, next: "1", items: (body["items"] as FileEntry[]).map((file) => ({ ...file, ...(DRIFT[file.path] ?? {}) })) },
    });
  });
};

const eventDrawer = (seq: string): Step => async ({ page }) => {
  await page.getByRole("row", { name: new RegExp(`#${seq}`) }).first().click();
  await page.getByRole("button", { name: "Close" }).first().waitFor();
};

const quickFilter = (name: RegExp): Step => async ({ page }) => {
  await page.getByRole("group", { name: "Quick filter" }).getByRole("button", { name }).click();
};

const NARROW = { width: 1024, height: 768 } as const;

// ----- frames ----------------------------------------------------------------------------------------------------------------------

export const frames: FrameDef[] = [
  // The File page: header "Changed by", and the History panel (frame 1j).
  { board: "states", frame: "A01", kind: "app", scenario: "design-attribution-history", path: HISTORY, title: "File 1j: header and History, every source and confidence", why: FILE_ROWS },
  { board: "states", frame: "A02", kind: "app", theme: "dark", scenario: "design-attribution-history", path: HISTORY, title: "File 1j, dark", why: FILE_ROWS },
  { board: "states", frame: "A03", kind: "app", scenario: "design-attribution-history", path: HISTORY, title: "evidence open: an OS login with executable, identity and time", why: FILE_ROWS, steps: openEvidence(history, /dana\.okoye/) },
  { board: "states", frame: "A04", kind: "app", theme: "dark", scenario: "design-attribution-history", path: HISTORY, title: "evidence open, dark", why: FILE_ROWS, steps: openEvidence(history, /dana\.okoye/) },
  { board: "states", frame: "A05", kind: "app", scenario: "design-attribution-history", path: HISTORY, title: "evidence open: unknown author, no score", why: FILE_ROWS, steps: openEvidence(history, /Unknown author/) },
  { board: "states", frame: "A06", kind: "app", scenario: "design-attribution-history", path: HISTORY, title: "evidence open: the head of the file, \"Changed by\"", why: FILE_ROWS, steps: openEvidence(header, /dana\.okoye/) },
  { board: "states", frame: "A07", kind: "app", scenario: "design-attribution-hostile", path: HISTORY, title: "hostile rows: override, zero-width space, markup, a 256-character login", why: `${FILE_ROWS}; every name is text with the hidden characters shown (S8)` },
  { board: "states", frame: "A08", kind: "app", theme: "dark", scenario: "design-attribution-hostile", path: HISTORY, title: "hostile rows, dark", why: FILE_ROWS },
  { board: "states", frame: "A09", kind: "app", scenario: "design-attribution-hostile", path: HISTORY, title: "hostile: the evidence of the first row", why: FILE_ROWS, steps: openEvidence(history, /invoice/) },
  { board: "states", frame: "A10", kind: "app", scenario: "design-attribution-history", path: HISTORY, size: NARROW, title: "1024 px wide: the panel keeps its width and the chips wrap", why: FILE_ROWS },

  // The Drift tab (Swimlane detail frames 1e and 1f).
  { board: "states", frame: "A11", kind: "app", scenario: SIX, path: "/swimlanes/sitb/drift", title: "Detail 1e: Drift, severity-sorted, who and how sure", why: DRIFT_ROWS, before: attributedDrift },
  { board: "states", frame: "A12", kind: "app", theme: "dark", scenario: SIX, path: "/swimlanes/sitb/drift", title: "Detail 1f: Drift, dark", why: DRIFT_ROWS, before: attributedDrift },
  {
    board: "states", frame: "A13", kind: "app", scenario: SIX, path: "/swimlanes/sitb/drift", title: "severity filter: High and above, with its note about the rows that are loaded", why: DRIFT_ROWS,
    before: attributedDriftMore,
    steps: async ({ page }) => {
      // The list reads its pages one after another until it is cut off at 50: the frame is that end state, not a moment on the way to it.
      await page.getByText("Results cut off").waitFor({ timeout: 30_000 });
      await page.getByRole("combobox", { name: "Severity" }).selectOption("high");
      await page.getByText(/loaded so far/).waitFor();
    },
  },
  { board: "states", frame: "A14", kind: "app", scenario: SIX, path: "/swimlanes/sitb/drift", size: NARROW, title: "1024 px wide: the chip wraps under the path", why: DRIFT_ROWS, before: attributedDrift },
  { board: "states", frame: "A15", kind: "app", scenario: SIX, path: "/swimlanes/sitb/drift", title: "evidence open from a Drift row", why: DRIFT_ROWS, before: attributedDrift, steps: openEvidence(everywhere, /dana\.okoye/) },

  // The Activity tab (Swimlane detail frames 1i and 1j).
  { board: "states", frame: "A16", kind: "app", scenario: "design-attribution-audit", path: "/swimlanes/sitb/activity", title: "Detail 1i: Activity, an event for every source", why: ACTIVITY_ROWS },
  { board: "states", frame: "A17", kind: "app", theme: "dark", scenario: "design-attribution-audit", path: "/swimlanes/sitb/activity", title: "Detail 1j: Activity, dark", why: ACTIVITY_ROWS },
  { board: "states", frame: "A18", kind: "app", scenario: "design-attribution-audit", path: "/swimlanes/sitb/activity", title: "evidence open from an Activity row", why: ACTIVITY_ROWS, steps: openEvidence(everywhere, /dana\.okoye/) },

  // The Audit log (Approvals and Audit frames 1v, 1ab, 1ag to 1aj, 1an).
  { board: "approvals", frame: "1v", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "default: who, and the attribution column with its meter", why: AUDIT_ROWS },
  { board: "states", frame: "A19", kind: "app", theme: "dark", scenario: "design-attribution-audit", path: AUDIT, title: "Audit, dark", why: AUDIT_ROWS },
  { board: "approvals", frame: "1ab", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "quick filter: unknown author", why: AUDIT_ROWS, steps: quickFilter(/^Unknown author/) },
  { board: "approvals", frame: "1ag", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: unknown author, no score, no session matched", why: AUDIT_ROWS, steps: eventDrawer("18,463") },
  { board: "approvals", frame: "1ah", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: an OS login, edited on the NFS VM", why: AUDIT_ROWS, steps: eventDrawer("18,468") },
  { board: "approvals", frame: "1ai", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: attribution upgraded to a mapped user", why: AUDIT_ROWS, steps: eventDrawer("18,467") },
  { board: "approvals", frame: "1aj", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: NFS client at medium, no person named", why: AUDIT_ROWS, steps: eventDrawer("18,465") },
  { board: "approvals", frame: "1an", kind: "app", theme: "dark", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: unknown author, dark", why: AUDIT_ROWS, steps: eventDrawer("18,463") },
  { board: "states", frame: "A20", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: a sync job", why: AUDIT_ROWS, steps: eventDrawer("18,466") },
  { board: "states", frame: "A21", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: a file owner hint at low", why: AUDIT_ROWS, steps: eventDrawer("18,464") },
  { board: "states", frame: "A22", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: a user the list no longer has (a short id)", why: AUDIT_ROWS, steps: eventDrawer("18,462") },
  { board: "states", frame: "A23", kind: "app", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: hostile login, executable and identity", why: AUDIT_ROWS, steps: eventDrawer("18,459") },
  { board: "states", frame: "A24", kind: "app", theme: "dark", scenario: "design-attribution-audit", path: AUDIT, title: "drawer: hostile, dark", why: AUDIT_ROWS, steps: eventDrawer("18,459") },
];
