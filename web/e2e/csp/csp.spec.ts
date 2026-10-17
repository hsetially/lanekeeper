// T1 Verify: "a CSP violation test in Playwright, which runs the app under the production CSP and fails on any violation
// report". The page is served by the hub's own header code (the Rust `web_fixture`), so this is the production policy:
// default-src 'none', nonce-only script and style, Trusted Types required with two policy names, no unsafe-* token.
import { ROUTES, ADMIN_SECTIONS, type RouteDef } from "../../src/routes/table.ts";
import { expect, pageViolations, test } from "../fixtures/test";

const ACCESS_TO_SCENARIO: Record<string, string> = { public: "signed-out", viewer: "viewer", editor: "editor", operator: "operator", admin: "admin" };

function urlFor(route: RouteDef): string {
  let path = route.path;
  path = path.replace(":id", route.path.startsWith("/swimlanes") ? "sit1" : "17").replace(":slug", "L2dpdC9tYWluL2RvY3MvYS5tZA").replace(":section", ADMIN_SECTIONS[0]);
  return path;
}

test.describe("the served policy", () => {
  test("is the production header: Trusted Types required, two policy names, no unsafe token, a fresh nonce", async ({ page, harness }) => {
    const first = await page.goto(`${harness.url}/signin`);
    const csp = first?.headers()["content-security-policy"] ?? "";
    expect(csp).toContain("require-trusted-types-for 'script'");
    expect(csp).toContain("trusted-types lanekeeper-docs lanekeeper-monaco");
    expect(csp).toContain("default-src 'none'");
    expect(csp).not.toMatch(/unsafe-|strict-dynamic|\*/);
    expect(csp).toMatch(/script-src 'self' 'nonce-[0-9a-f]{32}'/);
    expect(csp).toMatch(/style-src 'self' 'nonce-[0-9a-f]{32}'/);
    const nonce = /'nonce-([0-9a-f]{32})'/.exec(csp)?.[1];
    // The page carries the nonce of this very response, and the hub made a new one for the next.
    const inPage = await page.evaluate(() => document.querySelector<HTMLMetaElement>('meta[property="csp-nonce"]')?.nonce ?? "");
    expect(inPage).toBe(nonce);
    const second = await page.goto(`${harness.url}/signin`);
    expect(/'nonce-([0-9a-f]{32})'/.exec(second?.headers()["content-security-policy"] ?? "")?.[1]).not.toBe(nonce);
    expect(first?.headers()["cache-control"]).toBe("no-store");
  });

  test("really watches: a style attribute set from script is refused and the watcher sees it (negative control)", async ({ page, harness, csp }) => {
    await page.goto(`${harness.url}/signin`);
    await expect(page.getByRole("heading", { level: 1, name: "Sign in" })).toBeVisible();
    // The app never does this. The page's own CSP refuses it, which proves the policy is enforced and the watcher is live.
    await page.evaluate(() => document.body.insertAdjacentHTML("beforeend", "<b style=\"top:0\">x</b>")).catch(() => undefined);
    await page.evaluate(() => {
      const div = document.createElement("div");
      div.setAttribute("style", "top:0");
      document.body.append(div);
    });
    await expect.poll(async () => (await pageViolations(page)).length + csp.violations.length).toBeGreaterThan(0);
    csp.clear();
  });
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`every route, ${scheme}`, () => {
    test.use({ colorScheme: scheme });

    for (const route of ROUTES) {
      if (route.redirect) continue; // an alias has no screen of its own; the target is covered
      const scenario = ACCESS_TO_SCENARIO[route.access] ?? "admin";
      test(`${route.path} (${scenario})`, async ({ page, harness, csp }) => {
        harness.setScenario(scenario);
        await page.goto(`${harness.url}${urlFor(route)}`);
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
        // theme-init.js (a classic script from the same origin, no inline script) set the class before the first paint.
        expect(await page.evaluate(() => document.documentElement.classList.contains("dark"))).toBe(scheme === "dark");
        expect(await pageViolations(page), "securitypolicyviolation events").toEqual([]);
        expect(csp.report(), "violations").toBe("");
      });
    }

    test("the landing page leads to the swimlanes", async ({ page, harness, csp }) => {
      harness.setScenario("viewer");
      await page.goto(`${harness.url}/`);
      await expect(page).toHaveURL(/\/swimlanes$/);
      expect(csp.report()).toBe("");
    });

    test("a signed-out visit to a screen that needs a session ends on /signin with a next", async ({ page, harness, csp }) => {
      harness.setScenario("signed-out");
      await page.goto(`${harness.url}/swimlanes/sit1/files?path=a%2Fb.yml`);
      await expect(page).toHaveURL(/\/signin\?next=/);
      await expect(page.getByRole("link", { name: "Sign in with Microsoft" })).toHaveAttribute("href", "/auth/login?return_to=%2Fswimlanes%2Fsit1%2Ffiles%3Fpath%3Da%252Fb.yml");
      expect(csp.report()).toBe("");
    });
  });
}
