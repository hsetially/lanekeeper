// The things a frame of the four T5 sections of Admin needs a person to do: open a dialog, fill it, press its button, run a dry run. Shared by
// admin-t5/frames.ts (the states of each section) and admin/frames.ts (the design's frames 1ao to 1bm, which are these sections' own).
import { expect } from "../../fixtures/test";
import type { Page } from "@playwright/test";
import { problemBody } from "../../../src/mocks/scenario.ts";
import type { Step } from "../search/kit";

export const dialog = (page: Page) => page.getByRole("alertdialog");
const textbox = (page: Page, name: string) => dialog(page).getByRole("textbox", { name, exact: true });
const then = (...steps: Step[]): Step => async (env) => {
  for (const step of steps) await step(env);
};
export { then };

/** The status of a section's answer the hub refuses: a problem with a request id, for the route that matches. */
export const refuse = (pattern: RegExp, method: string, status: number, title: string, code = "unprocessable"): Step => async ({ page }) => {
  await page.route(pattern, (route) => {
    if (route.request().method() !== method) return route.fallback();
    return route.fulfill({ status, contentType: "application/problem+json", headers: { "x-request-id": "req-mock-0001" }, body: JSON.stringify(problemBody(status, code, title)) });
  });
};

// ----- OS Login ---------------------------------------------------------------------------------------------------------------------

export const osLoginPut = (status: number, title: string): Step => refuse(new RegExp(`/api/v1/admin/os-login-users/[^/?]+$`), "PUT", status, title);

export const addMapping: Step = async ({ page }) => {
  await page.getByRole("button", { name: "Add mapping" }).click();
  await dialog(page).getByText("OS Login username").first().waitFor();
};
export const fillMapping = (login: string, user: number | null): Step => async ({ page }) => {
  await textbox(page, "OS Login username").fill(login);
  if (user !== null) await dialog(page).getByRole("combobox", { name: "Lanekeeper user" }).selectOption({ index: user });
};
export const editMapping: Step = async ({ page }) => {
  await page.getByRole("button", { name: /^Edit mapping marco_diaz/ }).click();
  await dialog(page).getByText("The name cannot change").waitFor();
};
export const deleteMapping: Step = async ({ page }) => {
  await page.getByRole("button", { name: /^Delete mapping lena_b/ }).click();
  await dialog(page).waitFor();
};
export const saveMapping: Step = async ({ page }) => {
  await dialog(page).getByRole("button", { name: "Save mapping" }).click();
};
export const mappingSaved: Step = async ({ page }) => {
  await page.getByText(/Mapping .* saved/).first().waitFor();
};

// ----- severity rules ---------------------------------------------------------------------------------------------------------------

export const addRule: Step = async ({ page }) => {
  await page.getByRole("button", { name: "Add rule" }).click();
  await dialog(page).getByText("Finding types").waitFor();
};
export const editRule = (id: RegExp): Step => async ({ page }) => {
  await page.getByRole("button", { name: id }).click();
  await dialog(page).getByText("Finding types").waitFor();
};
export const fillRule = (fields: { id?: string; name?: string; path?: string; setting?: string }): Step => async ({ page }) => {
  if (fields.id !== undefined) await textbox(page, "Rule id").fill(fields.id);
  if (fields.name !== undefined) await textbox(page, "Name").fill(fields.name);
  if (fields.path !== undefined) await textbox(page, "Path glob").fill(fields.path);
  if (fields.setting !== undefined) await textbox(page, "Setting glob").fill(fields.setting);
};
export const checkKind = (name: string): Step => async ({ page }) => {
  await dialog(page).getByRole("checkbox", { name }).check();
};
export const saveRule: Step = async ({ page }) => {
  await dialog(page).getByRole("button", { name: "Save rule" }).click();
};
export const ruleSaved: Step = async ({ page }) => {
  await page.getByText(/^Rule .* saved|Rules? saved|saved/).first().waitFor();
};
export const invalidInDialog: Step = async ({ page }) => {
  await dialog(page).locator("[aria-invalid='true']").first().waitFor();
};

// ----- retention --------------------------------------------------------------------------------------------------------------------

const days = (page: Page) => page.getByRole("textbox", { name: "Days to keep versions", exact: true });
const last = (page: Page) => page.getByRole("textbox", { name: "Versions to always keep per file", exact: true });
export const setPolicy = (keepDays: string, keepLast: string): Step => async ({ page }) => {
  await days(page).fill(keepDays);
  await last(page).fill(keepLast);
};
export const dryRun: Step = async ({ page }) => {
  await page.getByRole("button", { name: "Run dry run" }).click();
};
export const dryRunShown: Step = async ({ page }) => {
  await page.getByText("This policy:").waitFor();
};
export const savePolicy: Step = async ({ page }) => {
  await page.getByRole("button", { name: "Save policy" }).click();
  await dialog(page).waitFor();
};
export const typeConfirm: Step = async ({ page }) => {
  await dialog(page).getByRole("textbox").fill("delete versions");
};
export const confirmSave: Step = async ({ page }) => {
  await dialog(page).getByRole("button", { name: /^Save/ }).click();
  await dialog(page).waitFor({ state: "detached" });
  await expect(page.getByText("Retention policy saved")).toBeVisible();
};

/** The dry run finds nothing to delete: the hub's answer to `PUT ?dry_run=true` has zero versions. */
export const nothingToDelete: Step = async ({ page }) => {
  await page.route(/\/api\/v1\/admin\/retention\?.*dry_run=true/, async (route) => {
    if (route.request().method() !== "PUT") return route.fallback();
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    return route.fulfill({ response, json: { ...body, dry_run_preview: { versions_to_delete: 0, bytes_to_free: 0, versions_kept_by_reference: 318 } } });
  });
};
/** The hub answers the dry run with no preview at all. */
export const noPreview: Step = async ({ page }) => {
  await page.route(/\/api\/v1\/admin\/retention\?.*dry_run=true/, async (route) => {
    if (route.request().method() !== "PUT") return route.fallback();
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    return route.fulfill({ response, json: { ...body, dry_run_preview: null } });
  });
};

/**
 * The table's sticky header (z-10, VirtualList) is inside the screen's box, which is a stacking context of its own, so the header's layer cannot
 * reach past it into the dialog's. Without that the header paints over the dialog's text where the two overlap (the severity rule dialog is taller
 * than the rows behind it) while the shell's header and sidebar, which sit above the scrim on purpose, stay as the design draws them. <main> itself
 * is NOT a stacking context: the menus, popovers and tooltips are portaled into it, and a context there put them under the sidebar (the account
 * menu's items could not be clicked: e2e/csp/shell.spec.ts).
 */
export const tableHeaderStaysUnderDialog: Step = async ({ page }) => {
  await expect(page.locator("main [data-slot='virtual-header']").first()).toBeAttached();
  expect(await page.locator("main > div").first().evaluate((el) => getComputedStyle(el).isolation), "the screen's box is a stacking context").toBe("isolate");
  expect(await page.locator("main").evaluate((el) => getComputedStyle(el).isolation), "<main> is not one").toBe("auto");
};
