// Approvals and the Audit log (S7) as an Admin walks them: deciding a proposal (approved, rejected with a reason, the file changed
// first, the swimlane is locked, one's own), following a link from a card to its drawer, and reading the audit log (filter, load
// more, open an event, copy its chain hash). A decision is never shown before the hub has answered, so the lists the page asks
// for again after a write are the hub's next answer: where a journey needs them to differ, it switches the scenario.
import { AUDIT, PENDING, PRIYA, STALE } from "../../src/mocks/design/approvals/fixtures.ts";
import { DIST, allowClipboard, bodyOf, callsTo, clipboard, derive, expect, finish, h1, open, responseOf, sidebar, test, writesOf, type MockContext } from "./support";

test.use({ dist: DIST });

const drawer = (page: import("@playwright/test").Page) => page.getByRole("dialog", { name: "Proposal detail" });
const LIST = responseOf("design-approvals-pending", "GET", "/api/v1/proposals");
const ONE = responseOf("design-approvals-pending", "GET", "/api/v1/proposals/{id}");
const APPROVE = responseOf("design-approvals-pending", "POST", "/api/v1/proposals/{id}/approve");
const REJECT = responseOf("design-approvals-pending", "POST", "/api/v1/proposals/{id}/reject");
const DECIDED_AT = 1_760_000_000_000;

/**
 * A hub that remembers a decision: the design's queue, where a proposal that was approved or rejected is no longer pending and is
 * read back with the status it was given, from the moment the hub has answered the POST. Nothing is optimistic in the page, so
 * what the page shows after a decision is what the next read says.
 */
function deciding(): string {
  const decided = new Map<number, object>();
  const idOf = (context: MockContext): number => Number(context.params["id"]);
  const proposal = (context: MockContext): object => decided.get(idOf(context)) ?? (bodyOf(ONE, context) as object);
  return derive(
    "design-approvals-pending",
    [
      {
        ...LIST,
        body: (context: MockContext) => {
          const answer = bodyOf(LIST, context) as { items: Array<{ id: number }>; next: null };
          return { ...answer, items: answer.items.filter((p) => !decided.has(p.id) || context.query.get("status") !== "pending") };
        },
      },
      { ...ONE, body: proposal },
      {
        ...APPROVE,
        body: (context: MockContext) => {
          decided.set(idOf(context), { ...(bodyOf(ONE, context) as object), status: "approved", decided_by: PRIYA, decided_at: DECIDED_AT });
          return bodyOf(APPROVE, context);
        },
      },
      {
        ...REJECT,
        body: (context: MockContext) => {
          const reason = (context.body as { reason?: string } | undefined)?.reason ?? "";
          const rejected = { ...(bodyOf(ONE, context) as object), status: "rejected", decided_by: PRIYA, decided_at: DECIDED_AT, decision_comment: reason };
          decided.set(idOf(context), rejected);
          return rejected;
        },
      },
    ],
    "journey-approvals-deciding",
  );
}

test.describe("deciding a proposal", () => {
  test.use({ scenario: "design-approvals-pending" });

  test("approve: the drawer says what happens first, writes once, says what the hub answered, and the queue and the Approvals count follow", async ({ page, harness, csp }) => {
    harness.setScenario(deciding());
    await open(page, harness.url, "/approvals", "Approvals");
    const badge = sidebar(page).getByRole("link", { name: /^Approvals 6 pending/ });
    await expect(badge).toBeVisible();
    await page.getByRole("grid", { name: "Proposals" }).getByRole("row", { name: /Ana Ruiz Edit sitb tx-infinity-core-sit7\.yml/ }).click();
    await expect(drawer(page)).toBeVisible();
    await expect(page).toHaveURL(/\/approvals\/2046$/);
    await expect(drawer(page)).toContainText("If you approve");
    expect(writesOf(harness)).toEqual([]);

    // The hub decides: the proposal is no longer pending when the queue is asked for again.
    await drawer(page).getByRole("button", { name: "Approve" }).click();
    await expect(drawer(page).getByText("Approved · written to sitb")).toBeVisible();
    await expect(drawer(page)).toContainText("Audit event 18,470.");
    const posts = callsTo(harness, "POST", "/api/v1/proposals/2046/approve");
    expect(posts).toHaveLength(1);
    expect(posts[0]).toMatchObject({ status: 200, csrf: "ok" });
    expect(posts[0]?.idempotencyKey).toBeTruthy();
    // The queue and the sidebar were asked for again, and now say five.
    await expect(sidebar(page).getByRole("link", { name: /^Approvals 5 pending/ })).toBeVisible();
    await expect(page.getByRole("tab", { name: /^Pending 5/ })).toBeVisible();
    // The buttons are gone with the decision (the proposal is read back as approved): it is not a thing to do twice.
    await expect(drawer(page).getByRole("button", { name: "Approve" })).toHaveCount(0);
    await finish(page, csp, harness);
  });

  test("reject: needs a reason, sends it, and the drawer then says who rejected it and why", async ({ page, harness, csp }) => {
    harness.setScenario(deciding());
    await open(page, harness.url, "/approvals/2046", "Approvals");
    await expect(drawer(page)).toBeVisible();
    const reject = drawer(page).getByRole("button", { name: "Reject" });
    await expect(reject).toBeDisabled();
    await drawer(page).getByRole("textbox", { name: /Reason/ }).fill("Raise maxRetries to 4 first.");
    await expect(reject).toBeEnabled();
    await reject.click();
    await expect.poll(() => callsTo(harness, "POST", "/api/v1/proposals/2046/reject").length).toBe(1);
    const posts = callsTo(harness, "POST", "/api/v1/proposals/2046/reject");
    expect(JSON.parse(posts[0]?.body ?? "{}")).toEqual({ reason: "Raise maxRetries to 4 first." });
    expect(posts[0]).toMatchObject({ status: 200, csrf: "ok" });
    // The drawer says who rejected it and why, from the proposal the hub then sends; its buttons are gone.
    await expect(drawer(page).getByText("Rejected by Priya Shah")).toBeVisible();
    await expect(drawer(page)).toContainText("Raise maxRetries to 4 first.");
    await expect(drawer(page).getByRole("button", { name: "Reject" })).toHaveCount(0);
    await finish(page, csp, harness);
  });

  test("approve is refused because the file changed first (409): nothing was written, the dialog names the new hash and the request, and the proposal stays pending", async ({ page, harness, csp }) => {
    harness.setScenario("design-approvals-conflict");
    await open(page, harness.url, "/approvals/2046", "Approvals");
    await drawer(page).getByRole("button", { name: "Approve" }).click();
    const dialog = page.getByRole("dialog", { name: "The file changed first" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("Nothing was written.");
    await expect(dialog).toContainText("e2a8f610");
    await expect(dialog).toContainText("req-mock-0001");
    expect(callsTo(harness, "POST", "/api/v1/proposals/2046/approve")[0]).toMatchObject({ status: 409 });
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(drawer(page)).toBeVisible();
    await finish(page, csp, harness, { expected: [409] });
  });

  test("approve is refused while the swimlane is locked (423): the dialog says until when, and nothing was approved", async ({ page, harness, csp }) => {
    harness.setScenario("design-approvals-locked");
    await open(page, harness.url, "/approvals/2046", "Approvals");
    await drawer(page).getByRole("button", { name: "Approve" }).click();
    const dialog = page.getByRole("dialog", { name: "Editing is locked" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("sitb is locked, so nothing was approved.");
    await expect(dialog).toContainText(/Locked until \d{4}-\d\d-\d\d \d\d:\d\d UTC/);
    expect(callsTo(harness, "POST", "/api/v1/proposals/2046/approve")[0]).toMatchObject({ status: 423 });
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toHaveCount(0);
    await finish(page, csp, harness, { expected: [423] });
  });

  test("your own proposal can be read but not decided: both buttons are off with the reason, and nothing can be sent", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/approvals/2044", "Approvals");
    await expect(drawer(page)).toBeVisible();
    await expect(drawer(page)).toContainText(/own proposal|your own/i);
    const approve = drawer(page).getByRole("button", { name: "Approve" });
    const reject = drawer(page).getByRole("button", { name: "Reject" });
    await expect(approve).toHaveAttribute("aria-disabled", "true");
    await expect(reject).toHaveAttribute("aria-disabled", "true");
    await approve.click({ force: true });
    await reject.click({ force: true });
    await page.waitForTimeout(300);
    expect(writesOf(harness)).toEqual([]);
    await finish(page, csp, harness);
  });

  test("a link to /proposals/:id (the Teams card, the audit log) opens the queue with that proposal's drawer, on the tab it belongs to", async ({ page, harness, csp }) => {
    await page.goto(`${harness.url}/proposals/2041`);
    await expect(page).toHaveURL(/\/approvals\/2041$/);
    await expect(h1(page)).toHaveText("Approvals");
    await expect(drawer(page)).toBeVisible();
    // 2041 is stale: the queue behind the drawer is on the Stale tab.
    await expect(page.getByRole("tab", { name: /^Stale/ })).toHaveAttribute("aria-selected", "true");
    expect(STALE.some((p) => p.id === 2041)).toBe(true);
    expect(PENDING.some((p) => p.id === 2046)).toBe(true);
    await finish(page, csp, harness);
  });
});

// The audit log with a second page: the first eight events and a cursor, then the rest.
const AUDIT_READ = responseOf("design-approvals-audit", "GET", "/api/v1/admin/audit");
const PAGED = derive(
  "design-approvals-audit",
  [
    // The audit scenarios leave out the queue the sidebar counts for an Admin; the Approvals page's answer is added.
    LIST,
    {
      ...AUDIT_READ,
      body: (context: MockContext) => {
        const all = (bodyOf(AUDIT_READ, context) as { items: Array<{ seq: number }>; next: null }).items;
        const after = context.query.get("cursor");
        return after === null ? { items: all.slice(0, 8), next: "page-2" } : { items: all.slice(8), next: null };
      },
    },
  ],
  "journey-audit-paged",
);

test.describe("the audit log", () => {
  test.use({ scenario: PAGED });

  test("shows the first page, loads more on request (the cursor is the hub's), and an event opens with its chain hash, which copies", async ({ page, harness, context, csp }) => {
    await allowClipboard(context, harness.url);
    await open(page, harness.url, "/audit", "Audit log");
    const table = page.getByRole("grid", { name: "Audit events" });
    await expect(page.getByText("Showing 8 of 8 loaded so far").first()).toBeVisible();
    await expect(table.getByRole("row")).toHaveCount(9);
    // The stream's first connection asks for what is on screen once more: let that settle before paging on.
    await expect(page.locator("[data-slot='live-indicator']")).toHaveAttribute("data-status", "live");
    await page.waitForTimeout(800);
    await page.getByRole("button", { name: "Load more" }).click();
    await expect(page.getByText(/Showing 14 of 14 loaded/).first()).toBeVisible();
    const reads = callsTo(harness, "GET", /^\/api\/v1\/admin\/audit\?/).map((r) => new URL(r.path, harness.url).searchParams.get("cursor"));
    expect(reads.filter((c) => c !== null)).toEqual(["page-2"]);
    await expect(page.getByRole("button", { name: "Load more" })).toHaveCount(0);

    // An event: its drawer has the chain hash of that event, and the button copies exactly it.
    const first = AUDIT[0];
    expect(first).toBeDefined();
    await table.getByRole("row").nth(1).click();
    const event = page.getByRole("dialog", { name: "Event detail" });
    await expect(event).toBeVisible();
    await expect(event).toContainText("Event 18,463");
    await event.getByRole("button", { name: "Copy chain hash" }).click();
    expect(await clipboard(page)).toBe(first?.hash);
    expect(first?.hash).toMatch(/^[0-9a-f]{64}$/);
    await page.keyboard.press("Escape");
    await expect(event).toHaveCount(0);
    await expect(table).toBeFocused();
    await finish(page, csp, harness);
  });

  test("a filter is the address's and the hub's: the user chosen is asked for by id, and the list that comes back is the filtered one", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/audit", "Audit log");
    await page.getByRole("combobox", { name: "User" }).selectOption({ label: "Wen Zhao" });
    await expect(page).toHaveURL(/actor=/);
    await expect.poll(() => callsTo(harness, "GET", /^\/api\/v1\/admin\/audit\?.*actor=/).length).toBeGreaterThan(0);
    const asked = callsTo(harness, "GET", /^\/api\/v1\/admin\/audit\?.*actor=/).map((r) => new URL(r.path, harness.url).searchParams);
    expect(asked.at(-1)?.get("actor")).toMatch(/[0-9a-f-]{20,}/);
    await expect(page.getByRole("grid", { name: "Audit events" }).getByRole("row")).toHaveCount(2);
    await expect(page.getByRole("grid", { name: "Audit events" })).toContainText("Wen Zhao");
    // Clearing goes back to the whole log.
    await page.getByRole("button", { name: "Clear filters" }).click();
    await expect(page).toHaveURL(/\/audit$/);
    await finish(page, csp, harness);
  });
});
