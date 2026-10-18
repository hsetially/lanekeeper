// The states of Compare (S4), Search, the doc reader and Feature status (S5): grid, drawer, whole-swimlane trees, pickers, text, settings
// and docs search with their empty, error, truncated and invalid answers, the reader (a Git doc, an uploaded doc, a doc with its own h1),
// upload, replace and delete dialogs, and the status table.
import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { fail, hold, modal } from "./helpers.ts";

const CORE = "tx-infinity-api%2Ftx-infinity-core.yml";
const GRID = `/compare?file=${CORE}&lanes=sit,sitb,sitc`;
const GIT_DOC = "/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md";
const UPLOADED_DOC = "/docs/view?path=%2Fuploads%2Finstant-payments.md";

/** The doc the server renders, with a heading of level 1 of its own (the sanitiser's allowlist lets `<h1>` through). */
export async function docWithItsOwnH1(page: Page): Promise<void> {
  await page.route(/\/api\/v1\/docs\/fs\/render/, async (route) => {
    const response = await route.fetch();
    // A revalidation answers 304 with no body: pass it on as it is.
    if (response.status() !== 200) {
      await route.fulfill({ response });
      return;
    }
    const body = (await response.json()) as { html: string; headings: Array<{ id: string; level: number; text: string; line: number }> };
    body.html = `<h1 id="doc-title">Account sorting, the title in the document</h1>${body.html}`;
    body.headings = [{ id: "doc-title", level: 1, text: "Account sorting, the title in the document", line: 1 }, ...body.headings];
    await route.fulfill({ response, json: body });
  });
}

export const compareSearchCases: Case[] = [
  // ---- Compare: one file across swimlanes (boards 1a to 1f), the picker, the drawer ------------------------------------------------------
  { id: "compare / choose a file", scenario: "design-compare-default", url: "/compare" },
  { id: "compare / the grid, three swimlanes", scenario: "design-compare-default", url: GRID },
  { id: "compare / the grid, six swimlanes", scenario: "design-compare-default", url: `/compare?file=${CORE}&lanes=sit,sitb,sitc,presita,presitb,presitc` },
  { id: "compare / the grid, an agent offline (as of its last scan)", scenario: "design-compare-offline", url: `/compare?file=${CORE}&lanes=sit,sitb,presitb` },
  { id: "compare / the grid, all rows (not only differing)", scenario: "design-compare-default", url: `${GRID}&only=all` },
  { id: "compare / the grid, the swimlanes agree: no row", scenario: "design-compare-empty", url: GRID },
  { id: "compare / more swimlanes than the grid takes (43)", scenario: "design-compare-toomany", url: `/compare?file=${CORE}&lanes=${["sit", "sitb", "sitc", "presita", "presitb", "presitc"].join(",")}` },
  {
    id: "compare / loading (the grid is held)",
    scenario: "design-compare-default",
    url: GRID,
    async before({ page }) {
      await hold(page, /\/api\/v1\/grid/);
    },
  },
  { id: "compare / the grid cannot be read (500)", scenario: "design-compare-error", url: GRID },
  {
    id: "compare / two cells picked: the diff drawer",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "absent (differs)" }).first().click();
      await page.getByRole("button", { name: "true", exact: true }).first().click();
      await expect(page.getByRole("button", { name: "Hide diff" })).toBeVisible();
    },
  },
  {
    id: "compare / the logical file picker open",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: /^Logical file/ }).click();
      await expect(page.getByRole("button", { name: /^Logical file/ })).toHaveAttribute("aria-expanded", "true");
    },
  },
  {
    id: "compare / choose swimlanes dialog",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "3 swimlanes" }).click();
      await expect(page.getByRole("dialog", { name: "Choose swimlanes" })).toBeVisible();
    },
  },
  {
    id: "compare / the channel menu",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("combobox", { name: "Channel" }).focus();
      await expect(page.getByRole("combobox", { name: "Channel" })).toBeFocused();
    },
  },
  {
    id: "compare / a setting search with no match",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("searchbox", { name: "Search settings" }).fill("zzzzzz");
    },
  },

  // ---- Compare: whole swimlane (boards 2a to 2i) ------------------------------------------------------------------------------------------
  {
    id: "compare whole swimlane / sit against sitb, a file chosen",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "Whole swimlane" }).click();
      await expect(page.getByRole("tree", { name: "Differing files" })).toBeVisible();
    },
  },
  {
    id: "compare whole swimlane / a binary file selected",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "Whole swimlane" }).click();
      await page.getByRole("treeitem", { name: /receipt-logo\.jpeg/ }).click();
      await expect(page.getByRole("tree", { name: "Differing files" })).toBeVisible();
    },
  },
  {
    id: "compare whole swimlane / unified diff",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "Whole swimlane" }).click();
      await page.getByRole("radio", { name: "Unified view" }).click();
      await expect(page.getByRole("radio", { name: "Unified view" })).toBeChecked();
    },
  },
  {
    id: "compare whole swimlane / against a branch",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "Whole swimlane" }).click();
      await page.getByRole("radiogroup", { name: "Compare against" }).getByRole("radio").nth(1).click();
      await expect(page.getByRole("radiogroup", { name: "Compare against" }).getByRole("radio").nth(1)).toBeChecked();
    },
  },
  {
    id: "compare whole swimlane / the left side picker",
    scenario: "design-compare-default",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "Whole swimlane" }).click();
      await page.getByRole("button", { name: /^Left side/ }).click();
      await expect(page.getByRole("button", { name: /^Left side/ })).toHaveAttribute("aria-expanded", "true");
    },
  },
  {
    id: "compare whole swimlane / nothing differs",
    scenario: "design-compare-identical",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "Whole swimlane" }).click();
      await expect(page.getByText(/identical|Nothing differs|no difference/i).first()).toBeVisible();
    },
  },
  {
    id: "compare whole swimlane / the comparison fails (500)",
    scenario: "design-compare-error",
    url: GRID,
    async open({ page }) {
      await page.getByRole("button", { name: "Whole swimlane" }).click();
      await expect(page.getByText("req-mock-0001").first()).toBeVisible();
    },
  },

  // ---- Search: text, settings, docs (boards 1a to 1x; States L05, E05, R05, T01) --------------------------------------------------
  { id: "search / text, hits", scenario: "design-search-admin", url: "/search?type=text&q=enableAccountSorting" },
  { id: "search / text, no query yet", scenario: "design-search-admin", url: "/search?type=text" },
  { id: "search / text, a regular expression", scenario: "design-search-admin", url: "/search?type=text&q=enable.*Sorting&rx=1" },
  { id: "search / text, more hits than the server sends (truncated)", scenario: "design-search-truncated", url: "/search?type=text&q=enableAccountSorting" },
  { id: "search / text, an invalid pattern (400)", scenario: "design-search-invalid", url: "/search?type=text&q=(&rx=1" },
  { id: "search / text, nothing found", scenario: "design-search-empty", url: "/search?type=text&q=enableAccountSorting" },
  { id: "search / text, the search fails (500)", scenario: "design-search-error", url: "/search?type=text&q=enableAccountSorting" },
  {
    id: "search / text, loading (the search is held)",
    scenario: "design-search-admin",
    url: "/search?type=text&q=enableAccountSorting",
    async before({ page }) {
      await hold(page, /\/api\/v1\/search\/text/);
    },
  },
  {
    id: "search / the swimlane picker open",
    scenario: "design-search-admin",
    url: "/search?type=text&q=enableAccountSorting",
    async open({ page }) {
      await page.getByRole("button", { name: /^Swimlanes/ }).click();
      await expect(page.getByRole("dialog").or(page.getByRole("menu")).or(page.getByRole("listbox")).first()).toBeVisible();
    },
  },
  { id: "search / settings, hits", scenario: "design-search-admin", url: "/search?type=settings&q=enableAccountSorting" },
  { id: "search / settings, nothing found", scenario: "design-search-empty", url: "/search?type=settings&q=enableAccountSorting" },
  { id: "search / settings, the search fails (500)", scenario: "design-search-error", url: "/search?type=settings&q=enableAccountSorting" },
  { id: "search / docs, hits", scenario: "design-search-admin", url: "/search?type=docs&q=sorting" },
  { id: "search / docs, an editor (Upload doc is on)", scenario: "design-search-editor", url: "/search?type=docs&q=sorting" },
  { id: "search / docs, a viewer (Upload doc is off, says which role)", scenario: "design-search-viewer", url: "/search?type=docs&q=sorting" },
  { id: "search / docs, nothing found", scenario: "design-search-empty", url: "/search?type=docs&q=sorting" },
  { id: "search / docs, the search fails (500)", scenario: "design-search-error", url: "/search?type=docs&q=sorting" },
  {
    id: "search / docs, the upload dialog",
    scenario: "design-search-editor",
    url: "/search?type=docs&q=sorting",
    async open({ page }) {
      await page.getByRole("button", { name: "Upload doc" }).click();
      await expect(modal(page)).toBeVisible();
    },
  },

  // ---- The doc reader (boards 1u to 1x) ------------------------------------------------------------------------------------------------
  { id: "doc reader / a doc from Git (read-only)", scenario: "design-search-admin", url: GIT_DOC },
  { id: "doc reader / an uploaded doc (editor)", scenario: "design-search-editor", url: UPLOADED_DOC },
  { id: "doc reader / an uploaded doc (viewer: replace and delete are off)", scenario: "design-search-viewer", url: UPLOADED_DOC },
  {
    // A rendered doc may carry its own <h1>; the page must still have exactly one.
    id: "doc reader / a doc that has a heading of level 1 of its own",
    scenario: "design-search-admin",
    url: GIT_DOC,
    async before({ page }) {
      await docWithItsOwnH1(page);
    },
    async open({ page }) {
      await expect(page.getByRole("heading", { name: "Account sorting, the title in the document" })).toBeVisible();
    },
  },
  {
    id: "doc reader / loading (the page is held)",
    scenario: "design-search-admin",
    url: GIT_DOC,
    async before({ page }) {
      await hold(page, /\/api\/v1\/docs\/fs\/render/);
    },
  },
  {
    id: "doc reader / the doc cannot be read (500)",
    scenario: "design-search-admin",
    url: GIT_DOC,
    async before({ page }) {
      await fail(page, /\/api\/v1\/docs\/fs\/render/);
    },
  },
  {
    id: "doc reader / the replace dialog",
    scenario: "design-search-editor",
    url: UPLOADED_DOC,
    async open({ page }) {
      await page.getByRole("button", { name: /^Replace/ }).click();
      await expect(modal(page)).toBeVisible();
    },
  },
  {
    id: "doc reader / the delete dialog",
    scenario: "design-search-editor",
    url: UPLOADED_DOC,
    async open({ page }) {
      await page.getByRole("button", { name: /^Delete/ }).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
    },
  },

  // ---- Feature status (States L06, E06, R06, O05, U03) ------------------------------------------------------------------------------------
  { id: "status / flags, an offline swimlane among them", scenario: "design-search-admin", url: "/status" },
  { id: "status / flags, a viewer", scenario: "design-search-viewer", url: "/status" },
  { id: "status / nothing to show", scenario: "design-search-empty", url: "/status" },
  { id: "status / the table cannot be read (500)", scenario: "design-search-error", url: "/status" },
  { id: "status / a setting and a swimlane named in the address", scenario: "design-search-admin", url: "/status?setting=txInfinityOptions.remote-itm-teller.enableAccountSorting&lane=sitb" },
  {
    id: "status / channels shown",
    scenario: "design-search-admin",
    url: "/status",
    async open({ page }) {
      await page.getByRole("button", { name: "Show channels" }).click();
      await expect(page.getByRole("button", { name: "Hide channels" })).toBeVisible();
    },
  },
  {
    id: "status / only the flags that differ",
    scenario: "design-search-admin",
    url: "/status",
    async open({ page }) {
      await page.getByRole("switch", { name: "Only flags that differ" }).click();
      await expect(page.getByRole("switch", { name: "Only flags that differ" })).toBeChecked();
    },
  },
  {
    id: "status / loading (the table is held)",
    scenario: "design-search-admin",
    url: "/status",
    async before({ page }) {
      await hold(page, /\/api\/v1\/features\/status/);
    },
  },
];
