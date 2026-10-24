// What the page asks for: nothing from another origin, and no API path that is not an operation of the contract (which is
// also how "no AI chat" is kept true). The one write the app makes in T1, sign-out, is checked end to end: a real browser,
// a real Origin header and the double-submit token, accepted by the harness's copy of the hub's CSRF rules.
import { readFileSync } from "node:fs";
import { parse } from "yaml";
import { expect, test } from "../fixtures/test";
import { signOut } from "../fixtures/shell";

const doc = parse(readFileSync(new URL("../../../api/openapi.yaml", import.meta.url), "utf8")) as { paths: Record<string, unknown> };
const templates = Object.keys(doc.paths).map((p) => new RegExp(`^${p.replace(/\{[^}]+\}/g, "[^/]+")}$`));
const isOperation = (path: string): boolean => templates.some((re) => re.test(path));

test("a signed-in visit asks only for its own static files and operations of the contract", async ({ page, harness }) => {
  harness.setScenario("admin");
  const seen: string[] = [];
  page.on("request", (request) => seen.push(request.url()));
  for (const path of ["/swimlanes", "/admin/users", "/approvals", "/search", "/compare"]) await page.goto(`${harness.url}${path}`);
  // The file screen asks for the file, its folder and its swimlane, which the design's file scenario answers (the admin scenario has no files).
  harness.setScenario("design-file-core");
  await page.goto(`${harness.url}/swimlanes/sitb/files?path=${encodeURIComponent("tx-infinity-api/tx-infinity-core-sit7.yml")}`);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  const origin = new URL(harness.url).origin;
  for (const url of seen) {
    const u = new URL(url);
    if (u.protocol === "data:") continue;
    expect(u.origin, url).toBe(origin);
    const static_ = u.pathname === "/" || u.pathname === "/theme-init.js" || u.pathname.startsWith("/assets/") || !u.pathname.includes(".");
    if (!static_) throw new Error(`unexpected path ${u.pathname}`);
    if (u.pathname.startsWith("/api/") || u.pathname.startsWith("/auth/")) expect(isOperation(u.pathname), u.pathname).toBe(true);
  }
  const unmocked = harness.requests().filter((r) => r.unmocked === true);
  expect(unmocked).toEqual([]);
});

test("sign-out is a fetch POST with the real Origin and the CSRF token, and ends on /signin", async ({ page, harness }) => {
  harness.setScenario("viewer");
  await page.goto(`${harness.url}/swimlanes`);
  await expect(page.getByRole("heading", { level: 1, name: "Swimlanes" })).toBeVisible();
  await signOut(page);
  await expect(page).toHaveURL(/\/signin$/);
  const logout = harness.requests().find((r) => r.path === "/auth/logout");
  expect(logout).toBeDefined();
  expect(logout?.method).toBe("POST");
  expect(logout?.origin).toBe(harness.url);
  expect(logout?.csrfHeader).toBe("present");
  expect(logout?.csrf).toBe("ok");
  expect(logout?.status).toBe(204);
  expect(logout?.idempotencyKey ?? "").toMatch(/^[0-9a-f-]{36}$/);
});

test("a write without the token is refused by the hub's rules, which proves the check is live", async ({ page, harness }) => {
  harness.setScenario("viewer");
  await page.goto(`${harness.url}/swimlanes`);
  await expect(page.getByRole("heading", { level: 1, name: "Swimlanes" })).toBeVisible();
  const status = await page.evaluate(async () => (await fetch("/auth/logout", { method: "POST" })).status);
  expect(status).toBe(403);
});
