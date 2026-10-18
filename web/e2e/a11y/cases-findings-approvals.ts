// The states of Findings (S6a), Approvals and Audit (S7): the list, its filter menus and drawer, every empty/error/offline/truncated
// answer, the approvals queue and its drawer with the decision answers (applied, 409, 423, a proposal of one's own), the audit log,
// its drawer, filters and a cut chain.
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { fail, hold } from "./helpers.ts";

const frame = (id: number, event: Record<string, unknown>): string => `id: ${id}\nevent: ${String(event["type"])}\ndata: ${JSON.stringify(event)}\n\n`;

const openRow = async (page: Page, grid: string, index: number): Promise<void> => {
  await page.getByRole("grid", { name: grid }).getByRole("row").nth(index).click();
};

export const findingsApprovalsCases: Case[] = [
  // ---- Findings (boards 1a to 1t; States L07, E07, R07, P?, O06, U04, T03) ----------------------------------------------------------
  { id: "findings / eleven open findings", scenario: "design-findings-open", url: "/findings" },
  { id: "findings / filtered by check C8 in sit", scenario: "design-findings-open", url: "/findings?lanes=sit&code=C8" },
  { id: "findings / a viewer", scenario: "design-findings-viewer", url: "/findings" },
  { id: "findings / nothing is open", scenario: "design-findings-empty", url: "/findings" },
  { id: "findings / one finding of each of the twelve checks", scenario: "design-findings-all-kinds", url: "/findings" },
  { id: "findings / an agent is offline (as of its last scan)", scenario: "design-findings-offline", url: "/findings" },
  { id: "findings / more than the screen loads (cut off)", scenario: "design-findings-truncated", url: "/findings" },
  {
    id: "findings / loading (the list is held)",
    scenario: "design-findings-open",
    url: "/findings",
    async before({ page }) {
      await hold(page, /\/api\/v1\/findings/);
    },
  },
  {
    id: "findings / the list fails (500)",
    scenario: "design-findings-open",
    url: "/findings",
    async before({ page }) {
      await fail(page, /\/api\/v1\/findings/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "findings / stale: the list could not be refreshed",
    scenario: "design-findings-open",
    url: "/findings",
    async open({ page, harness }) {
      await expect.poll(() => harness.streamCount(), { timeout: 10_000 }).toBeGreaterThan(0);
      await fail(page, /\/api\/v1\/findings/);
      harness.emit(frame(701, { type: "findings_changed", swimlane: "sitb" }));
      await expect(page.getByText("Couldn't refresh").first()).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    id: "findings / the filter matches nothing",
    scenario: "design-findings-open",
    url: "/findings?lanes=presitc",
  },
  ...["Swimlane", "Check", "Severity"].map(
    (name): Case => ({
      id: `findings / the ${name} filter menu open`,
      scenario: "design-findings-open",
      url: "/findings",
      async open({ page }) {
        await page.getByRole("button", { name, exact: true }).first().click();
        await expect(page.getByRole("menu").or(page.getByRole("dialog")).or(page.getByRole("listbox")).first()).toBeVisible();
      },
    }),
  ),
  {
    id: "findings / the drawer of a finding",
    scenario: "design-findings-open",
    url: "/findings",
    async open({ page }) {
      await openRow(page, "Findings", 1);
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  },
  {
    id: "findings / the drawer, a viewer",
    scenario: "design-findings-viewer",
    url: "/findings",
    async open({ page }) {
      await openRow(page, "Findings", 1);
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  },
  {
    id: "findings / the drawer of a critical finding",
    scenario: "design-findings-all-kinds",
    url: "/findings",
    async open({ page }) {
      await openRow(page, "Findings", 1);
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  },
  { id: "swimlane findings tab / the swimlane's own findings", scenario: "design-findings-open", url: "/swimlanes/sitb/findings" },

  // ---- Approvals (boards 1a to 1u; States L09, E09, R09, P05, U05, C02, T02) ---------------------------------------------------------
  { id: "approvals / pending", scenario: "design-approvals-pending", url: "/approvals" },
  { id: "approvals / nothing in any list", scenario: "design-approvals-empty", url: "/approvals" },
  { id: "approvals / an editor: needs the Admin role", scenario: "design-approvals-editor", url: "/approvals", h1: /needs the Admin role/ },
  {
    id: "approvals / loading (the queue is held)",
    scenario: "design-approvals-pending",
    url: "/approvals",
    async before({ page }) {
      await hold(page, /\/api\/v1\/proposals\?/);
    },
  },
  {
    id: "approvals / the queue fails (500)",
    scenario: "design-approvals-pending",
    url: "/approvals",
    async before({ page }) {
      await fail(page, /\/api\/v1\/proposals\?/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  ...["Stale", "Expired", "Decided"].map(
    (tab): Case => ({
      id: `approvals / the ${tab} tab`,
      scenario: "design-approvals-pending",
      url: "/approvals",
      async open({ page }) {
        await page.getByRole("tab", { name: new RegExp(`^${tab}`) }).click();
        await expect(page.getByRole("tab", { name: new RegExp(`^${tab}`) })).toHaveAttribute("aria-selected", "true");
      },
    }),
  ),
  {
    id: "approvals / a proposal arrives while the page is open",
    scenario: "design-approvals-arrival",
    url: "/approvals",
  },
  {
    id: "approvals / the drawer of a proposal",
    scenario: "design-approvals-pending",
    url: "/approvals",
    async open({ page }) {
      await openRow(page, "Proposals", 1);
      await expect(page.getByRole("dialog", { name: "Proposal detail" })).toBeVisible();
    },
  },
  {
    id: "approvals / the drawer of a proposal of one's own",
    scenario: "design-approvals-pending",
    url: "/approvals",
    async open({ page }) {
      await page.getByRole("grid", { name: "Proposals" }).getByRole("row", { name: /Priya Shah.*You/ }).click();
      await expect(page.getByRole("dialog", { name: "Proposal detail" })).toBeVisible();
    },
  },
  {
    id: "approvals / the drawer, a reason typed to reject",
    scenario: "design-approvals-pending",
    url: "/approvals",
    async open({ page }) {
      await openRow(page, "Proposals", 1);
      await page.getByRole("textbox", { name: /Reason/ }).fill("The retry count is too high for sit7.");
      await expect(page.getByRole("button", { name: "Reject" })).toBeEnabled();
    },
  },
  {
    id: "approvals / approved",
    scenario: "design-approvals-pending",
    url: "/approvals",
    async open({ page }) {
      await openRow(page, "Proposals", 1);
      await page.getByRole("button", { name: "Approve" }).click();
      await page.waitForTimeout(800);
    },
  },
  {
    id: "approvals / approving answers 409 (the file changed first)",
    scenario: "design-approvals-conflict",
    url: "/approvals",
    async open({ page }) {
      await openRow(page, "Proposals", 1);
      await page.getByRole("button", { name: "Approve" }).click();
      await expect(page.getByText(/changed|stale|conflict/i).first()).toBeVisible();
    },
  },
  {
    id: "approvals / approving answers 423 (the swimlane is locked)",
    scenario: "design-approvals-locked",
    url: "/approvals",
    async open({ page }) {
      await openRow(page, "Proposals", 1);
      await page.getByRole("button", { name: "Approve" }).click();
      await expect(page.getByText(/locked/i).first()).toBeVisible();
    },
  },
  { id: "approvals / the proposal page by address", scenario: "design-approvals-pending", url: "/approvals/2046" },

  // ---- Audit (boards 1v to 1ap; States L10, E10, R10, U06) -------------------------------------------------------------------------
  { id: "audit / the log, the chain whole", scenario: "design-approvals-audit", url: "/audit" },
  { id: "audit / the log, a cut link in the chain", scenario: "design-approvals-audit-broken", url: "/audit" },
  { id: "audit / no events", scenario: "design-approvals-audit-empty", url: "/audit" },
  { id: "audit / an editor: needs the Admin role", scenario: "design-approvals-editor", url: "/audit", h1: /needs the Admin role/ },
  {
    id: "audit / loading (the log is held)",
    scenario: "design-approvals-audit",
    url: "/audit",
    async before({ page }) {
      await hold(page, /\/api\/v1\/admin\/audit/);
    },
  },
  {
    id: "audit / the log fails (500)",
    scenario: "design-approvals-audit",
    url: "/audit",
    async before({ page }) {
      await fail(page, /\/api\/v1\/admin\/audit/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "audit / the drawer of an event",
    scenario: "design-approvals-audit",
    url: "/audit",
    async open({ page }) {
      await openRow(page, "Audit events", 1);
      await expect(page.getByRole("dialog")).toBeVisible();
    },
  },
  {
    id: "audit / filtered to one user",
    scenario: "design-approvals-audit",
    url: "/audit",
    async open({ page }) {
      await page.getByRole("combobox", { name: "User" }).selectOption({ index: 2 });
      await page.waitForTimeout(600);
    },
  },
  {
    id: "audit / the quick filter Via Cursor",
    scenario: "design-approvals-audit",
    url: "/audit",
    async open({ page }) {
      await page.getByRole("button", { name: /^Via Cursor/ }).click();
      await expect(page.getByRole("button", { name: /^Via Cursor/ })).toHaveAttribute("aria-pressed", "true");
    },
  },
  {
    id: "audit / the date range, a bad date",
    scenario: "design-approvals-audit",
    url: "/audit",
    async open({ page }) {
      // A date field cannot hold text, so the bad range is a From that is after the To.
      await page.getByLabel("To", { exact: true }).fill("2025-10-01");
      await page.getByLabel("From", { exact: true }).fill("2025-10-10");
      await page.getByLabel("From", { exact: true }).blur();
    },
  },
];
