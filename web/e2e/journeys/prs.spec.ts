// The pull-request pieces of T5 as a person meets them, against the real build and the hub's own header code: raising a pull
// request and finding it in the swimlane's list, the refusal that shows the pull request that already exists, a job that is never
// said to be done before the server says so (and the "Check now" that appears when live updates are paused), and the badges on the
// Drift tab and the file header that come with a pull request and go with the change it carried.
import type { Page } from "@playwright/test";
import { CORE_PATH, LANE } from "../../src/mocks/design/file/data.ts";
import type { PrLink } from "../../src/api/generated/model/index.ts";
import type { Harness } from "../harness/serve.ts";
import { DIST, bodyOf, callsTo, derive, expect, finish, h1, open, responseOf, test, writesOf, type MockContext } from "./support";
import { JOB_ID, NEW_LINK, PR_ADDRESS, PR_NUMBER, emit, goOffline, live, pill, prHub } from "./t5";

test.use({ dist: DIST });

const FILE = `/swimlanes/sitb/files?path=${encodeURIComponent(CORE_PATH)}`;
const dialog = (page: Page) => page.getByRole("dialog", { name: /pull request/i });
/** The writes of the page, leaving out the previews (a POST that reads: what the dialog will write, asked before anything exists). */
const writes = (harness: Harness) => writesOf(harness).filter((r) => r.path !== "/api/v1/prs/preview");

/** Raise a pull request from the file page: the tenant branch, the default title. Leaves the dialog on the job (or the refusal). */
async function raise(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Raise PR" }).click();
  const raising = page.getByRole("dialog", { name: "Raise pull request" });
  await expect(raising).toBeVisible();
  await raising.getByRole("button", { name: "Continue" }).click();
  await raising.getByRole("button", { name: "Create PR" }).click();
}

test.describe("raising a pull request and finding it", () => {
  test("'Find my pull request' leads to the swimlane's list, where the pull request that was raised is the row", async ({ page, harness, csp }) => {
    // The job finishes at once, and the hub lists the pull request from then on, as it does.
    const hub = prHub({ job: "completed" });
    harness.setScenario(hub.scenario);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await raise(page);
    const done = page.getByRole("dialog", { name: "Pull request created" });
    await expect(done).toBeVisible();
    await expect(done).toContainText(`PR #${String(PR_NUMBER)} created.`);
    await expect(done.getByRole("link", { name: new RegExp(`github\\.com/csp/csp-tenant-data/pull/${String(PR_NUMBER)}`) }).first()).toHaveAttribute("href", PR_ADDRESS);
    const find = done.getByRole("link", { name: "Find my pull request" });
    await expect(find).toHaveAttribute("href", "/swimlanes/sitb/pull-requests");
    // The page wrote one request, the pull request, and no other write.
    expect(callsTo(harness, "POST", "/api/v1/prs")).toHaveLength(1);
    expect(writes(harness)).toHaveLength(1);
    await find.click();
    await expect(page).toHaveURL(/\/swimlanes\/sitb\/pull-requests$/);
    await expect(h1(page)).toHaveText("Pull requests");
    // The dialog belongs to the file page: it is gone, and the list is the hub's answer for open pull requests of this swimlane.
    await expect(page.getByRole("dialog")).toHaveCount(0);
    const table = page.getByRole("grid", { name: "Pull requests" });
    await expect(table.getByRole("row")).toHaveCount(2);
    await expect(table.getByRole("row", { name: /#419/ })).toContainText("tx-infinity-core-sit7.yml");
    await expect(table.getByRole("row", { name: /#419/ })).toContainText("You");
    await expect(table.getByRole("link", { name: /#419/ })).toHaveAttribute("href", PR_ADDRESS);
    await expect(page.getByText("1 open pull request")).toBeVisible();
    // The file in the row is a link back into the app.
    await expect(table.getByRole("link", { name: CORE_PATH })).toHaveAttribute("href", `/swimlanes/sitb/files?path=${encodeURIComponent(CORE_PATH)}`);
    expect(callsTo(harness, "GET", /^\/api\/v1\/prs\?swimlane=sitb&state=open/).length).toBeGreaterThan(0);
    await finish(page, csp, harness);
  });

  test("the list says what is true for each state: merged and closed ones are only in their own filters, and an empty one says so", async ({ page, harness, csp }) => {
    harness.setScenario("design-prs-list");
    await open(page, harness.url, "/swimlanes/sitb/pull-requests", "Pull requests");
    const table = page.getByRole("grid", { name: "Pull requests" });
    await expect(table.getByRole("row", { name: /#418/ })).toBeVisible();
    // Open is the default: #421 carries two files and so has two rows; the merged and the closed one are not here.
    await expect(table.getByRole("row", { name: /#421/ })).toHaveCount(2);
    await expect(table).not.toContainText("#417");
    await expect(table).not.toContainText("#405");
    await page.getByRole("radio", { name: "Merged", exact: true }).click();
    await expect(page).toHaveURL(/\/pull-requests\?state=merged$/);
    await expect(table.getByRole("row", { name: /#417/ })).toBeVisible();
    await expect(table.getByRole("row", { name: /#417/ })).toContainText("application.properties");
    await page.getByRole("radio", { name: "All", exact: true }).click();
    await expect(table.getByRole("row", { name: /#405/ })).toBeVisible();
    await expect(table.getByRole("row")).toHaveCount(6);
    // Another swimlane's pull request (#409, sitc) is not on this swimlane's page.
    await expect(table).not.toContainText("#409");
    // A swimlane with none: the empty state names the state.
    harness.setScenario("design-prs-empty");
    await page.getByRole("radio", { name: "Closed", exact: true }).click();
    await expect(page.getByText("No closed pull requests")).toBeVisible();
    await finish(page, csp, harness);
  });
});

test.describe("a pull request that exists already", () => {
  test("is shown, not made again: the card says which, GitHub is one link and the swimlane's list is the other, and no second request goes out", async ({ page, harness, csp }) => {
    harness.setScenario("design-file-pr-exists");
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await page.getByRole("button", { name: "Raise PR" }).click();
    const raising = page.getByRole("dialog", { name: "Raise pull request" });
    await raising.getByRole("radio", { name: /Base \(all swimlanes\)/ }).click();
    await raising.getByRole("button", { name: "Continue" }).click();
    await raising.getByRole("button", { name: "Create PR" }).click();
    // The hub refused the second one (409 duplicate_pr) and named the first; the dialog shows that one.
    const card = raising.locator("[data-slot='duplicate-pr']");
    await expect(card).toBeVisible();
    await expect(card).toContainText("PR #418 is already open for this change.");
    await expect(card.locator("[data-slot='pr-card']")).toContainText("Pull request #418");
    await expect(card.locator("[data-slot='pr-card']")).toContainText("tx-infinity-core-sit7.yml");
    const open418 = raising.getByRole("link", { name: "Open PR #418" });
    await expect(open418).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/418");
    await expect(open418).toHaveAttribute("target", "_blank");
    await expect(open418).toHaveAttribute("rel", /noopener/);
    expect(callsTo(harness, "POST", "/api/v1/prs")).toHaveLength(1);
    expect(callsTo(harness, "POST", "/api/v1/prs")[0]).toMatchObject({ status: 409 });
    // The other link goes to the list, where that pull request is the row.
    await raising.getByRole("link", { name: "All pull requests for sitb" }).click();
    await expect(page).toHaveURL(/\/swimlanes\/sitb\/pull-requests$/);
    await expect(page.getByRole("grid", { name: "Pull requests" }).getByRole("row", { name: /#418/ })).toBeVisible();
    // Nothing else was written: the refusal was the whole of it.
    expect(writes(harness)).toHaveLength(1);
    await finish(page, csp, harness, { expected: [409] });
  });
});

test.describe("the job of a pull request", () => {
  test("is never said to be done before the server says so: an event that claims it is read again, the answer rules, and 'created' comes with the completed job", async ({ page, harness, csp }) => {
    const hub = prHub({ job: "pending" });
    harness.setScenario(hub.scenario);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await live(page);
    await raise(page);
    const job = page.locator("[data-slot='pr-job']");
    const modal = dialog(page);
    // Queued: the title says what is happening, no step has begun, and nothing says "created".
    await expect(modal).toBeVisible();
    await expect(page.getByRole("dialog", { name: "Creating pull request" })).toBeVisible();
    await expect(job).toHaveAttribute("data-phase", "queued");
    await expect(job).toContainText("Waiting to start.");
    await expect(job.locator("li[data-status='active']")).toHaveCount(0);
    await expect(job.locator("li[data-status='done']")).toHaveCount(0);
    await expect(modal).not.toContainText(/created/i);
    await expect(modal.getByRole("link", { name: /github\.com/ })).toHaveCount(0);

    // The server moves on and says so in an event: the view reads the job again and shows what it is told.
    hub.job = "preparing";
    emit(harness, { type: "pr_job_progress", job_id: JOB_ID, state: "preparing" });
    await expect(job).toHaveAttribute("data-phase", "running");
    await expect(job).toContainText("Creating your pull request.");
    await expect(job.locator("li[data-status='active']")).toHaveText("Preparing");
    await expect(job.locator("li[data-status='waiting']")).toHaveCount(3);

    // An event that SAYS completed while the server still says committing: the job is read, and the server's answer is what is shown.
    hub.job = "committing";
    const before = hub.reads();
    emit(harness, { type: "pr_job_progress", job_id: JOB_ID, state: "completed" });
    await expect.poll(() => hub.reads()).toBeGreaterThan(before);
    await expect(job).toHaveAttribute("data-phase", "running");
    await expect(job.locator("li[data-status='active']")).toHaveText("Committing");
    await expect(job.locator("li[data-status='done']")).toHaveText(["Preparing"]);
    await expect(modal).not.toContainText(/created/i);
    await expect(page.getByRole("dialog", { name: "Creating pull request" })).toBeVisible();
    // And the list of pull requests has not got the new one: the job did not finish.
    expect(hub.extra).toEqual([]);

    // The server completes the job: now, and only now, it is created, with the address the server gave.
    hub.job = "completed";
    emit(harness, { type: "pr_job_progress", job_id: JOB_ID, state: "completed" });
    const done = page.getByRole("dialog", { name: "Pull request created" });
    await expect(done).toBeVisible();
    await expect(job).toHaveAttribute("data-phase", "completed");
    await expect(done).toContainText(`PR #${String(PR_NUMBER)} created.`);
    await expect(done.getByRole("link", { name: /github\.com\/csp\/csp-tenant-data\/pull\/419/ }).first()).toHaveAttribute("href", PR_ADDRESS);
    expect(callsTo(harness, "POST", "/api/v1/prs")).toHaveLength(1);
    await finish(page, csp, harness);
  });

  test("a job that fails says what the server said and shows no steps and no pull request", async ({ page, harness, csp }) => {
    const hub = prHub({ job: "failed" });
    harness.setScenario(hub.scenario);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await raise(page);
    const failed = page.getByRole("dialog", { name: "Pull request not created" });
    await expect(failed).toBeVisible();
    await expect(failed).toContainText("The pull request job failed.");
    await expect(failed).toContainText("GitHub refused the push.");
    await expect(failed.locator("li[data-status]")).toHaveCount(0);
    await expect(failed).not.toContainText(/PR #\d+ created/);
    await expect(failed.getByRole("link", { name: "Find my pull request" })).toHaveCount(0);
    await finish(page, csp, harness);
  });

  test("with live updates paused the view says it may be behind and offers 'Check now', which reads the job and shows what the server says", async ({ page, harness, csp }) => {
    // The back-off of the stream is seconds long: a fake clock brings it forward (it is installed before the first page).
    await page.clock.install();
    const hub = prHub({ job: "committing" });
    harness.setScenario(hub.scenario);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await live(page);
    await raise(page);
    const job = page.locator("[data-slot='pr-job']");
    await expect(job).toHaveAttribute("data-phase", "running");
    await expect(job.getByRole("button", { name: "Check now" })).toHaveCount(0);

    await goOffline(page, harness);
    await expect(pill(page)).toHaveAttribute("data-status", "offline");
    const check = job.getByRole("button", { name: "Check now" });
    await expect(check).toBeVisible();
    await expect(job).toContainText("Live updates are paused, so this may be behind the server.");
    await expect(job.getByRole("alert").or(job.locator("[data-slot='alert']")).filter({ hasText: "Live updates are paused" })).toBeVisible();
    // The job is not the dialog's alone: the shell says the same on the whole page.
    await expect(page.getByText("Live updates paused, retrying")).toBeVisible();
    // The server finished the job while the stream was down; no event could say so. The view still says what it last read.
    hub.job = "completed";
    await expect(job).toHaveAttribute("data-phase", "running");
    await expect(dialog(page)).not.toContainText(/created/i);
    const before = hub.reads();
    await check.click();
    await expect.poll(() => hub.reads()).toBeGreaterThan(before);
    await expect(page.getByRole("dialog", { name: "Pull request created" })).toBeVisible();
    await expect(job).toHaveAttribute("data-phase", "completed");
    // A finished job is not checked again: the button is gone with the pause.
    await expect(job.getByRole("button", { name: "Check now" })).toHaveCount(0);
    // The stream was refused on purpose: the browser logs each refusal (503) as a console error of its own.
    await finish(page, csp, harness, { expected: [503] });
  });
});

test.describe("the pull-request badges", () => {
  test("in the Drift tab: an open pull request links to GitHub in another tab, a merged one waiting for sync is no link, a file with none has no badge, and the list is read once for all the rows", async ({ page, harness, csp }) => {
    // The swimlane's Drift tab, with the links of the design's frame: #418 open on the tenant file, #417 merged on a file that still differs.
    harness.setScenario("design-prs-drift-badges");
    await open(page, harness.url, "/swimlanes/sitb/drift", "sitb");
    const list = page.getByRole("list", { name: "Files not in sync" });
    const tenantRow = list.getByRole("listitem").filter({ hasText: "tx-infinity-core-sit7.yml" });
    await expect(tenantRow.getByRole("link", { name: /PR #418 open/ })).toHaveAttribute("href", "https://github.com/csp-tenant/csp-tenant-data/pull/418");
    const merged = list.getByRole("listitem").filter({ hasText: "application.properties" });
    await expect(merged.locator("[data-slot='pr-badge'][data-pr-state='merged']")).toContainText("Fix merged, awaiting sync");
    // The merged badge is not a link (the design draws it unlinked); the open one opens GitHub in another tab.
    await expect(merged.getByRole("link", { name: /Fix merged/ })).toHaveCount(0);
    await expect(tenantRow.getByRole("link", { name: /PR #418 open/ })).toHaveAttribute("target", "_blank");
    // A file with no pull request has no badge, and the other swimlane's pull request (#409) is on none of these rows.
    await expect(list.locator("[data-slot='pr-badge']")).toHaveCount(2);
    // One read of the open links and one of the merged ones, however many rows there are. The stream's first connection reads what is on
    // screen once more, when a page is already showing it (a connection with nothing to resume from), so a round is once or twice, and
    // the two lists are read the same number of times.
    await live(page);
    await page.waitForTimeout(700);
    const asked = callsTo(harness, "GET", /^\/api\/v1\/prs\?/).map((r) => new URL(r.path, harness.url).searchParams.get("state"));
    const times = (state: string): number => asked.filter((s) => s === state).length;
    expect(new Set(asked)).toEqual(new Set(["open", "merged"]));
    expect(times("open")).toBe(times("merged"));
    expect(times("open")).toBeLessThanOrEqual(2);
    await finish(page, csp, harness);
  });

  test("a pull request raised from the file shows on its header when the job completes, and the merged badge goes when the file stops differing", async ({ page, harness, csp }) => {
    const hub = prHub({ job: "pending" });
    harness.setScenario(hub.scenario);
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await live(page);
    const header = page.locator("[data-slot='pr-badge']");
    // No pull request yet: the header has no badge.
    await expect(header).toHaveCount(0);
    await raise(page);
    hub.job = "completed";
    emit(harness, { type: "pr_job_progress", job_id: JOB_ID, state: "completed" });
    await expect(page.getByRole("dialog", { name: "Pull request created" })).toBeVisible();
    await page.getByRole("dialog").getByRole("button", { name: "Close" }).first().click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // The completed job named the list of pull requests, which was read again: the badge is the open one, and links to it.
    const badge = page.locator("[data-slot='pr-badge'][data-pr-state='open']");
    await expect(badge).toHaveCount(1);
    await expect(badge).toContainText(`PR #${String(PR_NUMBER)} open`);
    await expect(badge).toHaveAttribute("href", PR_ADDRESS);
    // The pull request merges: the link is merged, the file still differs (NFS is ahead until the sync), and the badge says so.
    const merged: PrLink = { ...NEW_LINK, state: "merged", closed_at: NEW_LINK.created_at + 60_000 };
    hub.extra.push(merged);
    hub.job = "pending";
    emit(harness, { type: "pr_state_changed", link_id: merged.id, state: "merged" });
    const awaiting = page.locator("[data-slot='pr-badge'][data-pr-state='merged']");
    await expect(awaiting).toContainText("Fix merged, awaiting sync");
    await expect(page.locator("[data-slot='pr-badge'][data-pr-state='open']")).toHaveCount(0);
    // The swimlane syncs: the file no longer differs. The badge goes (it would be a false claim: nothing is waiting).
    harness.setScenario(syncedScenario(hub.scenario));
    emit(harness, { type: "drift_changed", swimlane: LANE, paths: [CORE_PATH] });
    await expect(page.getByText("In sync", { exact: true }).first()).toBeVisible();
    await expect(page.locator("[data-slot='pr-badge']")).toHaveCount(0);
    await finish(page, csp, harness);
  });
});

/** The same hub, with the file in sync: the tree entry (which the header reads) and the drift counts say so. */
function syncedScenario(base: string): string {
  const tree = responseOf(base, "GET", "/api/v1/swimlanes/{id}/tree");
  return derive(
    base,
    [
      {
        ...tree,
        body: (context: MockContext) => {
          const answer = bodyOf(tree, context) as { items: Array<{ path: string; drift: string; pickup_state: string | null }>; next: null };
          return { ...answer, items: answer.items.map((item) => (item.path === CORE_PATH ? { ...item, drift: "in_sync", pickup_state: null } : item)) };
        },
      },
    ],
    "journey-synced",
  );
}
