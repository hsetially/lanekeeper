// The Access screens (S1) as a person walks them: signing in, adding a GitHub token, changing and deleting it, being refused a
// screen for the role, the personal MCP tokens (shown once, copied once), and the page the hub does not offer.
import { DIST, allowClipboard, callsTo, clipboard, derive, designClock, expect, expectNoTrace, finish, h1, open, responseOf, sidebar, test, writesOf, type MockResponse } from "./support";

test.use({ dist: DIST });

// A fine-grained token as GitHub makes them: long, with a prefix. This one is made up.
const TOKEN = `github_pat_11ABCDEFG0${"x7Qm2".repeat(12)}`;

test.describe("signing in", () => {
  test.use({ scenario: "signed-out" });

  test("the sign-in link carries the page the person wanted as return_to, and a hostile one is dropped", async ({ page, harness, csp }) => {
    // A page behind the session: the 401 sends the person to sign-in, remembering where they were going.
    await page.goto(`${harness.url}/swimlanes/sitb/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml&line=13`);
    await expect(page).toHaveURL(/\/signin\?next=/);
    await expect(h1(page)).toHaveText("Lanekeeper");
    const link = page.getByRole("link", { name: "Sign in with Microsoft" });
    const target = new URL((await link.getAttribute("href")) ?? "", harness.url);
    expect(target.pathname).toBe("/auth/login");
    const back = new URL(target.searchParams.get("return_to") ?? "", harness.url);
    expect(back.origin).toBe(harness.url);
    expect(back.pathname).toBe("/swimlanes/sitb/files");
    expect(Object.fromEntries(back.searchParams)).toEqual({ path: "tx-infinity-api/tx-infinity-core-sit7.yml", line: "13" });

    // Straight to sign-in, with nothing to go back to: the link has no return_to.
    await page.goto(`${harness.url}/signin`);
    await expect(link).toHaveAttribute("href", "/auth/login");

    // A `next` that is not a path of this app (another origin, a protocol-relative address, a script) never reaches the link.
    for (const hostile of ["https://evil.example/", "//evil.example/", "javascript:alert(1)", "/\\evil.example", "/swimlanes/../../x"]) {
      await page.goto(`${harness.url}/signin?next=${encodeURIComponent(hostile)}`);
      await expect(h1(page)).toBeVisible();
      const href = (await link.getAttribute("href")) ?? "";
      expect(href, hostile).not.toContain("evil");
      expect(href, hostile).not.toMatch(/javascript/i);
    }

    // A public page asks the hub for nothing but the sign-out.
    harness.clearRequests();
    await page.goto(`${harness.url}/signin`);
    await expect(h1(page)).toBeVisible();
    await page.waitForTimeout(400);
    expect(harness.requests().filter((r) => r.path.startsWith("/api/"))).toEqual([]);
    await finish(page, csp, harness, { expected: [401] });
  });
});

test.describe("adding the GitHub token", () => {
  test.use({ scenario: "design-access-onboarding" });

  test("an Editor with no token is taken to the onboarding, pastes the token, sees it saved, and continues; the token is nowhere afterwards", async ({ page, harness, csp }) => {
    await designClock(page);
    // The home page of an Editor whose token is missing is the onboarding.
    await page.goto(`${harness.url}/`);
    await expect(page).toHaveURL(/\/access\/onboarding$/);
    await expect(h1(page)).toHaveText("Add your GitHub token");
    await expect(h1(page)).toBeFocused();

    const field = page.getByLabel("2 · Paste the token");
    await expect(field).toHaveAttribute("type", "password");
    await expect(field).toHaveAttribute("autocomplete", /off|new-password/);
    await field.fill(TOKEN);
    await page.getByRole("button", { name: "Verify and save" }).click();

    // Saved: only the login and the expiry are shown.
    const saved = page.getByRole("status").filter({ hasText: "Token verified and saved" });
    await expect(saved).toBeVisible();
    await expect(saved).toContainText("mdiaz");
    await expect(saved).toContainText("in 90 days");
    await expect(page.getByLabel("2 · Paste the token")).toHaveCount(0);

    // One write, to the right place, with the token in its body and nowhere else; CSRF and an idempotency key went with it.
    const puts = callsTo(harness, "PUT", "/api/v1/me/github-token");
    expect(puts).toHaveLength(1);
    expect(puts[0]).toMatchObject({ csrf: "ok", status: 200 });
    expect(puts[0]?.idempotencyKey).toBeTruthy();
    expect(puts[0]?.body).toContain(TOKEN);
    expect(writesOf(harness)).toHaveLength(1);

    // The token is not in the page, the storage, the cookies, the console or any address.
    await expectNoTrace(page, csp, harness, TOKEN);

    // Continue goes to the swimlanes; the person is not sent back to the onboarding by a stale answer.
    harness.setScenario("design-swimlanes-six");
    await page.getByRole("link", { name: "Continue to Lanekeeper" }).click();
    await expect(page).toHaveURL(/\/swimlanes$/);
    await expect(h1(page)).toHaveText("Swimlanes");
    await expectNoTrace(page, csp, harness, TOKEN);
    await finish(page, csp, harness);
  });

  test("the field is emptied after a refusal, and the token is not kept for another try", async ({ page, harness, csp }) => {
    harness.setScenario("design-access-onboarding-classic");
    await page.goto(`${harness.url}/access/onboarding`);
    await expect(h1(page)).toHaveText("Add your GitHub token");
    await page.getByLabel("2 · Paste the token").fill(TOKEN);
    await page.getByRole("button", { name: "Verify and save" }).click();
    await expect(page.getByText(/Classic tokens are not accepted|classic token/i).first()).toBeVisible();
    await expect(page.getByLabel("2 · Paste the token")).toHaveValue("");
    await expectNoTrace(page, csp, harness, TOKEN);
    await finish(page, csp, harness, { expected: [422] });
  });
});

// The access scenarios answer the reads of My GitHub token; the writes (replace, delete) are added here.
const REPLACE = responseOf("design-access-onboarding", "PUT", "/api/v1/me/github-token");
const DELETE: MockResponse = { method: "DELETE", path: "/api/v1/me/github-token", status: 204 };
const WITH_TOKEN = derive("design-access-mytoken-ok", [REPLACE, DELETE], "journey-token");
const WITHOUT_TOKEN = derive("design-access-mytoken-none", [REPLACE, DELETE], "journey-no-token");

test.describe("changing the token", () => {
  test.use({ scenario: WITH_TOKEN });

  test("replaces the token (the old one is not shown, the new one is not kept) and deletes it, and the page then offers to add one", async ({ page, harness, csp }) => {
    await designClock(page);
    await open(page, harness.url, "/me/github-token", "My GitHub token");
    await expect(page.getByText("mdiaz", { exact: true })).toBeVisible();
    await expect(page.getByText("Stored encrypted. It can't be viewed or copied.")).toBeVisible();
    // Nothing on the page lets the person see or copy the token.
    await expect(page.getByRole("button", { name: /copy/i })).toHaveCount(0);

    // Replace: the form opens under the summary with the field ready; the new token goes out once and the field goes away.
    await page.getByRole("button", { name: "Replace token" }).click();
    const field = page.getByLabel("2 · Paste the token");
    await expect(field).toBeFocused();
    await field.fill(TOKEN);
    await page.getByRole("button", { name: "Verify and save" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Token replaced" })).toBeVisible();
    await expect(field).toHaveCount(0);
    const puts = callsTo(harness, "PUT", "/api/v1/me/github-token");
    expect(puts).toHaveLength(1);
    expect(puts[0]?.body).toContain(TOKEN);
    await expectNoTrace(page, csp, harness, TOKEN);

    // Delete: asks first, names the consequence, and the page offers a new token after the hub says it is gone.
    await page.getByRole("button", { name: "Delete token" }).click();
    const confirm = page.getByRole("alertdialog", { name: "Delete your GitHub token" });
    await expect(confirm).toBeVisible();
    await expect(confirm).toContainText("You can't save or raise PRs until you add a new token");
    expect(callsTo(harness, "DELETE", "/api/v1/me/github-token")).toHaveLength(0);
    harness.setScenario(WITHOUT_TOKEN);
    await confirm.getByRole("button", { name: "Delete token", exact: true }).click();
    await expect.poll(() => callsTo(harness, "DELETE", "/api/v1/me/github-token").length).toBe(1);
    await expect(page.getByRole("status").filter({ hasText: "Your GitHub token was deleted." })).toBeVisible();
    await expect(page.getByText("No GitHub token stored")).toBeVisible();
    await expect(page.getByLabel("2 · Paste the token")).toBeVisible();
    await expect(page.getByRole("button", { name: "Delete token" })).toHaveCount(0);
    expect(callsTo(harness, "DELETE", "/api/v1/me/github-token")[0]).toMatchObject({ csrf: "ok", status: 204 });
    await expectNoTrace(page, csp, harness, TOKEN);
    // The browser logs the 404 the hub answers when it is asked for the token that is no longer there.
    await finish(page, csp, harness, { expected: [404] });
  });
});

test.describe("a role that does not reach a screen", () => {
  test("a Viewer at the onboarding is told which role is needed, offered a request for it, and sees no entry for it in the menu", async ({ page, harness, csp }) => {
    // The account menu asks whether the personal MCP tokens are on (a Viewer may have them), which this scenario does not draw.
    harness.setScenario(derive("design-access-viewer", [{ method: "GET", path: "/api/v1/me/mcp-tokens", body: { items: [], next: null } }], "journey-viewer"));
    await page.goto(`${harness.url}/access/onboarding`);
    await expect(h1(page)).toBeVisible();
    await expect(page.getByText(/needs the Editor role|Editor/).first()).toBeVisible();
    // The permission state, not the onboarding: no field for a token.
    await expect(page.getByLabel("2 · Paste the token")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /request a role change/i })).toBeVisible();
    // The account menu has no entry for the GitHub token of a role that cannot use one.
    await page.getByRole("complementary", { name: "Primary" }).getByRole("button", { name: /Marco Diaz/ }).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(page.getByRole("menuitem", { name: /GitHub token/i })).toHaveCount(0);
    await page.keyboard.press("Escape");
    await finish(page, csp, harness);
  });
});

test.describe("the personal MCP tokens", () => {
  test.use({ scenario: "design-access-mcp-off" });

  test("a hub that has the mode off answers 404, and the page says so instead of failing", async ({ page, harness, csp }) => {
    await open(page, harness.url, "/me/tokens", "Personal MCP tokens");
    await expect(page.getByText(/not (switched )?on|isn't available|not available|turned off/i).first()).toBeVisible();
    await expect(page.getByRole("button", { name: "Create token" })).toHaveCount(0);
    // The sidebar has no entry for it either.
    await sidebar(page).getByRole("button", { name: /Marco Diaz/ }).click();
    await expect(page.getByRole("menuitem", { name: /MCP tokens/i })).toHaveCount(0);
    await finish(page, csp, harness, { expected: [404] });
  });
});

test.describe("an MCP token that is shown once", () => {
  test.use({ scenario: "design-access-mcp-token-issued" });

  test("is created once, copied with the button (the clipboard has the whole token, the page says it will not be shown again), and is gone when it is put away", async ({ page, harness, context, csp }) => {
    await allowClipboard(context, harness.url);
    await designClock(page);
    await open(page, harness.url, "/me/tokens", "Personal MCP tokens");
    await page.getByLabel("Label").fill("ci");
    await page.getByRole("button", { name: "Create token" }).click();
    const shown = page.getByLabel("Your new token");
    await expect(shown).toBeVisible();
    const token = await shown.inputValue();
    expect(token).toMatch(/^lkpat_[A-Za-z0-9]{40,}$/);

    // One POST made it. Nothing else was written, and the token is not in any address.
    expect(callsTo(harness, "POST", "/api/v1/me/mcp-tokens")).toHaveLength(1);
    expect(writesOf(harness)).toHaveLength(1);

    await page.evaluate(() => navigator.clipboard.writeText("something else"));
    await page.getByRole("button", { name: "Copy token" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Copied. It will not be shown again." })).toHaveCount(1);
    expect(await clipboard(page)).toBe(token);

    // Put away: the token is gone from the page, and creating another is a new request.
    await page.getByRole("button", { name: "I have stored it" }).click();
    await expect(page.getByLabel("Your new token")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Copy token" })).toHaveCount(0);
    await expectNoTrace(page, csp, harness, token);
    await finish(page, csp, harness);
  });
});
