// Search, the docs reader and the docs writes (S5) as a person walks them: a text hit opens the file at its line, a docs hit opens
// the reader at its heading, and an uploaded doc is uploaded (and opened), replaced (the hub says someone saved first) and deleted.
import type { Page } from "@playwright/test";
import { DIST, callsTo, derive, expect, finish, h1, open, responseOf, responsesOf, test, writesOf, type MockResponse } from "./support";

test.use({ dist: DIST });

// The search scenarios have no file view behind their hits, and the file scenario no search: a hit has to open a file, so the file
// view's answers (for the same Admin and the same six swimlanes) are added to the search scenario's. The search answers are kept.
const FILE_VIEW = responsesOf("design-file-core").filter((r) => !["/api/v1/me", "/api/v1/swimlanes", "/api/v1/proposals", "/api/v1/admin/access-requests", "/auth/logout"].includes(r.path));
const SEARCH_AND_FILES = derive("design-search-admin", FILE_VIEW, "journey-search-files");

test.describe("a text search", () => {
  test.use({ scenario: SEARCH_AND_FILES });

  test("finds a setting, and the hit opens the file at its line with that line marked", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/search?type=text", "Search");
    const query = page.getByRole("searchbox", { name: "Query" });
    await query.fill("timeoutMs");
    // The search goes out a moment after the person stops typing; the answer is a tree of swimlanes, files and lines.
    const tree = page.getByRole("tree", { name: "Results by swimlane and file" });
    await expect(tree).toBeVisible();
    await expect(page.getByText(/26 matches in 14 files/)).toBeVisible();
    expect(new URL(page.url()).searchParams.get("q")).toBe("timeoutMs");
    const asked = callsTo(harness, "GET", /^\/api\/v1\/search\/text\?/).map((r) => new URL(r.path, harness.url).searchParams);
    expect(asked.at(-1)?.get("pattern")).toBe("timeoutMs");
    expect(asked.at(-1)?.get("regex")).toBeNull();

    // The hit of sitb, line 12 of its core file.
    await tree.getByRole("link", { name: "sitb tx-infinity-core.yml line 12", exact: true }).click();
    await expect(page).toHaveURL(/\/swimlanes\/sitb\/files\?line=12&path=tx-infinity-api%2Ftx-infinity-core\.yml$/);
    await expect(h1(page)).toBeVisible();
    const marked = page.locator("[data-highlight='true']");
    await expect(marked).toHaveCount(1);
    await expect(marked).toContainText("12");
    // The marked line is in view, not below the fold.
    const inView = await marked.evaluate((el) => {
      const box = el.getBoundingClientRect();
      return box.top >= 0 && box.bottom <= window.innerHeight;
    });
    expect(inView).toBe(true);
    // Back returns to the search with the query still in the address.
    await page.goBack();
    await expect(page.getByRole("searchbox", { name: "Query" })).toHaveValue("timeoutMs");
    await finish(page, csp, harness);
  });
});

test.describe("the docs", () => {
  test.use({ scenario: "design-search-editor" });

  test("a docs hit opens the reader at the heading of the section it found, with the heading marked", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/search?type=docs", "Search");
    await page.getByRole("searchbox", { name: "Docs query" }).fill("tenant");
    const hit = page.getByRole("link", { name: "Account sorting: Account sorting > Feature Flags" });
    await expect(hit).toBeVisible();
    await expect(hit).toHaveAttribute("href", "/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md#doc-feature-flags");
    await hit.click();
    await expect(page).toHaveURL(/\/docs\/view\?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting\.md#doc-feature-flags$/);
    await expect(page.getByRole("article").getByRole("heading", { level: 2, name: "Feature Flags" })).toBeVisible();
    await expect(page.locator("#doc-feature-flags")).toHaveAttribute("data-lk-target", "");
    await expect(page.locator("#doc-overview")).not.toHaveAttribute("data-lk-target", "");
    // The page the server rendered went through the sanitiser: nothing in it runs or loads, and a Git doc is read-only.
    await expect(page.getByText("Read-only.", { exact: false }).first()).toBeVisible();
    await expect(page.getByRole("button", { name: "Replace" })).toBeDisabled();
    expect(await page.locator("article script, article iframe, article img, article [onclick], article [style]").count()).toBe(0);
    // The table of contents moves to another section of the same page.
    await page.getByRole("navigation", { name: "Table of contents" }).getByRole("link", { name: "Troubleshooting" }).click();
    await expect(page).toHaveURL(/#doc-troubleshooting$/);
    await expect(page.locator("#doc-troubleshooting")).toHaveAttribute("data-lk-target", "");
    await finish(page, csp, harness);
  });
});

// The writes of the docs: upload a file, replace an uploaded doc (the hub says someone saved first, then accepts it), delete it.
const DOC_PUT = responseOf("design-search-editor", "PUT", "/api/v1/docs/{id}");
const CONFLICT: MockResponse = { ...DOC_PUT, status: 409, body: { type: "urn:lanekeeper:problem:conflict", title: "Conflict", status: 409, request_id: "req-mock-0001" } };
const REPLACE_CONFLICTS = derive("design-search-editor", [CONFLICT], "journey-docs-conflict");

test.describe("an uploaded doc", () => {
  test.use({ scenario: "design-search-editor" });

  /** Choose a file in the dialog, as the file chooser would: a name, a type and the bytes. */
  async function upload(page: Page, name: string, text: string, mimeType = "text/markdown"): Promise<void> {
    await page.locator("input[type=file]").setInputFiles({ name, mimeType, buffer: Buffer.from(text) });
  }

  test("is uploaded from a Markdown file (the title comes from it), and the reader then shows it", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/search?type=docs", "Search");
    await page.getByRole("button", { name: "Upload doc" }).click();
    const dialog = page.getByRole("alertdialog", { name: "Upload a doc" });
    await expect(dialog).toBeVisible();
    // A file that is not Markdown is refused where it is chosen, and nothing is sent.
    await upload(page, "notes.txt", "plain", "text/plain");
    await expect(dialog.getByRole("button", { name: "Upload doc" })).toBeDisabled();
    await upload(page, "receipts-v4.md", "# Receipts v4\n\nThe receipt footer is set per channel.\n");
    await expect(dialog.getByLabel("Title")).toHaveValue("Receipts v4");
    expect(writesOf(harness)).toEqual([]);
    await dialog.getByRole("button", { name: "Upload doc" }).click();
    const posts = callsTo(harness, "POST", "/api/v1/docs");
    expect(posts).toHaveLength(1);
    expect(posts[0]).toMatchObject({ status: 201, csrf: "ok" });
    expect(JSON.parse(posts[0]?.body ?? "{}")).toEqual({ title: "Receipts v4", markdown: "# Receipts v4\n\nThe receipt footer is set per channel.\n" });
    // The doc opens in the reader, as the server named it.
    await expect(page).toHaveURL(/\/docs\/view\?path=%2Fuploads%2Freceipts-v4\.md/);
    await expect(h1(page)).toBeVisible();
    await finish(page, csp, harness);
  });

  test("is replaced: the hub says someone else saved first, nothing is lost, and the same replace is accepted when it is tried again", async ({ page, harness, csp }) => {
    harness.setScenario(REPLACE_CONFLICTS);
    await open(page, harness.url, "/docs/view?path=%2Fuploads%2Finstant-payments.md", "Instant payments");
    await page.getByRole("button", { name: "Replace" }).click();
    const dialog = page.getByRole("alertdialog", { name: /^Replace “/ });
    await expect(dialog).toBeVisible();
    await upload(page, "instant-payments.md", "# Instant payments\n\nNew text.\n");
    await dialog.getByRole("button", { name: "Replace doc" }).click();
    // 409: the dialog stays, with the file the person chose, and says what happened.
    await expect(dialog.getByRole("alert")).toContainText("Someone else changed it first");
    await expect(dialog.getByRole("alert")).toContainText("Nothing was saved.");
    await expect(dialog.getByRole("alert")).toContainText("req-mock-0001");
    const puts = callsTo(harness, "PUT", /^\/api\/v1\/docs\/\d+$/);
    expect(puts).toHaveLength(1);
    expect(puts[0]).toMatchObject({ status: 409 });
    expect(JSON.parse(puts[0]?.body ?? "{}")).toMatchObject({ markdown: "# Instant payments\n\nNew text.\n", expected_version: expect.any(Number) });
    // The hub now accepts it (the person read the new text and tried again): the dialog closes.
    harness.setScenario("design-search-editor");
    await dialog.getByRole("button", { name: "Replace doc" }).click();
    await expect(dialog).toHaveCount(0);
    expect(callsTo(harness, "PUT", /^\/api\/v1\/docs\/\d+$/).map((r) => r.status)).toEqual([409, 200]);
    await finish(page, csp, harness, { expected: [409] });
  });

  test("is deleted after its file name is typed (Enter does not confirm), and the person is taken back to the docs", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/docs/view?path=%2Fuploads%2Finstant-payments.md", "Instant payments");
    await page.getByRole("button", { name: "Delete" }).click();
    const dialog = page.getByRole("alertdialog", { name: /^Delete “/ });
    await expect(dialog).toBeVisible();
    const verb = dialog.getByRole("button", { name: "Delete doc" });
    await expect(verb).toHaveAttribute("aria-disabled", "true");
    const field = dialog.getByRole("textbox");
    await field.fill("instant-payments.md");
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    expect(writesOf(harness)).toEqual([]);
    await verb.click();
    await expect(page).toHaveURL(/\/search\?type=docs/);
    const deletes = callsTo(harness, "DELETE", /^\/api\/v1\/docs\/\d+$/);
    expect(deletes).toHaveLength(1);
    expect(deletes[0]).toMatchObject({ status: 204, csrf: "ok" });
    await finish(page, csp, harness);
  });
});
