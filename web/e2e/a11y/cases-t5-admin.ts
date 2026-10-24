// The states of the four Admin sections that T5 adds (slice D): Sentinels (read only, with the alert that names a disconnected VM), OS Login
// users (mappings, with their dialog), Severity rules (the admin's own rules, with their dialog and the "Not applied yet" marker) and
// Retention (the policy, the dry run, the typed confirmation of a policy that deletes, and "never changed"). Each in its list, empty,
// loading, failed, and dialog states.
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { fail, hold, modal } from "./helpers.ts";
import { itemsOf, patchJson } from "./t5.ts";

const problem = (status: number, title: string) => ({ type: "urn:lanekeeper:problem:invalid", title, status, request_id: "req-a11y-0001" });

/** The next write to these addresses is refused, so the dialog shows the answer. */
async function refuseWrites(page: Page, pattern: RegExp, status: number, title: string): Promise<void> {
  await page.route(pattern, (route) => (route.request().method() === "GET" ? route.fallback() : route.fulfill({ status, contentType: "application/problem+json", headers: { "x-request-id": "req-a11y-0001" }, body: JSON.stringify(problem(status, title)) })));
}

const press = (name: string | RegExp) => async ({ page }: { page: Page }): Promise<void> => {
  await page.getByRole("button", { name, exact: typeof name === "string" }).first().click();
  await expect(modal(page).first()).toBeVisible();
};

/** Retention: type the days, ask for the dry run and wait for its answer. */
async function dryRun(page: Page, days = "90", last = "20"): Promise<void> {
  await page.getByRole("textbox", { name: "Days to keep versions" }).fill(days);
  await page.getByRole("textbox", { name: "Versions to always keep per file" }).fill(last);
  await page.getByRole("button", { name: "Run dry run" }).click();
  await expect(page.getByText(/^This policy:/)).toBeVisible();
}

export const t5AdminCases: Case[] = [
  // ---- Sentinels ----------------------------------------------------------------------------------------------------------------------------
  { id: "admin sentinels / six VMs, one disconnected", scenario: "design-admin-t5-sentinels", url: "/admin/sentinels" },
  { id: "admin sentinels / none registered", scenario: "design-admin-t5-sentinels-empty", url: "/admin/sentinels" },
  {
    id: "admin sentinels / loading (the list is held)",
    scenario: "design-admin-t5-sentinels",
    url: "/admin/sentinels",
    async before({ page }) {
      await hold(page, /\/api\/v1\/admin\/sentinels/);
    },
  },
  {
    id: "admin sentinels / the list fails (500)",
    scenario: "design-admin-t5-sentinels",
    url: "/admin/sentinels",
    async before({ page }) {
      await fail(page, /\/api\/v1\/admin\/sentinels/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "admin sentinels / two disconnected and one never seen",
    scenario: "design-admin-t5-sentinels",
    url: "/admin/sentinels",
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/admin\/sentinels/, (list) => ({
        ...list,
        items: itemsOf(list).map((s, i) => (i === 1 ? { ...s, status: "disconnected", last_seen_at: 1_759_999_000_000 } : i === 3 ? { ...s, status: "never_seen", last_seen_at: null } : s)),
      }));
    },
    async open({ page }) {
      await expect(page.getByText(/2 sentinels are disconnected/)).toBeVisible();
    },
  },

  // ---- OS Login users ----------------------------------------------------------------------------------------------------------------------
  { id: "admin oslogin / two mappings", scenario: "design-admin-t5-oslogin", url: "/admin/oslogin" },
  { id: "admin oslogin / no mapping", scenario: "design-admin-t5-oslogin-empty", url: "/admin/oslogin" },
  {
    id: "admin oslogin / loading (the mappings are held)",
    scenario: "design-admin-t5-oslogin",
    url: "/admin/oslogin",
    async before({ page }) {
      await hold(page, /\/api\/v1\/admin\/os-login-users$/);
    },
  },
  {
    id: "admin oslogin / the mappings fail (500)",
    scenario: "design-admin-t5-oslogin",
    url: "/admin/oslogin",
    async before({ page }) {
      await fail(page, /\/api\/v1\/admin\/os-login-users$/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  { id: "admin oslogin / add a mapping (dialog)", scenario: "design-admin-t5-oslogin", url: "/admin/oslogin", open: press("Add mapping") },
  {
    id: "admin oslogin / add a mapping with nothing filled in (both fields in error)",
    scenario: "design-admin-t5-oslogin",
    url: "/admin/oslogin",
    async open({ page }) {
      await press("Add mapping")({ page });
      await modal(page).getByRole("button", { name: "Save mapping" }).click();
      await expect(modal(page).getByText("Choose the user this name stands for")).toBeVisible();
    },
  },
  {
    id: "admin oslogin / add a mapping for a name that is mapped already",
    scenario: "design-admin-t5-oslogin",
    url: "/admin/oslogin",
    async open({ page }) {
      await press("Add mapping")({ page });
      await modal(page).getByRole("textbox", { name: "OS Login username" }).fill("lena_b_corp_example");
      await modal(page).getByRole("combobox", { name: "Lanekeeper user" }).selectOption({ index: 1 });
      await modal(page).getByRole("button", { name: "Save mapping" }).click();
      await expect(modal(page).getByText(/mapped already/)).toBeVisible();
    },
  },
  {
    id: "admin oslogin / the hub refuses the mapping (422)",
    scenario: "design-admin-t5-oslogin",
    url: "/admin/oslogin",
    async before({ page }) {
      await refuseWrites(page, /\/api\/v1\/admin\/os-login-users\/.+/, 422, "That user cannot be mapped");
    },
    async open({ page }) {
      await press("Add mapping")({ page });
      await modal(page).getByRole("textbox", { name: "OS Login username" }).fill("dana_okoye_work");
      await modal(page).getByRole("combobox", { name: "Lanekeeper user" }).selectOption({ index: 1 });
      await modal(page).getByRole("button", { name: "Save mapping" }).click();
      await expect(modal(page).getByRole("alert").or(modal(page).getByText("req-a11y-0001")).first()).toBeVisible();
    },
  },
  { id: "admin oslogin / edit a mapping (the name cannot change)", scenario: "design-admin-t5-oslogin", url: "/admin/oslogin", open: press("Edit mapping marco_diaz_contractor_example") },
  { id: "admin oslogin / delete a mapping", scenario: "design-admin-t5-oslogin", url: "/admin/oslogin", open: press("Delete mapping lena_b_corp_example") },
  {
    id: "admin oslogin / a mapping for a user the list does not have",
    scenario: "design-admin-t5-oslogin",
    url: "/admin/oslogin",
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/admin\/users(\?.*)?$/, (list) => ({ ...list, items: itemsOf(list).slice(0, 1) }));
    },
    async open({ page }) {
      await expect(page.getByText("Unknown user").first()).toBeVisible();
    },
  },

  // ---- Severity rules -------------------------------------------------------------------------------------------------------------------------
  { id: "admin severity / four rules, one off, one not applied yet", scenario: "design-admin-t5-severity", url: "/admin/severity" },
  { id: "admin severity / none of the admin's own", scenario: "design-admin-t5-severity-empty", url: "/admin/severity" },
  {
    id: "admin severity / loading (the rules are held)",
    scenario: "design-admin-t5-severity",
    url: "/admin/severity",
    async before({ page }) {
      await hold(page, /\/api\/v1\/admin\/severity-rules$/);
    },
  },
  {
    id: "admin severity / the rules fail (500)",
    scenario: "design-admin-t5-severity",
    url: "/admin/severity",
    async before({ page }) {
      await fail(page, /\/api\/v1\/admin\/severity-rules$/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  { id: "admin severity / add a rule (dialog: matches everything)", scenario: "design-admin-t5-severity", url: "/admin/severity", open: press("Add rule") },
  {
    id: "admin severity / add a rule with nothing filled in (id and name in error)",
    scenario: "design-admin-t5-severity",
    url: "/admin/severity",
    async open({ page }) {
      await press("Add rule")({ page });
      await modal(page).getByRole("button", { name: "Save rule" }).click();
      await expect(modal(page).getByText("Enter a rule id")).toBeVisible();
    },
  },
  {
    id: "admin severity / add a rule with a setting glob (Not applied yet)",
    scenario: "design-admin-t5-severity",
    url: "/admin/severity",
    async open({ page }) {
      await press("Add rule")({ page });
      await modal(page).getByRole("textbox", { name: "Setting glob" }).fill("spring.datasource.*");
      await expect(modal(page).getByText("Not applied yet")).toBeVisible();
    },
  },
  {
    id: "admin severity / add a rule with a path, a finding type and a change type",
    scenario: "design-admin-t5-severity",
    url: "/admin/severity",
    async open({ page }) {
      await press("Add rule")({ page });
      await modal(page).getByRole("textbox", { name: "Path glob" }).fill("security/**");
      await modal(page).getByRole("checkbox").first().check();
      await modal(page).getByRole("checkbox").nth(14).check();
      await expect(modal(page).getByRole("status", { name: "What this rule matches" })).toContainText("This rule matches:");
    },
  },
  {
    id: "admin severity / the hub refuses the rule (409)",
    scenario: "design-admin-t5-severity",
    url: "/admin/severity",
    async before({ page }) {
      await refuseWrites(page, /\/api\/v1\/admin\/severity-rules\/.+/, 409, "Conflict");
    },
    async open({ page }) {
      await press("Edit rule security-high")({ page });
      await modal(page).getByRole("button", { name: "Save rule" }).click();
      await expect(modal(page).getByRole("alert").or(modal(page).getByText("req-a11y-0001")).first()).toBeVisible();
    },
  },
  { id: "admin severity / edit a rule (the id cannot change)", scenario: "design-admin-t5-severity", url: "/admin/severity", open: press("Edit rule security-high") },
  { id: "admin severity / edit a rule that is not applied yet", scenario: "design-admin-t5-severity", url: "/admin/severity", open: press("Edit rule removed-settings") },
  { id: "admin severity / delete a rule", scenario: "design-admin-t5-severity", url: "/admin/severity", open: press("Delete rule payments-high") },

  // ---- Retention -----------------------------------------------------------------------------------------------------------------------------------
  { id: "admin retention / the saved policy and its dry run", scenario: "design-admin-t5-retention", url: "/admin/retention" },
  { id: "admin retention / never changed (the defaults apply, no dry run yet)", scenario: "design-admin-t5-retention-defaults", url: "/admin/retention" },
  {
    id: "admin retention / loading (the policy is held)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async before({ page }) {
      await hold(page, /\/api\/v1\/admin\/retention$/);
    },
  },
  {
    id: "admin retention / the policy fails (500)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async before({ page }) {
      await fail(page, /\/api\/v1\/admin\/retention$/);
    },
    async open({ page }) {
      await expect(page.getByText("req-a11y-0001").first()).toBeVisible();
    },
  },
  {
    id: "admin retention / values that are not allowed (both fields in error)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async open({ page }) {
      await page.getByRole("textbox", { name: "Days to keep versions" }).fill("3");
      await page.getByRole("textbox", { name: "Versions to always keep per file" }).fill("x");
      await page.getByRole("button", { name: "Run dry run" }).click();
      await expect(page.getByText("Minimum is 30 days")).toBeVisible();
    },
  },
  {
    id: "admin retention / a dry run for new values (Save is on)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async open({ page }) {
      await dryRun(page);
      await expect(page.getByRole("button", { name: "Save policy" })).toBeEnabled();
    },
  },
  {
    id: "admin retention / values changed after the dry run (run it again)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async open({ page }) {
      await dryRun(page);
      await page.getByRole("textbox", { name: "Days to keep versions" }).fill("120");
      await expect(page.getByText(/Run it again before you save/)).toBeVisible();
    },
  },
  {
    id: "admin retention / a policy that deletes: type the words to save (dialog)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async open({ page }) {
      await dryRun(page);
      await page.getByRole("button", { name: "Save policy" }).click();
      await expect(page.getByRole("alertdialog", { name: /Save a policy that deletes versions/ })).toBeVisible();
    },
  },
  {
    id: "admin retention / the typed words match (Save and delete is on)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async open({ page }) {
      await dryRun(page);
      await page.getByRole("button", { name: "Save policy" }).click();
      await modal(page).getByRole("textbox").fill("delete versions");
      await expect(modal(page).getByRole("button", { name: "Save and delete" })).toBeEnabled();
    },
  },
  {
    id: "admin retention / a policy that deletes nothing now (a plain confirmation)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/admin\/retention(\?.*)?$/, (policy, url) => (url.searchParams.get("dry_run") === "true" ? { ...policy, dry_run_preview: { versions_to_delete: 0, bytes_to_free: 0, versions_kept_by_reference: 318 } } : policy));
    },
    async open({ page }) {
      await page.getByRole("textbox", { name: "Days to keep versions" }).fill("400");
      await page.getByRole("button", { name: "Run dry run" }).click();
      await expect(page.getByText(/^This policy:/)).toBeVisible();
      await page.getByRole("button", { name: "Save policy" }).click();
      await expect(page.getByRole("alertdialog", { name: "Save the retention policy" })).toBeVisible();
    },
  },
  {
    id: "admin retention / a dry run with very large numbers",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/admin\/retention(\?.*)?$/, (policy, url) =>
        url.searchParams.get("dry_run") === "true" ? { ...policy, dry_run_preview: { versions_to_delete: 12_345_678_901, bytes_to_free: 7_300_000_000_000_000, versions_kept_by_reference: 987_654_321 } } : policy,
      );
    },
    async open({ page }) {
      await page.getByRole("textbox", { name: "Days to keep versions" }).fill("30");
      await page.getByRole("button", { name: "Run dry run" }).click();
      await expect(page.getByText("12,345,678,901")).toBeVisible();
    },
  },
  {
    id: "admin retention / the hub sent no preview for the policy",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async before({ page }) {
      await patchJson(page, /\/api\/v1\/admin\/retention(\?.*)?$/, (policy, url) => (url.searchParams.get("dry_run") === "true" ? { ...policy, dry_run_preview: null } : policy));
    },
    async open({ page }) {
      await page.getByRole("textbox", { name: "Days to keep versions" }).fill("90");
      await page.getByRole("button", { name: "Run dry run" }).click();
      await expect(page.getByText("The hub sent no preview for this policy")).toBeVisible();
    },
  },
  {
    id: "admin retention / the dry run fails (503)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async before({ page }) {
      await page.route(/\/api\/v1\/admin\/retention\?dry_run=true/, (route) => route.fulfill({ status: 503, contentType: "application/problem+json", headers: { "x-request-id": "req-a11y-0001" }, body: JSON.stringify(problem(503, "Service unavailable")) }));
    },
    async open({ page }) {
      await page.getByRole("textbox", { name: "Days to keep versions" }).fill("90");
      await page.getByRole("button", { name: "Run dry run" }).click();
      await expect(page.getByText(/dry run could not be run|could not run the dry run/i).first()).toBeVisible();
    },
  },
  {
    id: "admin retention / saving is refused because someone else changed the policy (409)",
    scenario: "design-admin-t5-retention",
    url: "/admin/retention",
    async before({ page }) {
      await page.route(/\/api\/v1\/admin\/retention$/, (route) => (route.request().method() === "PUT" ? route.fulfill({ status: 409, contentType: "application/problem+json", headers: { "x-request-id": "req-a11y-0001" }, body: JSON.stringify(problem(409, "Conflict")) }) : route.fallback()));
    },
    async open({ page }) {
      await dryRun(page);
      await page.getByRole("button", { name: "Save policy" }).click();
      await modal(page).getByRole("textbox").fill("delete versions");
      await modal(page).getByRole("button", { name: "Save and delete" }).click();
      await expect(modal(page).getByText(/Someone else changed the policy/)).toBeVisible();
    },
  },
];
