// The keymap's File viewer and Diff rows, in a real browser against the real editor (Monaco, under the production CSP): e starts
// an edit, Control or Command and S opens the review (it never saves by itself), Escape cancels (and asks when there is something
// to lose); u switches the diff between split and unified, n and p walk its changes, Enter on a fold opens the unchanged lines.
// Each has the cases that make a key safe: a key typed into the editor is text, a role that cannot edit gets nothing, and a key
// does nothing while a dialog is open.
import type { Page } from "@playwright/test";
import type { Harness } from "../harness/serve.ts";
import { pageViolations } from "../fixtures/test";
import type { Comparison } from "../../src/api/generated/model/index.ts";
import { coreFile } from "../../src/mocks/design/file/data.ts";
import { bodyOf, derive, responseOf, type MockContext } from "../journeys/scenarios";
import { DIST, expect, test, h1, keyboardOnly, ready, row } from "./support";

test.use({ dist: DIST });

const FILE = "/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const editorBox = (page: Page) => page.getByRole("textbox", { name: "Editor for tx-infinity-core-sit7.yml" });
const editButton = (page: Page) => page.getByRole("button", { name: "Edit", exact: true });
const editing = (page: Page) => page.getByText("Editing NFS");
/** What the editor shows on its first line (Monaco draws only the lines in view, so the first is always there). */
const firstLine = (page: Page): Promise<string> => page.locator(".monaco-editor .view-line").first().evaluate((el) => (el.textContent ?? "").replace(/\u00a0/g, " "));
const writes = (harness: Harness) => harness.requests().filter((r) => r.method !== "GET");

async function startEditing(page: Page): Promise<void> {
  await page.keyboard.press("e");
  await expect(editing(page)).toBeVisible();
  // The editor is a lazy chunk: it takes focus when it arrives.
  await expect(editorBox(page)).toBeFocused({ timeout: 20_000 });
}

test.describe("the file viewer", () => {
  test.use({ scenario: "design-file-edit" });

  test(row("file-viewer.edit") + ": e starts editing with the cursor in the editor; typed into the editor it is text", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, FILE, "tx-infinity-core.yml");
    await expect(editButton(page)).toBeEnabled();
    await startEditing(page);
    await expect(editButton(page)).toBeDisabled();
    // Already editing: the key is a letter, and it is the editor's.
    await page.keyboard.press("Control+Home");
    await page.keyboard.type("eee");
    await expect.poll(() => firstLine(page)).toMatch(/^eee# tx-infinity-core/);
    // Nothing was written.
    expect(writes(harness)).toEqual([]);
    expect(await pointer()).toEqual([]);
    expect(await pageViolations(page)).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("file: e is a letter in a field, does nothing while a dialog is open, and starts nothing for a file that is not editable", async ({ page, harness }) => {
    await ready(page, harness.url, FILE, "tx-infinity-core.yml");
    // A dialog is open: the key is not delivered to the page (the dialog's, if it wanted it).
    await page.getByRole("button", { name: "Delete" }).click();
    await expect(page.getByRole("alertdialog")).toBeVisible();
    await page.keyboard.press("e");
    await expect(editing(page)).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    // Back on the page the same key starts the edit.
    await page.locator("body").click({ position: { x: 5, y: 5 } });
    await page.keyboard.press("e");
    await expect(editing(page)).toBeVisible();
  });

  test("file: e does nothing for a Viewer, and for a file whose view has masked values", async ({ page, harness }) => {
    harness.setScenario("design-file-viewer");
    await ready(page, harness.url, FILE, "tx-infinity-core.yml");
    await expect(editButton(page)).toBeDisabled();
    await page.keyboard.press("e");
    await page.waitForTimeout(300);
    await expect(editing(page)).toHaveCount(0);
    harness.setScenario("design-file-core");
    await ready(page, harness.url, FILE, "tx-infinity-core.yml");
    await expect(editButton(page)).toBeDisabled();
    await page.keyboard.press("e");
    await page.waitForTimeout(300);
    await expect(editing(page)).toHaveCount(0);
  });

  test(row("file-viewer.review-and-save-opens-the") + ": Control or Command and S opens the review and never saves by itself; with nothing to save it does nothing", async ({ page, harness, csp }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, FILE, "tx-infinity-core.yml");
    await startEditing(page);
    // No change yet: nothing to review.
    await page.keyboard.press("ControlOrMeta+s");
    await page.waitForTimeout(300);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    // A change: the review opens, on the least destructive control, and nothing is written.
    await page.keyboard.press("Control+Home");
    await page.keyboard.type("x");
    await page.keyboard.press("ControlOrMeta+s");
    const review = page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" });
    await expect(review).toBeVisible();
    await expect(review.getByRole("button", { name: "Cancel" })).toBeFocused();
    expect(writes(harness)).toEqual([]);
    // The chord again does not open another; Escape closes it and the editor has what it had.
    await page.keyboard.press("ControlOrMeta+s");
    await expect(page.getByRole("alertdialog")).toHaveCount(1);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    expect(writes(harness)).toEqual([]);
    expect(await firstLine(page)).toMatch(/^x# tx-infinity-core/);
    // Reviewed and confirmed from the keyboard: one write, with the hash the edit was based on.
    await page.keyboard.press("ControlOrMeta+s");
    await expect(review).toBeVisible();
    await page.keyboard.press("Tab");
    await expect(review.getByRole("button", { name: "Save to sitb NFS" })).toBeFocused();
    const sent = page.waitForRequest((request) => request.method() === "PUT");
    await page.keyboard.press("Enter");
    const body = (await sent).postDataJSON() as { path: string; expected: { state: string; hash: string }; content: string };
    expect(body.path).toBe("tx-infinity-api/tx-infinity-core-sit7.yml");
    expect(body.expected).toEqual({ state: "hash", hash: "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a" });
    expect(body.content.startsWith("x# tx-infinity-core")).toBe(true);
    await expect(editing(page)).toHaveCount(0);
    const put = writes(harness);
    expect(put).toHaveLength(1);
    expect(put[0]).toMatchObject({ method: "PUT", csrf: "ok" });
    expect(put[0]?.idempotencyKey).toBeTruthy();
    expect(await pointer()).toEqual([]);
    expect(csp.report()).toBe("");
  });

  test("editor: the single keys of the app are text in the editor", async ({ page, harness }) => {
    await ready(page, harness.url, FILE, "tx-infinity-core.yml");
    await startEditing(page);
    await page.keyboard.press("Control+Home");
    // Go to, the shortcuts sheet, the filter key, the diff keys and the edit key: all letters here.
    await page.keyboard.type("gc?/unpfe");
    await expect.poll(() => firstLine(page)).toMatch(/^gc\?\/unpfe# tx-infinity-core/);
    expect(new URL(page.url()).pathname).toBe("/swimlanes/sitb/files");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(editing(page)).toBeVisible();
    // Nothing was asked of the hub but the file's reads.
    expect(writes(harness)).toEqual([]);
  });

  test("dialog: while the review is open the keys of the page behind it do nothing", async ({ page, harness }) => {
    await ready(page, harness.url, FILE, "tx-infinity-core.yml");
    await startEditing(page);
    await page.keyboard.press("Control+Home");
    await page.keyboard.type("x");
    await page.keyboard.press("ControlOrMeta+s");
    const review = page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" });
    await expect(review).toBeVisible();
    await page.keyboard.press("g");
    await page.keyboard.press("c");
    await page.keyboard.press("?");
    await page.keyboard.press("/");
    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.press("ControlOrMeta+b");
    await page.waitForTimeout(300);
    expect(new URL(page.url()).pathname).toBe("/swimlanes/sitb/files");
    await expect(page.getByRole("alertdialog")).toHaveCount(1);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(review.getByRole("button", { name: "Cancel" })).toBeFocused();
  });

  test(row("file-viewer.cancel-edit-asks-if-there") + ": Escape leaves an edit with nothing in it at once, and asks before it throws changes away", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, FILE, "tx-infinity-core.yml");
    await startEditing(page);
    // Nothing changed: Escape just leaves.
    await page.keyboard.press("Escape");
    await expect(editing(page)).toHaveCount(0);
    await expect(page.getByRole("alertdialog")).toHaveCount(0);

    // Changed: Escape asks, on Keep editing.
    await page.keyboard.press("e");
    await expect(editorBox(page)).toBeFocused({ timeout: 20_000 });
    await page.keyboard.press("Control+Home");
    await page.keyboard.type("zz");
    await page.keyboard.press("Escape");
    const ask = page.getByRole("alertdialog", { name: "Discard your changes?" });
    await expect(ask).toBeVisible();
    await expect(ask.getByRole("button", { name: "Keep editing" })).toBeFocused();
    // Escape in the question keeps editing, and the text is still there.
    await page.keyboard.press("Escape");
    await expect(ask).toHaveCount(0);
    await expect(editing(page)).toBeVisible();
    await expect(editorBox(page)).toBeFocused();
    expect(await firstLine(page)).toMatch(/^zz# tx-infinity-core/);
    // Asked again, and this time discarded from the keyboard.
    await page.keyboard.press("Escape");
    await expect(ask).toBeVisible();
    await page.keyboard.press("Tab");
    await expect(ask.getByRole("button", { name: "Discard changes" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(editing(page)).toHaveCount(0);
    await expect(h1(page)).toHaveText("tx-infinity-core.yml");
    expect(writes(harness)).toEqual([]);
    expect(await pointer()).toEqual([]);
  });
});

// The unfolding is tested on hunks made from the file itself: two changes of three lines of context each, as the engine draws them (the
// scenario's own hunks agree with its file too, src/mocks/design/file/consistency.test.ts, but they carry the design's wider context).
const compareOf = responseOf("design-file-diff", "GET", "/api/v1/compare");
const TEXT_LINES = (coreFile({ masked: true }).content.text ?? "").split("\r\n");
const line = (n: number): string => TEXT_LINES[n - 1] ?? "";
const context = (from: number, to: number) => Array.from({ length: to - from + 1 }, (_, i) => ({ kind: "context" as const, text: line(from + i), inline_changes: [] }));
const HUNKS: Comparison["hunks"] = [
  {
    old_start: 10,
    old_lines: 7,
    new_start: 10,
    new_lines: 8,
    lines: [...context(10, 12), { kind: "removed", text: "    enableAccountSorting: false", inline_changes: [] }, { kind: "added", text: line(13), inline_changes: [] }, { kind: "added", text: line(14), inline_changes: [] }, ...context(15, 17)],
  },
  {
    old_start: 26,
    old_lines: 7,
    new_start: 27,
    new_lines: 7,
    lines: [...context(27, 29), { kind: "removed", text: "  level: WARN", inline_changes: [] }, { kind: "added", text: line(30), inline_changes: [] }, ...context(31, 33)],
  },
];
const ALIGNED = derive("design-file-diff", [{ ...compareOf, body: (context: MockContext) => ({ ...(bodyOf(compareOf, context) as Comparison), hunks: HUNKS }) }], "keyboard-diff-aligned");

test.describe("the diff", () => {
  test.use({ scenario: ALIGNED });
  const DIFF = `${FILE}&tab=diff`;
  const grid = (page: Page) => page.getByRole("grid", { name: "Diff of tx-infinity-core-sit7.yml" });
  const layout = (page: Page, name: "Side by side" | "Unified") => page.getByRole("radio", { name });

  test(row("diff.toggle-split-and-unified") + ": u switches between side by side and unified, from anywhere on the tab, and is a letter in a field", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, DIFF, "tx-infinity-core.yml");
    await expect(grid(page)).toBeVisible();
    await expect(layout(page, "Side by side")).toBeChecked();
    await page.keyboard.press("u");
    await expect(layout(page, "Unified")).toBeChecked();
    await page.keyboard.press("u");
    await expect(layout(page, "Side by side")).toBeChecked();
    // Typed into a field it is a character: the palette's search box is one.
    await page.keyboard.press("ControlOrMeta+k");
    await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox")).toBeFocused();
    await page.keyboard.type("uu");
    await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox")).toHaveValue("uu");
    await page.keyboard.press("Escape");
    await expect(layout(page, "Side by side")).toBeChecked();
    expect(await pointer()).toEqual([]);
  });

  test(row("diff.next-previous-change") + ": n goes to the next change and p to the previous, and the toolbar says which one", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, DIFF, "tx-infinity-core.yml");
    await expect(grid(page)).toBeVisible();
    await expect(page.getByText("2 changes", { exact: true })).toBeVisible();
    // The keys hinted on the buttons work as the tab opens, with the keyboard still where navigation left it.
    await page.keyboard.press("n");
    await expect(page.getByText("Change 1 of 2")).toBeVisible();
    await page.keyboard.press("n");
    await expect(page.getByText("Change 2 of 2")).toBeVisible();
    // At the last change n stays, and p goes back.
    await page.keyboard.press("n");
    await expect(page.getByText("Change 2 of 2")).toBeVisible();
    await page.keyboard.press("p");
    await expect(page.getByText("Change 1 of 2")).toBeVisible();
    await page.keyboard.press("p");
    await expect(page.getByText("Change 1 of 2")).toBeVisible();
    // The active change is the row the diff names as its active descendant, and it is in view.
    const active = await grid(page).evaluate((el) => {
      const row = document.getElementById(el.getAttribute("aria-activedescendant") ?? "");
      const box = row?.getBoundingClientRect();
      const host = el.getBoundingClientRect();
      return { text: row?.textContent ?? "", inView: box !== undefined && box.top >= host.top && box.bottom <= host.bottom };
    });
    expect(active.text).toContain("enableAccountSorting");
    expect(active.inView).toBe(true);
    expect(await pointer()).toEqual([]);
  });

  test("diff: n and p do the same with the diff itself focused, and are letters in a field", async ({ page, harness }) => {
    await ready(page, harness.url, DIFF, "tx-infinity-core.yml");
    await grid(page).focus();
    await page.keyboard.press("n");
    await expect(page.getByText("Change 1 of 2")).toBeVisible();
    await page.keyboard.press("n");
    await expect(page.getByText("Change 2 of 2")).toBeVisible();
    await page.keyboard.press("p");
    await expect(page.getByText("Change 1 of 2")).toBeVisible();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox")).toBeFocused();
    await page.keyboard.type("nnpp");
    await expect(page.getByRole("dialog", { name: "Command palette" }).getByRole("combobox")).toHaveValue("nnpp");
    await expect(page.getByText("Change 1 of 2")).toBeVisible();
  });

  test(row("diff.expand-unchanged-lines") + ": Enter on a fold row opens the unchanged lines it stands for", async ({ page, harness }) => {
    const pointer = await keyboardOnly(page);
    await ready(page, harness.url, DIFF, "tx-infinity-core.yml");
    const folds = grid(page).getByRole("button", { name: /^\d+ unchanged lines$/ });
    await expect(folds.first()).toBeVisible();
    const before = await folds.count();
    expect(before).toBeGreaterThanOrEqual(2);
    // Tab to the first fold, as a keyboard user does: the diff first, then the folds in it.
    await grid(page).focus();
    await page.keyboard.press("Tab");
    const first = folds.first();
    await expect(first).toBeFocused();
    await expect(first).toHaveText("9 unchanged lines");
    await page.keyboard.press("Enter");
    // The nine lines are there now, from the first line of the file, and the fold that stood for them is gone.
    await expect(grid(page).getByRole("row", { name: /^1 # tx-infinity-core/ })).toBeVisible();
    await expect(folds).toHaveCount(before - 1);
    await expect(grid(page).getByRole("button", { name: "9 unchanged lines", exact: true })).toHaveCount(1);
    expect(await pointer()).toEqual([]);
  });
});
