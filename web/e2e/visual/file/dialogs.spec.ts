// Tier 2, the dialogs of the file view: board "Consequence Dialogs" frames 1b to 1k (Raise PR), 2a and 2b (Delete) and the four dark frames
// 9a to 9d, each a 600 px specimen in the design. The words of the specimen must be in the app's dialog (see lib/parity.ts), the differences
// being listed with their reasons, and the app's dialog has its own baseline.
//
// 1a is the board's live prototype, not a specimen. 1h (a job resumed after a reload) is not offered by the app (DEVIATIONS "File: Raise PR");
// the last test asserts that.
import { expect, test } from "../../fixtures/test";
import type { Page } from "@playwright/test";
import { pin, settle } from "../access/lib/app";
import { parity, type Difference, type ParityCase } from "../access/lib/parity";
import { api, rewrite } from "../access/lib/steps";

const BOARD = "Lanekeeper Consequence Dialogs.dc.html";
const FILE = "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const dialog = (page: Page) => page.getByRole("dialog");
const confirm = (page: Page) => page.getByRole("alertdialog");

const openPr = async (page: Page): Promise<void> => {
  await page.getByRole("button", { name: "Raise PR" }).click();
  await page.getByRole("dialog", { name: "Raise pull request" }).waitFor();
};
const chooseBase = async (page: Page): Promise<void> => {
  await openPr(page);
  await page.getByRole("radio", { name: /^Base/ }).check();
};
const toForm = async (page: Page): Promise<void> => {
  await openPr(page);
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("dialog").getByText("Step 2 of 2").waitFor();
};
const create = async (page: Page): Promise<void> => {
  await toForm(page);
  await page.getByRole("button", { name: "Create PR" }).click();
};
const openDelete = (typed: string) => async (page: Page): Promise<void> => {
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await confirm(page).waitFor();
  await confirm(page).getByRole("textbox").fill(typed);
};
const list = (value: unknown): Array<Record<string, unknown>> => (Array.isArray(value) ? (value as Array<Record<string, unknown>>) : []);
const jobAt = (state: string) => async (page: Page): Promise<void> => {
  await rewrite(page, api("/pr-jobs/7"), (job) => ({ ...job, state, pr_url: null }));
};

// Why a line of the design is not in the app's dialog. Each is a row of design/DEVIATIONS.md (named by its first column) or a contract that cannot
// fill it; "candidate row" is a difference nobody has written a row for yet, for the design owner to approve or the app to fix.
const why = {
  branch: "contract: PrPreview.base_branch is a name; whether it is the default branch of the repository is not in it (candidate row)",
  head: "contract: the head branch (lk/<swimlane>-<file>) is made by the job, and the preview does not name it (candidate row)",
  data: "dataset: the scenario's names, values and identifiers are not the design's example (the hub's own answers: PR number, repository owner, request id)",
  job: "candidate row: the job is shown by the contract's four states (pending, preparing, committing, opening), not the design's step log with durations, the job id and 'stored on the server'",
  github: "contract: the problem has a title, a type and a request id; what GitHub answered (its status and its words) is not in it",
  checks: "contract: PrLink has the state, number and URL; checks and reviews are not in it",
  effects: "candidate row: step 2 does not repeat the effect block of step 1",
  drift: "candidate row: the success dialog says the file stays NFS ahead until the PR merges, not what the drift list will show",
  del: "candidate row: the delete dialog says what the effective config does from the preview (the changed keys of this file), not the design's example (base v2.3.1, three keys, a restart of three pods)",
};
const d = (key: keyof typeof why, ...texts: string[]): Difference[] => texts.map((text) => ({ text, why: why[key] }));
const MAIN = d("branch", "main (default)");
const DELETE = [
  ...d("del", "After the next restart, tx-infinity-core in sitb will use tx-infinity-core.yml.", "The tenant override leaves NFS now. Running pods keep using it until they restart.", "Now (sit7 file)", "After (base v2.3.1)", "tx.core.pool.maxSize", "40", "tx.core.retry.maxAttempts", "Removed", "tx.core.features.fastSettle", "not set (default false)", "Added", "tx.core.metrics.enabled", "not set", "14 other keys are identical in both files.", "Restart", "3 pods in sitb need a restart to apply this", "The file stays on", "csp-tenant-data@sit7", ". sitb shows"),
  ...d("del", "sitb / tx-infinity-api / tx-infinity-core-sit7.yml"),
];

const cases: ParityCase[] = [
  { name: "step 1, tenant branch selected", board: BOARD, id: "1b", scenario: "design-file-pr", path: FILE, prepare: openPr, region: dialog, differences: MAIN },
  { name: "step 1, Base selected", board: BOARD, id: "1c", scenario: "design-file-pr", path: FILE, prepare: chooseBase, region: dialog, differences: MAIN },
  {
    name: "step 1, the tenant file is new on sit7",
    board: BOARD,
    id: "1d",
    scenario: "design-file-pr",
    path: FILE,
    before: async (page) => {
      // What the hub says of a tenant file that is not on the branch yet: the file is new there, and the preview carries the warning (the browser derives none).
      await rewrite(page, api("/prs/preview"), (body) => ({
        ...body,
        files: list(body["files"]).map((f) => ({ ...f, exists_on_branch: false })),
        warnings: [...list(body["warnings"]), "tx-infinity-core.yml doesn't exist on sit7 yet. Future base changes won't reach sit7 for this file."],
      }));
    },
    prepare: openPr,
    region: dialog,
    differences: MAIN,
  },
  {
    name: "step 2, title and description prefilled",
    board: BOARD,
    id: "1e",
    scenario: "design-file-pr",
    path: FILE,
    prepare: toForm,
    region: dialog,
    differences: [...d("effects", "This PR changes sit7 only.", "sitb picks it up after the PR merges and the swimlane syncs."), ...d("head", "From", "lk/sitb-tx-infinity-core-sit7")],
  },
  {
    name: "an open PR already exists for this change",
    board: BOARD,
    id: "1f",
    scenario: "design-file-pr-exists",
    path: FILE,
    prepare: async (page) => {
      await openPr(page);
      await page.getByRole("button", { name: "Continue" }).click();
      await page.getByText(/already open for this change/).waitFor();
    },
    region: dialog,
    differences: [
      ...d("data", "Update tx-infinity-core for sit7 (from sitb)", "github.com/csp-tenant/csp-tenant-data/pull/418", "Same target and same content (NFS hash 9f3c1a7e42b0), so there is nothing new to create."),
      ...d("checks", "#418 · opened by you 2 h ago · 3 of 3 checks passed · waiting for review"),
      ...d("head", "lk/sitb-tx-infinity-core-sit7"),
    ],
  },
  {
    name: "the job, committing",
    board: BOARD,
    id: "1g",
    scenario: "design-file-pr",
    path: FILE,
    before: jobAt("committing"),
    prepare: create,
    region: dialog,
    differences: d("job", "Checking out csp-tenant-data@sit7 and creating lk/sitb-tx-infinity-core-sit7", "1.2 s", "Writing data/config/tx-infinity-api/tx-infinity-core.yml · 1 file", "working", "Pushing the branch and opening the PR against sit7", "The PR link appears here", "Job prj_5d21c8 · started by you · stored on the server, so it survives a reload."),
  },
  {
    name: "success, with the link to the pull request",
    board: BOARD,
    id: "1i",
    scenario: "design-file-pr",
    path: FILE,
    prepare: create,
    region: dialog,
    differences: [...d("head", "lk/sitb-tx-infinity-core-sit7"), ...d("data", "github.com/csp-tenant/csp-tenant-data/pull/419")],
  },
  {
    name: "error, the GitHub token has expired",
    board: BOARD,
    id: "1j",
    scenario: "design-file-pr-token-expired",
    path: FILE,
    prepare: create,
    region: dialog,
    differences: [...d("github", "GitHub answered 401 Bad credentials for", "Why", "The token saved in My token expired on 8 Oct 2026."), ...d("data", "req_8c41f0a2e7")],
  },
  {
    name: "error, no push access",
    board: BOARD,
    id: "1k",
    scenario: "design-file-pr-no-access",
    path: FILE,
    prepare: create,
    region: dialog,
    differences: [
      ...d("github", "You don't have push access to csp-tenant-data, so the PR was not created.", "What happened", "GitHub answered 403 when creating", "Your GitHub account is signed in and the token is valid, but it has Read access to this repo only."),
      ...d("head", "lk/sitb-tx-infinity-core-sit7"),
      ...d("data", "req_2f90b7d6c1"),
    ],
  },
  { name: "delete, the name is not yet typed", board: BOARD, id: "2a", scenario: "design-file-core", path: FILE, prepare: openDelete("tx-infinity-core-s"), region: confirm, differences: DELETE },
  { name: "delete, the name matches", board: BOARD, id: "2b", scenario: "design-file-core", path: FILE, prepare: openDelete("tx-infinity-core-sit7.yml"), region: confirm, differences: DELETE },
  { name: "dark: step 1, Base selected", board: BOARD, id: "9a", theme: "dark", scenario: "design-file-pr", path: FILE, prepare: chooseBase, region: dialog, differences: MAIN },
  {
    name: "dark: the job, opening the pull request",
    board: BOARD,
    id: "9b",
    theme: "dark",
    scenario: "design-file-pr",
    path: FILE,
    before: jobAt("opening"),
    prepare: create,
    region: dialog,
    differences: d("job", "Checking out csp-tenant-data@sit7 and creating lk/sitb-tx-infinity-core-sit7", "1.2 s", "Writing data/config/tx-infinity-api/tx-infinity-core.yml · 1 file", "0.8 s", "Pushing the branch and opening the PR against sit7", "working", "The PR link appears here", "Job prj_5d21c8 · started by you · stored on the server, so it survives a reload."),
  },
  { name: "dark: delete, the name matches", board: BOARD, id: "9c", theme: "dark", scenario: "design-file-core", path: FILE, prepare: openDelete("tx-infinity-core-sit7.yml"), region: confirm, differences: DELETE },
  {
    name: "dark: error, no push access",
    board: BOARD,
    id: "9d",
    theme: "dark",
    scenario: "design-file-pr-no-access",
    path: FILE,
    prepare: create,
    region: dialog,
    differences: [
      ...d("github", "You don't have push access to csp-tenant-data, so the PR was not created.", "What happened", "GitHub answered 403 when creating", "Your GitHub account is signed in and the token is valid, but it has Read access to this repo only."),
      ...d("head", "lk/sitb-tx-infinity-core-sit7"),
      ...d("data", "req_2f90b7d6c1"),
    ],
  },
];
parity(cases);

test.describe("frames the app does not draw, by the DEVIATIONS rows", () => {
  test("1h, a job resumed after a reload: a reload forgets the job, and the dialog is not offered again (File: Raise PR)", async ({ page, harness }) => {
    harness.setScenario("design-file-pr");
    await pin(page, "light", { width: 1440, height: 900 });
    await page.goto(`${harness.url}${FILE}`);
    await settle(page);
    await page.getByRole("button", { name: "Raise PR" }).click();
    await page.getByRole("button", { name: "Continue" }).click();
    await page.getByRole("button", { name: "Create PR" }).click();
    await page.getByText("Pull request created").waitFor();
    await page.reload();
    await settle(page);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByText(/resumed|stored on the server/i)).toHaveCount(0);
  });
});
