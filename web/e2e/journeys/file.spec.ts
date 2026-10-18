// The file view (S3) as a person walks it, against the real editor: edit and save (the hub says the file changed first, the person
// re-applies on top of the new version; the swimlane is locked), raise a pull request (one is already open), revert to an earlier
// version, apply a draft, and delete the file.
import type { Page } from "@playwright/test";
import type { FileContent } from "../../src/api/generated/model/index.ts";
import { H_NEWER, H_NFS } from "../../src/mocks/design/file/data.ts";
import { DIST, bodyOf, callsTo, derive, expect, finish, h1, open, responseOf, test, writesOf, type MockContext } from "./support";

test.use({ dist: DIST });

const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const FILE = `/swimlanes/sitb/files?path=${encodeURIComponent(PATH)}`;
const FILES = "/api/v1/swimlanes/{id}/files";
const editorBox = (page: Page) => page.getByRole("textbox", { name: "Editor for tx-infinity-core-sit7.yml" });
const editing = (page: Page) => page.getByText("Editing NFS");

/** The same scenario with the file as someone else saved it: another hash, and a line more. */
function newer(base: string): string {
  const content = responseOf(base, "GET", `${FILES}/content`);
  return derive(
    base,
    [
      {
        ...content,
        body: (context: MockContext) => {
          const file = bodyOf(content, context) as FileContent;
          return file.text == null ? file : { ...file, hash: H_NEWER, text: `# changed by someone else\r\n${file.text}` };
        },
      },
    ],
    "journey-file-newer",
  );
}

async function startEditing(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await expect(editing(page)).toBeVisible();
  await expect(editorBox(page)).toBeFocused({ timeout: 20_000 });
}

async function typeAtStart(page: Page, text: string): Promise<void> {
  await page.keyboard.press("Control+Home");
  await page.keyboard.type(text);
}

const puts = (harness: Parameters<typeof callsTo>[0]) => callsTo(harness, "PUT", /^\/api\/v1\/swimlanes\/sitb\/files$/);

test.describe("saving an edit", () => {
  test.use({ scenario: "design-file-edit" });

  test("is applied: reviewed first, written once with the hash the edit was based on, and the view says it was saved", async ({ page, harness, csp }) => {
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await startEditing(page);
    await typeAtStart(page, "# note\r\n");
    await page.getByRole("button", { name: "Save to sitb NFS" }).click();
    const review = page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" });
    await expect(review).toBeVisible();
    await expect(review).toContainText("The file keeps its line endings (CRLF), encoding and BOM.");
    expect(writesOf(harness)).toEqual([]);
    const sent = page.waitForRequest((request) => request.method() === "PUT");
    await review.getByRole("button", { name: "Save to sitb NFS" }).click();
    const body = (await sent).postDataJSON() as { path: string; expected: { state: string; hash: string }; content: string };
    expect(body.path).toBe(PATH);
    expect(body.expected).toEqual({ state: "hash", hash: H_NFS });
    expect(body.content.startsWith("# note")).toBe(true);
    // CRLF file: the lines the editor wrote keep their endings.
    expect(body.content.includes("\r\n")).toBe(true);
    await expect(editing(page)).toHaveCount(0);
    expect(puts(harness)).toHaveLength(1);
    expect(puts(harness)[0]).toMatchObject({ status: 200, csrf: "ok" });
    expect(puts(harness)[0]?.idempotencyKey).toBeTruthy();
    await finish(page, csp, harness);
  });

  test("is refused because the file changed first (409): nothing was written, the person's text is kept, and re-applying it on the new version saves it", async ({ page, harness, csp }) => {
    // The hub's answers change while the person edits: someone saves first (the file and the refusal), then the person re-applies.
    const conflict = newer("design-file-edit-conflict");
    const accepted = newer("design-file-edit");
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await startEditing(page);
    await typeAtStart(page, "# mine\r\n");
    harness.setScenario(conflict);
    await page.getByRole("button", { name: "Save to sitb NFS" }).click();
    await page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" }).getByRole("button", { name: "Save to sitb NFS" }).click();

    const dialog = page.getByRole("dialog", { name: "File changed on NFS while you were editing" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("Your save was not applied. Nothing on NFS changed.");
    await expect(dialog).toContainText(H_NFS.slice(0, 7));
    await expect(dialog).toContainText(H_NEWER.slice(0, 7));
    await expect(dialog).toContainText("req-mock-0001");
    expect(puts(harness)).toHaveLength(1);
    expect(puts(harness)[0]).toMatchObject({ status: 409 });

    // Re-apply on top: the person's text stays, the new version is the base, and the next save says it replaces someone else's.
    harness.setScenario(accepted);
    await dialog.getByRole("button", { name: "Reload and re-apply" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(editing(page)).toBeVisible();
    await expect(page.locator(".monaco-editor .view-line").first()).toContainText("# mine");
    await page.getByRole("button", { name: "Save to sitb NFS" }).click();
    const review = page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" });
    await expect(review).toContainText("This replaces the version that is on NFS now, which someone else saved, with yours.");
    const sent = page.waitForRequest((request) => request.method() === "PUT");
    await review.getByRole("button", { name: "Save to sitb NFS" }).click();
    const second = (await sent).postDataJSON() as { expected: { hash: string }; content: string };
    expect(second.expected.hash, "the second save is based on the version that was there when the person re-applied").toBe(H_NEWER);
    expect(second.content.startsWith("# mine")).toBe(true);
    await expect(editing(page)).toHaveCount(0);
    expect(puts(harness).map((r) => r.status)).toEqual([409, 200]);
    await finish(page, csp, harness, { expected: [409] });
  });

  test("is refused while the swimlane is locked (423): the dialog says why and offers the adoption, and the edit stays open", async ({ page, harness, csp }) => {
    harness.setScenario("design-file-edit-locked");
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await startEditing(page);
    await typeAtStart(page, "# mine\r\n");
    await page.getByRole("button", { name: "Save to sitb NFS" }).click();
    await page.getByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" }).getByRole("button", { name: "Save to sitb NFS" }).click();
    const dialog = page.getByRole("dialog", { name: "sitb is locked until it is adopted" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("Its baseline is unconfirmed, so edits are blocked.");
    await expect(dialog.getByRole("link", { name: "Start adoption" })).toHaveAttribute("href", "/swimlanes/sitb/adopt");
    expect(puts(harness)[0]).toMatchObject({ status: 423 });
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toHaveCount(0);
    // Nothing was lost: the editor is still there with the person's line.
    await expect(editing(page)).toBeVisible();
    await expect(page.locator(".monaco-editor .view-line").first()).toContainText("# mine");
    await finish(page, csp, harness, { expected: [423] });
  });
});

test.describe("raising a pull request", () => {
  test.use({ scenario: "design-file-pr" });

  test("goes to the tenant branch by default, shows what it writes first, and ends with the link only when the job is done", async ({ page, harness, csp }) => {
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await page.getByRole("button", { name: "Raise PR" }).click();
    const dialog = page.getByRole("dialog", { name: "Raise pull request" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("radio", { name: /Tenant branch sit7/ })).toBeChecked();
    await expect(dialog).toContainText("csp/csp-tenant-data");
    await expect(dialog).toContainText("data/config/tx-infinity-api/tx-infinity-core.yml");
    await dialog.getByRole("button", { name: "Continue" }).click();
    await expect(dialog.getByRole("textbox", { name: "Title" })).toHaveValue("Update tx-infinity-core for sit7 (from sitb)");
    await dialog.getByRole("button", { name: "Create PR" }).click();
    // The title of the dialog changes with the job: it says "created" only when the job says completed.
    const done = page.getByRole("dialog", { name: "Pull request created" });
    await expect(done).toBeVisible();
    await expect(done.getByRole("link", { name: /github\.com\/.*\/pull\/\d+/ }).first()).toBeVisible();
    const posts = callsTo(harness, "POST", "/api/v1/prs");
    expect(posts).toHaveLength(1);
    expect(posts[0]).toMatchObject({ status: 202, csrf: "ok" });
    expect(JSON.parse(posts[0]?.body ?? "{}")).toMatchObject({ swimlane: "sitb", paths: [PATH], target: { kind: "tenant", branch: "sit7" } });
    await finish(page, csp, harness);
  });

  test("is not created twice: a change that has a pull request open shows that one, from the preview and from the hub's refusal (409 duplicate_pr)", async ({ page, harness, csp }) => {
    harness.setScenario("design-file-pr-exists");
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await page.getByRole("button", { name: "Raise PR" }).click();
    const dialog = page.getByRole("dialog", { name: "Raise pull request" });
    // The tenant target: the preview already knows the open one.
    await dialog.getByRole("button", { name: "Continue" }).click();
    await expect(dialog.getByText("PR #418 is already open for this change.")).toBeVisible();
    await expect(dialog.getByRole("link", { name: "Open PR #418" })).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/418");
    expect(callsTo(harness, "POST", "/api/v1/prs")).toHaveLength(0);
    await dialog.getByRole("button", { name: "Close" }).first().click();
    await expect(dialog).toHaveCount(0);

    // The base target: the preview knows nothing, the person goes on, and the hub refuses the second one.
    await page.getByRole("button", { name: "Raise PR" }).click();
    await dialog.getByRole("radio", { name: /Base \(all swimlanes\)/ }).click();
    await dialog.getByRole("button", { name: "Continue" }).click();
    await dialog.getByRole("button", { name: "Create PR" }).click();
    await expect(dialog.getByText("PR #418 is already open for this change.")).toBeVisible();
    const posts = callsTo(harness, "POST", "/api/v1/prs");
    expect(posts).toHaveLength(1);
    expect(posts[0]).toMatchObject({ status: 409 });
    await finish(page, csp, harness, { expected: [409] });
  });
});

test.describe("reverting to an earlier version", () => {
  test.use({ scenario: "design-file-edit" });

  test("is asked first, names the version, and is written once against the hash that is there now", async ({ page, harness, csp }) => {
    await open(page, harness.url, `${FILE}&panel=history`, "tx-infinity-core.yml");
    const history = page.getByRole("complementary", { name: "History" });
    await expect(history.getByRole("list", { name: "Versions of this file" }).getByRole("listitem")).toHaveCount(4);
    await history.getByRole("button", { name: "Revert to c19a4e2" }).click();
    const dialog = page.getByRole("alertdialog", { name: "Revert to c19a4e2?" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("The version you replace stays in the history.");
    expect(writesOf(harness)).toEqual([]);
    await dialog.getByRole("button", { name: "Revert to c19a4e2" }).click();
    await expect(dialog).toHaveCount(0);
    const posts = callsTo(harness, "POST", /\/files\/revert$/);
    expect(posts).toHaveLength(1);
    expect(posts[0]).toMatchObject({ status: 200, csrf: "ok" });
    expect(JSON.parse(posts[0]?.body ?? "{}")).toEqual({ path: PATH, expected: { state: "hash", hash: H_NFS }, to: H_NEWER });
    await finish(page, csp, harness);
  });
});

test.describe("applying a draft", () => {
  test.use({ scenario: "design-file-draft" });

  test("shows what it changes first, applies it once with the hash it was made against, and says it is applied", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/drafts/7", "Review draft");
    await expect(page.getByRole("grid", { name: "Changes to tx-infinity-core-sit7.yml" })).toBeVisible();
    await expect(page.getByText("2 settings changed, 1 added")).toBeVisible();
    await page.getByRole("button", { name: "Apply draft" }).click();
    const dialog = page.getByRole("alertdialog", { name: "Apply this draft?" });
    await expect(dialog).toBeVisible();
    expect(writesOf(harness)).toEqual([]);
    await dialog.getByRole("button", { name: "Apply draft" }).click();
    await expect(dialog).toHaveCount(0);
    const posts = callsTo(harness, "POST", "/api/v1/drafts/7/apply");
    expect(posts).toHaveLength(1);
    expect(posts[0]).toMatchObject({ status: 200, csrf: "ok" });
    await expect(page.getByText(/applied/i).first()).toBeVisible();
    await finish(page, csp, harness);
  });

  test("is refused when the file changed since the draft was made (409): nothing is applied", async ({ page, harness, csp }) => {
    harness.setScenario("design-file-draft-conflict");
    await open(page, harness.url, "/drafts/7", "Review draft");
    await page.getByRole("button", { name: "Apply draft" }).click();
    await page.getByRole("alertdialog", { name: "Apply this draft?" }).getByRole("button", { name: "Apply draft" }).click();
    await expect(page.getByRole("dialog", { name: /changed|Someone else/i })).toBeVisible();
    expect(callsTo(harness, "POST", "/api/v1/drafts/7/apply")[0]).toMatchObject({ status: 409 });
    await finish(page, csp, harness, { expected: [409] });
  });
});

test.describe("deleting the file", () => {
  test.use({ scenario: "design-file-edit" });

  test("works out the effect first, needs the file's name typed (Enter does not confirm), and takes the person back to the swimlane", async ({ page, harness, csp }) => {
    await open(page, harness.url, FILE, "tx-infinity-core.yml");
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    const dialog = page.getByRole("alertdialog", { name: "Delete tx-infinity-core-sit7.yml from sitb?" });
    await expect(dialog).toBeVisible();
    // The effect is the engine's, asked for when the dialog opens.
    await expect.poll(() => callsTo(harness, "POST", /\/files\/preview$/).length).toBe(1);
    const verb = dialog.getByRole("button", { name: "Delete file" });
    await dialog.getByRole("textbox").fill("tx-infinity-core-sit7.yml");
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    expect(callsTo(harness, "DELETE", /\/files\?/)).toHaveLength(0);
    await verb.click();
    await expect(page).toHaveURL(/\/swimlanes\/sitb$/);
    const deletes = callsTo(harness, "DELETE", /\/files\?/);
    expect(deletes).toHaveLength(1);
    expect(deletes[0]).toMatchObject({ status: 200, csrf: "ok" });
    const query = new URL(deletes[0]?.path ?? "", harness.url).searchParams;
    expect(query.get("path")).toBe(PATH);
    expect(query.get("expected")).toBe(H_NFS);
    expect(query.get("preview_id")).toBe("pv-mock-0001");
    await expect(h1(page)).toHaveText("sitb");
    await finish(page, csp, harness);
  });
});
