// The states of the pull-request pieces of T5 (slice B): the Pull requests page of a swimlane (`/swimlanes/:id/pull-requests`: one list
// for each state filter, empty, loading, failed, stale, an update held back, a second page, a long list, links the page must not trust),
// the job and the duplicate card inside the Raise PR dialog (queued, running, completed with and without an address, failed, behind,
// refresh failed, a pull request that exists), and the badges beside a file (the file header, the Drift tab, the Findings tab and the
// drawer of a finding).
import { expect } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { fail, hold, modal } from "./helpers.ts";
import { finding } from "../../src/mocks/design/findings/fixtures.ts";
import { H_NFS } from "../../src/mocks/design/file/data.ts";
import { LINKS } from "../../src/mocks/design/prs/fixtures.ts";
import { CORE_FILE, frame, goOffline, itemsOf, jobIs, listPrs, manyPrs, patchJson, raisePr, withPrs } from "./t5.ts";

const PAGE = "/swimlanes/sitb/pull-requests";
const PRS_LIST = /\/api\/v1\/prs(\?.*)?$/;
const FILE_PRS = /\/api\/v1\/prs\?/;

export const t5PrCases: Case[] = [
  // ---- The Pull requests page -----------------------------------------------------------------------------------------------------------
  { id: "pull requests / open (the default filter)", scenario: "design-prs-list", url: PAGE },
  { id: "pull requests / merged", scenario: "design-prs-list", url: `${PAGE}?state=merged` },
  { id: "pull requests / closed", scenario: "design-prs-list", url: `${PAGE}?state=closed` },
  { id: "pull requests / all", scenario: "design-prs-list", url: `${PAGE}?state=all` },
  { id: "pull requests / none open (empty)", scenario: "design-prs-empty", url: PAGE },
  { id: "pull requests / none merged (empty)", scenario: "design-prs-empty", url: `${PAGE}?state=merged` },
  { id: "pull requests / none closed (empty)", scenario: "design-prs-empty", url: `${PAGE}?state=closed` },
  { id: "pull requests / none at all (empty)", scenario: "design-prs-empty", url: `${PAGE}?state=all` },
  {
    id: "pull requests / loading (the list is held)",
    scenario: "design-prs-list",
    url: PAGE,
    async before({ page }) {
      await hold(page, PRS_LIST);
    },
    async open({ page }) {
      await expect(page.locator("[aria-busy='true']").first()).toBeVisible();
    },
  },
  {
    id: "pull requests / the list fails (500)",
    scenario: "design-prs-list",
    url: PAGE,
    async before({ page }) {
      await fail(page, PRS_LIST);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    // Stale: the list is on screen, a live event asks for it again, and the hub cannot answer.
    id: "pull requests / stale: the list could not be refreshed",
    scenario: "design-prs-list",
    url: PAGE,
    async open({ page, harness }) {
      await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
      await fail(page, PRS_LIST);
      harness.emit(frame(800, { type: "pr_state_changed", link_id: 14, state: "merged" }));
      await expect(page.getByText("Couldn't refresh").first()).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    // A pull request arrives while the page is open: the list is held still and says so ("1 update") until the person asks to see it.
    id: "pull requests / an update waits (the list is held still)",
    scenario: "design-prs-list",
    url: PAGE,
    async open({ page, harness }) {
      await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
      const arrived = { ...LINKS[0], id: 99, pr_number: 430, pr_url: "https://github.com/csp-tenant/csp-tenant-data/pull/430", path: "tx-infinity-api/tx-infinity-payments-sit12.yml" };
      await withPrs(page, [arrived as (typeof LINKS)[number], ...LINKS]);
      harness.emit(frame(801, { type: "pr_state_changed", link_id: 99, state: "open" }));
      await expect(page.getByRole("status").filter({ hasText: /1 update/ })).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    id: "pull requests / more than one page (Load more)",
    scenario: "design-prs-list",
    url: PAGE,
    async before({ page }) {
      const links = manyPrs(330);
      await page.route(PRS_LIST, (route) => route.fulfill({ json: listPrs(links, new URL(route.request().url())) }));
    },
    async open({ page }) {
      await expect(page.getByRole("button", { name: "Load more" })).toBeVisible();
    },
  },
  {
    id: "pull requests / a long list (rows scroll inside the table)",
    scenario: "design-prs-list",
    url: `${PAGE}?state=all`,
    async before({ page }) {
      await withPrs(page, manyPrs(95));
    },
    async open({ page }) {
      await expect(page.getByRole("grid", { name: "Pull requests" })).toBeVisible();
    },
  },
  { id: "pull requests / links the page must not trust (hostile)", scenario: "design-prs-hostile", url: PAGE },
  {
    id: "pull requests / a filter chosen with the keyboard",
    scenario: "design-prs-list",
    url: PAGE,
    async open({ page }) {
      await page.getByRole("radio", { name: "Merged" }).focus();
      await page.keyboard.press("Enter");
      await expect(page).toHaveURL(/state=merged/);
    },
  },

  // ---- The job and the duplicate card in the Raise PR dialog ----------------------------------------------------------------------------
  {
    id: "pr job / queued",
    scenario: "design-file-pr",
    url: CORE_FILE,
    async before({ page }) {
      await jobIs(page, "pending");
    },
    async open({ page }) {
      await raisePr(page, 3);
      await expect(page.getByText("Waiting to start.")).toBeVisible();
    },
  },
  {
    id: "pr job / running: preparing",
    scenario: "design-file-pr",
    url: CORE_FILE,
    async before({ page }) {
      await jobIs(page, "preparing");
    },
    async open({ page }) {
      await raisePr(page, 3);
      await expect(page.getByText("Creating your pull request.")).toBeVisible();
    },
  },
  {
    id: "pr job / running: opening the pull request",
    scenario: "design-file-pr",
    url: CORE_FILE,
    async before({ page }) {
      await jobIs(page, "opening");
    },
    async open({ page }) {
      await raisePr(page, 3);
      await expect(page.getByText("Creating your pull request.")).toBeVisible();
    },
  },
  {
    id: "pr job / completed, with a warning",
    scenario: "design-file-pr",
    url: CORE_FILE,
    async before({ page }) {
      await jobIs(page, "completed", { warnings: ["This tenant file forks from base: base changes will no longer reach sit7 through it."] });
    },
    async open({ page }) {
      await raisePr(page, 3);
      await expect(page.getByText(/PR #419 created/)).toBeVisible();
      await expect(page.getByText(/forks from base/)).toBeVisible();
    },
  },
  {
    id: "pr job / completed, the server gave no address that can be opened",
    scenario: "design-file-pr",
    url: CORE_FILE,
    async before({ page }) {
      await jobIs(page, "completed", { pr_url: "javascript:alert(1)" });
    },
    async open({ page }) {
      await raisePr(page, 3);
      await expect(page.getByText(/no address that can be opened/)).toBeVisible();
    },
  },
  {
    id: "pr job / failed, with what the server said",
    scenario: "design-file-pr-failed",
    url: CORE_FILE,
    async open({ page }) {
      await raisePr(page, 3);
      await expect(page.getByText("What the server said")).toBeVisible();
    },
  },
  {
    id: "pr job / live updates paused: it may be behind (Check now)",
    scenario: "design-file-pr",
    url: CORE_FILE,
    async before({ page }) {
      await jobIs(page, "preparing");
    },
    async open(ctx) {
      await raisePr(ctx.page, 3);
      await expect(ctx.page.getByText("Creating your pull request.")).toBeVisible();
      await goOffline(ctx);
      await expect(ctx.page.getByRole("button", { name: "Check now" })).toBeVisible();
    },
  },
  {
    id: "pr job / the last read failed: showing the last state (Retry)",
    scenario: "design-file-pr",
    url: CORE_FILE,
    async before({ page }) {
      await fail(page, /\/api\/v1\/pr-jobs\/\d+$/);
    },
    async open({ page }) {
      await raisePr(page, 3);
      await expect(page.getByText(/Couldn't refresh this job/)).toBeVisible();
    },
  },
  {
    id: "pr duplicate / the hub refuses a second pull request (409) and names the open one",
    scenario: "design-file-pr-exists",
    url: CORE_FILE,
    async before({ page }) {
      // The preview knows of no open pull request, so the dialog goes on to Create PR and the hub answers 409 with the link.
      await patchJson(page, /\/api\/v1\/prs\/preview$/, (preview) => {
        const { open_pr: _open, ...rest } = preview;
        return rest;
      });
    },
    async open({ page }) {
      await raisePr(page, 3);
      await expect(modal(page).getByText(/already open/i).first()).toBeVisible();
    },
  },
  {
    id: "pr duplicate / the open pull request has an address that cannot be opened",
    scenario: "design-file-pr-exists",
    url: CORE_FILE,
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/prs\/preview$/, (preview) => ({ ...preview, open_pr: { ...(preview["open_pr"] as object), pr_url: "http://github.com/csp/x/pull/418" } }));
    },
    async open({ page }) {
      await raisePr(page, 1);
      await page.getByRole("button", { name: "Continue" }).click();
      await expect(modal(page).getByText(/already open/i).first()).toBeVisible();
    },
  },
  {
    id: "pr duplicate / the pull request merged meanwhile",
    scenario: "design-file-pr-exists",
    url: CORE_FILE,
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/prs\/preview$/, (preview) => ({ ...preview, open_pr: { ...(preview["open_pr"] as object), state: "merged", closed_at: 1_759_999_000_000 } }));
    },
    async open({ page }) {
      await raisePr(page, 1);
      await page.getByRole("button", { name: "Continue" }).click();
      await expect(modal(page).getByText(/is merged/i).first()).toBeVisible();
    },
  },

  // ---- The badges beside a file -----------------------------------------------------------------------------------------------------------
  // The file header: an open pull request carries this change (the scenario where it has one), the one that merged and has not synced,
  // and the one that was raised for an earlier version of the file.
  { id: "pr badge / the file header: PR open", scenario: "design-file-pr-exists", url: CORE_FILE },
  {
    id: "pr badge / the file header: PR open, raised for an earlier version",
    scenario: "design-file-pr-exists",
    url: CORE_FILE,
    async before({ page }) {
      await patchJson(page, FILE_PRS, (list) => ({ ...list, items: itemsOf(list).map((l) => ({ ...l, nfs_hash: "b".repeat(64) })) }));
    },
    async open({ page }) {
      await expect(page.locator("[data-slot='pr-badge']").first()).toBeVisible();
    },
  },
  {
    id: "pr badge / the file header: fix merged, awaiting sync",
    scenario: "design-file-pr",
    url: CORE_FILE,
    async before({ page }) {
      const merged = { ...LINKS[0], id: 40, path: "tx-infinity-api/tx-infinity-core-sit7.yml", state: "merged", nfs_hash: H_NFS, closed_at: 1_759_999_000_000 };
      await page.route(FILE_PRS, async (route) => {
        const url = new URL(route.request().url());
        await route.fulfill({ json: listPrs([merged as (typeof LINKS)[number]], url) });
      });
    },
    async open({ page }) {
      await expect(page.locator("[data-slot='pr-badge'][data-pr-state='merged']")).toBeVisible();
    },
  },
  { id: "pr badge / the Drift tab: open, merged and none", scenario: "design-prs-drift-badges", url: "/swimlanes/sitb/drift" },
  {
    // The swimlane's own findings, with a drift finding on the file that has an open pull request.
    id: "pr badge / the Findings tab of a swimlane",
    scenario: "design-swimlanes-six",
    url: "/swimlanes/sitb/findings",
    async before({ page }) {
      await withPrs(page);
      await patchJson(page, /\/api\/v1\/findings(\?.*)?$/, (list) => ({ ...list, items: [finding(11), ...itemsOf(list)] }));
    },
    async open({ page }) {
      await expect(page.getByRole("tabpanel").locator("[data-slot='pr-badge']").first()).toBeVisible();
    },
  },
  {
    // A drift finding of sitc on a file with an open pull request: the drawer shows the badge beside the file.
    id: "pr badge / the drawer of a drift finding",
    scenario: "design-findings-open",
    url: "/findings",
    async before({ page }) {
      await withPrs(page, [{ ...LINKS[1], path: "tx-infinity-api/gateway-routes-sit9.yml" } as (typeof LINKS)[number]]);
    },
    async open({ page }) {
      await page.getByRole("grid", { name: "Findings" }).getByRole("row", { name: /C5 NFS and Git disagree/ }).first().click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await expect(page.getByRole("dialog").locator("[data-slot='pr-badge']")).toBeVisible();
    },
  },
];
