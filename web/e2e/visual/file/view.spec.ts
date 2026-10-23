// Tier 1, the file view: board "File View" frames 1a to 1o against the app on the `design-file-*` scenarios, 1440x900.
//
// What is masked, and why (each a row of design/DEVIATIONS.md, named here by its first column):
//  - the facts row under the title (path, CRLF, UTF-8, lines, hash, and who changed the file at its end): "Copy affordances the design
//    does not draw" (a link to the file as well as the path and the hash) and "File header and History" (no "CRLF - Git has LF"; who
//    changed the file, and its pull request badge beside the drift state, which the design's header does not draw).
//  - the Edit button: "File: Edit" (off while the server masks a value in the file; the design edits it).
//  - from the tab row down, where the page is the file's body: "File viewer and diff (read-only)" (CodeView and DiffView, a gutter of
//    their own, not Monaco), "File: Diff and large files" (no outline, no minimap, no truncation notice), "File: view" (no Rendered,
//    or As served) and the "History" and "Docs for this file" panels' own deviations. What the design draws there is compared
//    by a person on the frame, and by the app's own baseline here.
// What stays compared: the sidebar, the top bar with the breadcrumb, the title with its chips and the row of actions.
import { expect, test } from "../../fixtures/test";
import { pin, settle } from "../access/lib/app";
import { tier1, type FrameCase } from "../access/lib/matrix";
import type { Mask } from "../access/lib/compare";
import { signedInAs } from "../access/lib/steps";

const BOARD = "Lanekeeper File View.dc.html";
const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const file = (path: string, query = ""): string => `/swimlanes/sitb/files?path=${path}${query}`;

const FACTS: Mask = { x: 245, y: 90, width: 1195, height: 26, why: "Copy affordances the design does not draw; File header and History (no \"CRLF - Git has LF\", and who changed the file at the end of the row)" };
const EDIT: Mask = { x: 248, y: 125, width: 68, height: 30, why: "File: Edit (off while a value is masked)" };
const BODY: Mask = { x: 224, y: 160, width: 1216, height: 740, why: "File viewer and diff (read-only); File: Diff and large files; File: view; the panels" };
const CHROME_ONLY: Mask = { x: 224, y: 44, width: 1216, height: 856, why: "the page under the top bar: a banner above the title moves it (the banner's own layout is compared on the frame by a person); File: Diff and large files; File: view" };
const LAYOUT = [FACTS, EDIT, BODY];
// Where the actions sit depends on whether the facts row and the actions fit on one line: the design's row is a few pixels wider than the
// app's, so for a file whose facts are long the design wraps the actions under it and the app keeps them beside the title.
const ACTIONS_BESIDE: Mask = { x: 960, y: 56, width: 470, height: 34, why: "File header: the actions wrap under the facts at a width that differs by a few pixels from the design's" };
const TITLE_ONLY: Mask = { x: 224, y: 88, width: 1216, height: 812, why: "File header and tab row: the actions wrap under the facts (see ACTIONS_BESIDE)" };

const EDITING_MASKS: Mask[] = [FACTS, EDIT, { x: 224, y: 160, width: 1216, height: 740, why: "File: edit status bar (no validation line, no error count, no cursor position); the editor is Monaco, the design's is a drawing of it" }];

const cases: FrameCase[] = [
  {
    name: "Diff tab, side by side, the ref picker open",
    board: BOARD,
    ids: ["1a"],
    scenario: "design-file-diff",
    path: file(CORE, "&tab=diff"),
    prepare: async (page) => {
      await page.getByRole("button", { name: /^Left / }).click();
      await page.getByRole("menu").or(page.getByRole("listbox")).first().waitFor();
      await page.mouse.move(1439, 899);
    },
    masks: LAYOUT,
  },
  {
    name: "Diff tab, unified",
    board: BOARD,
    ids: ["1b"],
    scenario: "design-file-diff",
    path: file(CORE, "&tab=diff"),
    prepare: async (page) => {
      await page.getByRole("radio", { name: "Unified" }).click();
      await page.mouse.move(1439, 899);
    },
    masks: LAYOUT,
  },
  { name: "Diff tab, line endings only: the diff is behind a notice", board: BOARD, ids: ["1c"], scenario: "design-file-lineending", path: file("tx-infinity-api%2Ftx-infinity-gateway.yml", "&tab=diff"), masks: [ACTIONS_BESIDE, TITLE_ONLY] },
  {
    name: "NFS tab, a masked value, an Editor",
    board: BOARD,
    ids: ["1d"],
    scenario: "design-file-core",
    path: file(CORE),
    before: async (page) => {
      await signedInAs(page, "editor");
    },
    masks: LAYOUT,
  },
  { name: "history panel", board: BOARD, ids: ["1j"], scenario: "design-file-core", path: file(CORE, "&panel=history"), masks: LAYOUT },
  {
    name: "history panel, a revert asks first and lists what it does",
    board: BOARD,
    ids: ["1k"],
    scenario: "design-file-core",
    path: file(CORE, "&panel=history"),
    prepare: async (page) => {
      await page.getByRole("button", { name: /^Revert/ }).first().click();
      await page.getByRole("alertdialog").waitFor();
      await page.mouse.move(1439, 899);
    },
    masks: LAYOUT,
  },
  { name: "large file", board: BOARD, ids: ["1l"], scenario: "design-file-large", path: file("security%2Fsecurity-roles.yml"), masks: [CHROME_ONLY] },
  { name: "binary file, image comparison", board: BOARD, ids: ["1m"], scenario: "design-file-binary", path: file("document-service%2Fresources%2Freceipt-logo.jpeg", "&tab=diff"), masks: [CHROME_ONLY] },
  { name: "denied file, content withheld", board: BOARD, ids: ["1n"], scenario: "design-file-denied", path: file("security%2Fkeystore.jks"), masks: [CHROME_ONLY] },
  { name: "Docs for this file panel", board: BOARD, ids: ["1o"], scenario: "design-file-core", path: file(CORE, "&panel=docs"), masks: LAYOUT },
  {
    name: "edit mode, a save that needs approval",
    board: BOARD,
    ids: ["1i"],
    scenario: "design-file-edit-approval",
    path: file(CORE),
    clock: "date",
    before: async (page) => {
      await signedInAs(page, "editor", "marco", { requires_approval: true });
    },
    prepare: async (page) => {
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await page.locator(".monaco-editor .view-lines").waitFor();
      await page.mouse.move(1439, 899);
    },
    masks: EDITING_MASKS,
  },
  {
    name: "edit mode, a secret in the text: the save is refused",
    board: BOARD,
    ids: ["1h"],
    scenario: "design-file-edit-secret",
    path: file(CORE),
    clock: "date",
    prepare: async (page) => {
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await page.locator(".monaco-editor .view-lines").waitFor();
      await page.locator(".monaco-editor .view-lines").click({ position: { x: 300, y: 100 } });
      await page.keyboard.press("Control+End");
      await page.keyboard.type("\nclientSecret: abc123");
      await page.getByRole("button", { name: /^Save/ }).click();
      await page.getByRole("alertdialog").getByRole("button", { name: /^Save/ }).click();
      await page.getByRole("region", { name: "Problems" }).waitFor();
      await page.mouse.move(1439, 899);
    },
    masks: EDITING_MASKS,
  },
];
tier1(cases);

// Frames whose subject the app does not have, as the design's rows say: the test asserts the absence, so that the row stays true
// (and fails the day the feature arrives, when the frame should be compared).
test.describe("frames the app draws differently, or not at all, by the DEVIATIONS rows", () => {
  test("1d to 1f, Reveal: the masked value is a token, and an Editor has a Reveal button that asks first and sends nothing until asked (File: masked value, File: Reveal dialog)", async ({ page, harness }) => {
    harness.setScenario("design-file-core");
    await pin(page, "light", { width: 1440, height: 900 });
    await signedInAs(page, "editor");
    await page.goto(`${harness.url}${file(CORE)}`);
    await settle(page);
    // The design draws five dots; the app draws a token of words ("Hidden value", and the length the marker states) and, because the
    // mask names its setting, a Reveal button for an Editor. The marker's text is not on the page as text, and no value is.
    await expect(page.getByText("Hidden value").first()).toBeVisible();
    await expect(page.getByText("[REDACTED: 22 chars, generic_secret]")).toHaveCount(0);
    const reveal = page.getByRole("button", { name: /^Reveal value of / });
    await expect(reveal).toHaveCount(1);
    const asked: string[] = [];
    page.on("request", (request) => {
      if (request.url().includes("/reveal")) asked.push(request.url());
    });
    await reveal.click();
    await expect(page.getByRole("alertdialog")).toBeVisible();
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(asked, "opening the dialog and cancelling it asks the hub for nothing").toEqual([]);
  });

  test("1g, Edit with an error at line 88: no validation line, no error count, no cursor position (File: edit status bar)", async ({ page, harness }) => {
    harness.setScenario("design-file-edit");
    await pin(page, "light", { width: 1440, height: 900 }, "date");
    await page.goto(`${harness.url}${file(CORE)}`);
    await settle(page);
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await page.locator(".monaco-editor .view-lines").waitFor();
    const bar = page.getByRole("group", { name: "File format" });
    await expect(bar).toContainText("YAML");
    await expect(page.getByText(/validates as yaml/i)).toHaveCount(0);
    await expect(page.getByText(/\d+ errors?/i)).toHaveCount(0);
    await expect(page.getByText(/Ln \d+/)).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^Save/ })).toBeDisabled();
  });
});

