// The frames of the doc reader and the upload, replace and delete dialogs: the board frames 1k to 1m and 1o to 1t of
// "Lanekeeper Search and Status.dc.html".
//
//  - 1k, 1l, 1m (the reader) are app baselines: the design draws the reader inside the Search tab (a page title, the tabs, a "Results for"
//    strip, related files and a version history), and the app has its own routes and drops those (design/DEVIATIONS.md: "Doc reader",
//    "Doc reader: images").
//  - 1o is tier 1 on the toolbar (the rest is the docs list of 1j).
//  - 1p to 1t are the dialogs, app baselines: the design draws them 520 px wide at the top of the page region, the shared consequence
//    dialog is 480 px wide (sizes 400, 480 and 600) and centred over the viewport, and the design's consequences differ from the app's
//    (design/DEVIATIONS.md: "Doc upload and replace"). Measured: 480 by 292 against 520 by 300 (1p), 480 by 344 against 520 by 363
//    (1q, 1r), 480 by 414 against 520 by 484 (1s, 1t). The width and position are not in DEVIATIONS.md yet: reported to the integrator.
import type { Page } from "@playwright/test";
import { HIDDEN_NAV, SWITCHER, type FrameDef, type Mask } from "../search/kit";

const LIST = "/search?type=docs&q=account%20sorting";
const GIT_DOC = "/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md";
const UPLOADED_DOC = "/docs/view?path=%2Fuploads%2Finstant-payments.md";
const DIALOG = "the shared consequence dialog (480 px, centred over the viewport) against the design's 520 px dialog at the top of the page region, and the design's consequence list (design/DEVIATIONS.md: \"Doc upload and replace\"; the width is not in the file yet)";
const READER = "the app's own reader: the design draws it inside the Search tab with related files and a version history (design/DEVIATIONS.md: \"Doc reader\")";

const DOCS_NOTE: Mask = { x: 385, y: 208, width: 340, height: 28, row: "Search: docs (\"Best matches first, from titles, headings and text\")" };
const DOCS_ROWS: Mask = { x: 249, y: 238, width: 1166, height: 646, row: "Search: docs (the source chip, the anchors with the hub's doc- prefix, the snippet marks)" };

const upload = (page: Page, name: string, bytes: number, text?: string) =>
  page.locator("input[type='file']").setInputFiles({ name, mimeType: name.endsWith(".md") ? "text/markdown" : "application/pdf", buffer: Buffer.from(text ?? "x".repeat(bytes)) });

/** The open dialog (the consequence dialogs are alert dialogs). */
const dialog = (page: Page) => page.locator("[role='dialog'], [role='alertdialog']").first();

const openUpload = async (page: Page): Promise<void> => {
  await page.getByRole("button", { name: "Upload doc" }).first().click();
  await dialog(page).waitFor();
};

const MARKDOWN = `# Instant payments\n\n## Feature Flags\n\n| Flag | Channel |\n|---|---|\n| enableInstantPayments | atm-iso |\n\n\`\`\`yaml\nfeatureFlags:\n  instantPayments: true\n\`\`\`\n\n## Limits\n\nLimits are set per entry group.\n`;

export const frames: FrameDef[] = [
  {
    board: "search", frame: "1k", kind: "app", scenario: "design-search-editor", path: GIT_DOC, title: "reader, Git doc, Replace off with the reason", why: READER,
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Replace" }).hover();
    },
  },
  { board: "search", frame: "1l", kind: "app", scenario: "design-search-editor", path: UPLOADED_DOC, title: "reader, uploaded doc, Replace on", why: READER },
  { board: "search", frame: "1m", kind: "app", scenario: "design-search-admin", path: GIT_DOC, title: "reader, Git doc", why: READER },
  {
    board: "search", frame: "1o", kind: "tier1", scenario: "design-search-viewer", path: LIST, title: "Viewer: Upload is off, with the role it needs",
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Upload doc" }).hover();
    },
    masks: [SWITCHER, DOCS_NOTE, DOCS_ROWS, HIDDEN_NAV, { x: 1170, y: 200, width: 240, height: 30, row: "Permission state (\"Ask an Admin\": the contract cannot name the Admin who can grant the role, plan CG14)" }],
  },
  {
    board: "search", frame: "1p", kind: "app", scenario: "design-search-admin", path: LIST, title: "upload, drop zone ready",
    steps: async ({ page }) => openUpload(page),
    why: DIALOG,
  },
  {
    board: "search", frame: "1q", kind: "app", scenario: "design-search-admin", path: LIST, title: "upload, wrong type",
    steps: async ({ page }) => {
      await openUpload(page);
      await upload(page, "retail-notes.pdf", 412 * 1024);
      await dialog(page).getByText(/Only Markdown/).waitFor();
    },
    why: DIALOG,
  },
  {
    board: "search", frame: "1r", kind: "app", scenario: "design-search-admin", path: LIST, title: "upload, over 2 MB",
    steps: async ({ page }) => {
      await openUpload(page);
      await upload(page, "export-all-flags.md", 3_400_000);
      await dialog(page).getByText(/over the 2 MB/).waitFor();
    },
    why: DIALOG,
  },
  {
    board: "search", frame: "1s", kind: "app", scenario: "design-search-editor", path: UPLOADED_DOC, title: "replace, consequences listed before the button",
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Replace" }).click();
      await dialog(page).waitFor();
      await upload(page, "instant-payments.md", 0, MARKDOWN);
      await dialog(page).getByText(/Ready/).waitFor();
    },
    why: DIALOG,
  },
  {
    board: "search", frame: "1t", kind: "app", scenario: "design-search-editor", path: UPLOADED_DOC, title: "replace",
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Replace" }).click();
      await dialog(page).waitFor();
      await upload(page, "instant-payments.md", 0, MARKDOWN);
      await dialog(page).getByText(/Ready/).waitFor();
    },
    why: DIALOG,
  },
];

const APP = "a state of the app's own dialog that the design has no frame for (design/DEVIATIONS.md: \"Doc delete\", \"Doc upload and replace\")";

// States the board has no frame for, as app-owned baselines: delete (a type-the-name confirmation), a replace that meets a 409, and the
// reader for a doc that is not there.
export const states: FrameDef[] = [
  {
    board: "states", frame: "docs-delete", kind: "state", scenario: "design-search-editor", path: UPLOADED_DOC, title: "Docs, delete confirmation (type the name)", why: APP,
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Delete" }).click();
      await dialog(page).waitFor();
    },
  },
  {
    board: "states", frame: "docs-replace-409", kind: "state", scenario: "design-search-editor", path: UPLOADED_DOC, title: "Docs, replace answers 409: someone saved first", why: APP,
    before: async ({ page }) => {
      await page.route(/\/api\/v1\/docs\/\d+$/, (route) => (route.request().method() === "PUT" ? route.fulfill({ status: 409, contentType: "application/problem+json", headers: { "x-request-id": "req-mock-0001" }, body: JSON.stringify({ type: "urn:lanekeeper:problem:conflict", title: "Conflict", status: 409, code: "conflict" }) }) : route.fallback()));
    },
    steps: async ({ page }) => {
      await page.getByRole("button", { name: "Replace" }).click();
      await dialog(page).waitFor();
      await upload(page, "instant-payments.md", 0, MARKDOWN);
      await dialog(page).getByText(/Ready/).waitFor();
      await dialog(page).getByRole("button", { name: "Replace doc" }).click();
      await dialog(page).getByText(/Someone else changed it first/).waitFor();
    },
  },
];
