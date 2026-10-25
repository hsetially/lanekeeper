// The frames of the pull-request pieces (T5, slice B): the Pull requests page of a swimlane, the job of a pull request in the Raise PR
// dialog in every state it can be in, the duplicate that the hub names, and the badges the pull requests put on the Drift tab, the
// file's header and the findings (board "Consequence Dialogs": frames 1f, 1g, 1i and 1l).
//
// None of them is compared with the design pixel for pixel. The Pull requests page has no handoff frame (design/DEVIATIONS.md: "Pull requests (page)", handoff gap H2). The dialog's frames are 600 px specimens that the app's dialog differs from by documented rows (the
// job is the contract's states and no step log: "File: Raise PR", "File: Raise PR, job steps", "File: Raise PR, duplicate"), and e2e/visual/file/dialogs.spec.ts compares
// their words. The badges are drawn in a grouped list and not in the design's table with a "Pull request" column ("Pull request badges"), so
// prs.spec.ts compares the words of the design's frame 1l with the app's. What is left is the app against its own baseline, which CI
// generates; the design's ids are kept in the titles ("1f", "1g", "1i", "1l").
import type { PrLink } from "../../../src/api/generated/model/index.ts";
import { LINKS } from "../../../src/mocks/design/prs/fixtures.ts";
import { expect } from "../../fixtures/test";
import { api, pushEvents, rewrite } from "../access/lib/steps";
import type { FrameDef, Step } from "../search/kit";
import { expectPaused, expectSkeleton, failRequests, holdRequests, pauseStream, pausedMasks } from "../search/steps";

const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const FILE = `/swimlanes/sitb/files?path=${CORE}`;
const PAGE = "/swimlanes/sitb/pull-requests";
const PRS = /\/api\/v1\/prs(\?|$)/;
const JOB = api("/pr-jobs/7");

const PAGE_WHY = "the Pull requests page has no frame in the handoff (design/DEVIATIONS.md: \"Pull requests (page)\", \"Pull requests: rows\", handoff gap H2)";
const JOB_WHY = "the job is the contract's states, with no step log or times (design/DEVIATIONS.md: \"File: Raise PR\", \"File: Raise PR, job steps\"); the dialog's 600 px specimens are compared by words in e2e/visual/file/dialogs.spec.ts";
const DUPLICATE_WHY = "the card says the pull request's number and file, with no title, branches or checks (design/DEVIATIONS.md: \"File: Raise PR, duplicate\"); compared by words in e2e/visual/file/dialogs.spec.ts";
const BADGE_WHY = "the badges sit in the grouped Drift list, the file header and the findings, with no \"Pull request\" column or \"No PR\" (design/DEVIATIONS.md: \"Pull request badges\")";

// ----- steps -----------------------------------------------------------------------------------------------------------------------

const openPr: Step = async ({ page }) => {
  await page.getByRole("button", { name: "Raise PR" }).click();
  await page.getByRole("dialog", { name: "Raise pull request" }).waitFor();
};
const toForm: Step = async (env) => {
  await openPr(env);
  await env.page.getByRole("button", { name: "Continue" }).click();
  await env.page.getByRole("dialog").getByText("Step 2 of 2").waitFor();
};
/** Raise the pull request: the tenant branch, the default title. Leaves the dialog on the job. */
const create: Step = async (env) => {
  await toForm(env);
  await env.page.getByRole("button", { name: "Create PR" }).click();
  await env.page.locator("[data-slot='pr-job']").waitFor();
};

/** What the hub says of the job (the dialog asks for `/pr-jobs/7` as soon as the job is made). */
const jobIs = (state: string, extra: Record<string, unknown> = {}): Step => async ({ page }) => {
  await rewrite(page, JOB, (job) => ({ ...job, state, pr_url: null, ...extra }));
};
const phase = (name: string): Step => async ({ page }) => {
  await page.locator(`[data-slot='pr-job'][data-phase='${name}']`).waitFor();
};
const then = (...steps: Step[]): Step => async (env) => {
  for (const step of steps) await step(env);
};

/** The live stream goes down while the job is on screen: nothing says it has moved, so the view says it may be behind and offers "Check now". */
const streamDown: Step = async ({ page, harness }) => {
  harness.refuseStreams(503, 99, { retryAfter: 1 });
  harness.closeStreams();
  await page.getByRole("button", { name: "Check now" }).waitFor({ timeout: 30_000 });
};

/** The next read of the job fails (an event says the job moved, and the read that follows is refused): the last state stays, with the request id and Retry. */
const refreshFails: Step = async ({ page, harness }) => {
  await page.route(JOB, (route) => route.fulfill({ status: 500, contentType: "application/problem+json", headers: { "x-request-id": "req-mock-0007" }, body: JSON.stringify({ type: "about:blank", title: "Internal error", status: 500, code: "internal", request_id: "req-mock-0007" }) }));
  await pushEvents(page, harness, [201, { type: "pr_job_progress", job_id: 7, state: "committing" }]);
  await page.getByRole("button", { name: "Retry" }).waitFor();
};

/** The links the hub lists for the swimlane: the scenario's, and one more that arrives after the page was read (a state of the list that is held still). */
const NEWER: PrLink = { id: 15, swimlane: "sitb", path: "document-service/resources/receipt-logo.jpeg", nfs_hash: "a".repeat(64), state: "open", pr_url: "https://github.com/csp-tenant/csp-tenant-data/pull/422", pr_number: 422, author: LINKS[0]?.author ?? "", created_at: 1_760_000_000_000 - 60_000, closed_at: null };
const newPullRequest: Step = async ({ page, harness }) => {
  await page.route(PRS, async (route) => {
    const response = await route.fetch();
    const body = (await response.json()) as { items: PrLink[]; next: string | null };
    await route.fulfill({ response, json: { ...body, items: [NEWER, ...body.items] } });
  });
  await pushEvents(page, harness, [301, { type: "pr_state_changed", link_id: 15, state: "open" }]);
  await page.getByRole("status").filter({ hasText: "held still" }).waitFor();
};

/** A list that has a next page: the foot says how much is shown and offers more. */
const moreRows: Step = async ({ page }) => {
  await rewrite(page, PRS, (body) => ({ ...body, next: "2" }));
};

/** The same links, one of which (#418) was raised for an earlier version of the file than the one the drift now shows. */
const outdatedLink: Step = async ({ page }) => {
  await rewrite(page, PRS, (body) => ({ ...body, items: (body["items"] as PrLink[]).map((link) => (link.pr_number === 418 ? { ...link, nfs_hash: "b".repeat(64) } : link)) }));
};

/** An open pull request on a drift finding of sitc (the Findings tab): the hub's list has a link for it. */
const sitcLink: Step = async ({ page }) => {
  await page.route(PRS, (route) =>
    route.fulfill({
      json: { items: [{ id: 31, swimlane: "sitc", path: "tx-infinity-api/tx-infinity-core.yml", nfs_hash: "a".repeat(64), state: "open", pr_url: "https://github.com/csp-tenant/csp-tenant-data/pull/409", pr_number: 409, author: LINKS[0]?.author ?? "", created_at: 1_760_000_000_000 - 3_600_000, closed_at: null }], next: null },
    }),
  );
};

// ----- frames ----------------------------------------------------------------------------------------------------------------------

const frame = (code: string, def: Omit<FrameDef, "board" | "frame" | "kind">): FrameDef => ({ board: "states", frame: code, kind: "app", ...def });

export const frames: FrameDef[] = [
  // The Pull requests page.
  frame("B01", { scenario: "design-prs-list", path: PAGE, title: "Pull requests: open, with a pull request that carries two files", why: PAGE_WHY }),
  frame("B02", { scenario: "design-prs-list", path: PAGE, theme: "dark", title: "Pull requests: open, dark", why: PAGE_WHY }),
  frame("B03", { scenario: "design-prs-list", path: `${PAGE}?state=merged`, title: "Pull requests: merged", why: PAGE_WHY }),
  frame("B04", { scenario: "design-prs-list", path: `${PAGE}?state=closed`, title: "Pull requests: closed", why: PAGE_WHY }),
  frame("B05", { scenario: "design-prs-list", path: `${PAGE}?state=all`, title: "Pull requests: all", why: PAGE_WHY }),
  frame("B06", { scenario: "design-prs-empty", path: PAGE, title: "Pull requests: none open", why: PAGE_WHY }),
  frame("B07", { scenario: "design-prs-empty", path: PAGE, theme: "dark", title: "Pull requests: none open, dark", why: PAGE_WHY }),
  frame("B08", { scenario: "design-prs-empty", path: `${PAGE}?state=merged`, title: "Pull requests: none merged", why: PAGE_WHY }),
  frame("B09", { scenario: "design-prs-empty", path: `${PAGE}?state=all`, title: "Pull requests: none at all", why: PAGE_WHY }),
  frame("B10", { scenario: "design-prs-hostile", path: PAGE, title: "Pull requests: hostile paths and addresses (override, credentials, http, a script, 600 characters)", why: PAGE_WHY }),
  frame("B11", { scenario: "design-prs-hostile", path: PAGE, theme: "dark", title: "Pull requests: hostile, dark", why: PAGE_WHY }),
  frame("B12", { scenario: "design-prs-list", path: PAGE, title: "Pull requests: loading", why: PAGE_WHY, before: ({ page }) => holdRequests(page, PRS), ready: ({ page }) => expectSkeleton(page) }),
  frame("B13", { scenario: "design-prs-list", path: PAGE, title: "Pull requests: error with the request id", why: PAGE_WHY, before: ({ page }) => failRequests(page, PRS) }),
  frame("B14", { scenario: "design-prs-list", path: PAGE, title: "Pull requests: live updates paused", why: PAGE_WHY, before: pauseStream, steps: ({ page }) => expectPaused(page), mask: pausedMasks }),
  frame("B15", { scenario: "design-prs-list", path: PAGE, title: "Pull requests: a next page", why: PAGE_WHY, before: moreRows }),
  frame("B16", { scenario: "design-prs-list", path: PAGE, title: "Pull requests: a new pull request is held until it is asked for", why: PAGE_WHY, steps: newPullRequest }),
  frame("B17", { scenario: "design-prs-list", path: PAGE, size: { width: 1024, height: 768 }, title: "Pull requests: 1024 px wide", why: PAGE_WHY }),

  // The job of a pull request (frames 1g and 1i), in the Raise PR dialog.
  frame("B20", { scenario: "design-file-pr", path: FILE, title: "job 1g: queued, nothing has begun", why: JOB_WHY, before: jobIs("pending"), steps: then(create, phase("queued")) }),
  frame("B21", { scenario: "design-file-pr", path: FILE, title: "job 1g: preparing", why: JOB_WHY, before: jobIs("preparing"), steps: then(create, phase("running")) }),
  frame("B22", { scenario: "design-file-pr", path: FILE, title: "job 1g: committing", why: JOB_WHY, before: jobIs("committing"), steps: then(create, phase("running")) }),
  frame("B23", { scenario: "design-file-pr", path: FILE, theme: "dark", title: "job 1g: committing, dark", why: JOB_WHY, before: jobIs("committing"), steps: then(create, phase("running")) }),
  frame("B24", { scenario: "design-file-pr", path: FILE, title: "job: opening the pull request", why: JOB_WHY, before: jobIs("opening"), steps: then(create, phase("running")) }),
  frame("B25", { scenario: "design-file-pr", path: FILE, title: "job 1i: created, with the address the server gave", why: JOB_WHY, steps: then(create, phase("completed")) }),
  frame("B26", { scenario: "design-file-pr", path: FILE, theme: "dark", title: "job 1i: created, dark", why: JOB_WHY, steps: then(create, phase("completed")) }),
  frame("B27", { scenario: "design-file-pr", path: FILE, title: "job: completed and the server gave no address", why: JOB_WHY, before: jobIs("completed"), steps: then(create, phase("completed")) }),
  frame("B28", { scenario: "design-file-pr", path: FILE, title: "job: completed with an address that is a script, so no link", why: JOB_WHY, before: jobIs("completed", { pr_url: "javascript:alert(1)" }), steps: then(create, phase("completed")) }),
  frame("B29", { scenario: "design-file-pr-failed", path: FILE, title: "job: failed, with what the server said", why: JOB_WHY, steps: then(create, phase("failed")) }),
  frame("B30", { scenario: "design-file-pr-failed", path: FILE, theme: "dark", title: "job: failed, dark", why: JOB_WHY, steps: then(create, phase("failed")) }),
  frame("B31", { scenario: "design-file-pr-failed", path: FILE, title: "job: failed, with hostile text from the server", why: JOB_WHY, before: jobIs("failed", { error: "push refused for ‮gnp.exe: <img src=x onerror=alert(1)> ​" }), steps: then(create, phase("failed")) }),
  frame("B32", { scenario: "design-file-pr", path: FILE, title: "job: live updates paused, so the view may be behind: \"Check now\"", why: JOB_WHY, before: jobIs("preparing"), steps: then(create, streamDown), mask: pausedMasks }),
  frame("B33", { scenario: "design-file-pr", path: FILE, title: "job: the last read failed, the last state stays with the request id and Retry", why: JOB_WHY, before: jobIs("preparing"), steps: then(create, refreshFails) }),

  // The duplicate (frame 1f).
  frame("B40", { scenario: "design-file-pr-exists", path: FILE, title: "duplicate 1f: the hub names the pull request that is open for this change (409)", why: DUPLICATE_WHY, steps: async (env) => {
    await openPr(env);
    await env.page.getByRole("button", { name: "Continue" }).click();
    await env.page.locator("[data-slot='duplicate-pr']").waitFor();
  } }),
  frame("B41", { scenario: "design-file-pr-exists", path: FILE, theme: "dark", title: "duplicate 1f, dark", why: DUPLICATE_WHY, steps: async (env) => {
    await openPr(env);
    await env.page.getByRole("button", { name: "Continue" }).click();
    await env.page.locator("[data-slot='duplicate-pr']").waitFor();
  } }),

  // The badges (frame 1l): the Drift tab, the file's header, the findings.
  frame("B50", { scenario: "design-prs-drift-badges", path: "/swimlanes/sitb/drift", title: "badges 1l: Drift, PR #418 open and \"Fix merged, awaiting sync\"", why: BADGE_WHY }),
  frame("B51", { scenario: "design-prs-drift-badges", path: "/swimlanes/sitb/drift", theme: "dark", title: "badges 1l: Drift, dark", why: BADGE_WHY }),
  frame("B52", {
    scenario: "design-prs-drift-badges", path: "/swimlanes/sitb/drift", title: "badges: a pull request raised for an earlier version of the file says so in its note", why: BADGE_WHY, before: outdatedLink,
    // The badge looks as it does for the current version: what differs is its note, which is a tooltip and is not in a photograph.
    steps: async ({ page }) => {
      await expect(page.locator("[data-slot='pr-badge'][data-pr-state='open']").first()).toHaveAttribute("title", /raised for an earlier version/);
    },
  }),
  frame("B53", { scenario: "design-file-pr-exists", path: FILE, title: "badges: the file's header, beside the drift state", why: BADGE_WHY }),
  frame("B54", { scenario: "design-file-pr-exists", path: FILE, theme: "dark", title: "badges: the file's header, dark", why: BADGE_WHY }),
  frame("B55", { scenario: "design-swimlanes-six", path: "/swimlanes/sitc/findings", title: "badges: the swimlane's Findings tab, a drift finding with an open pull request", why: BADGE_WHY, before: sitcLink }),
];
