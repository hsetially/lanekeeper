// The Playwright `test` every spec uses: a harness per test (the hub's header code plus mock scenarios), and a CSP watcher
// that fails the test on what the policy refuses.
//
// What the watcher fails on (S12): a `securitypolicyviolation` event, a Chromium `ContentSecurityPolicyIssue` (from the
// Audits domain), a Trusted Types or CSP error in the console of the page or of a worker, and any uncaught exception. It does
// NOT fail on every console message: Chromium 141 logs a warning for a Permissions-Policy feature it does not know, other
// Audits issues are about form fields (G0 finding 3). Those are not CSP violations.
import { test as base, expect, type Page } from "@playwright/test";
import { startHarness } from "../harness/serve.ts";

type Harness = Awaited<ReturnType<typeof startHarness>>;
export type Violation = { source: string; detail: string };

const POLICY_TEXT = /content security policy|refused to|trustedhtml|trustedscript|trusted ?type|require-trusted-types|violates the following/i;

export class CspWatch {
  readonly violations: Violation[] = [];
  /** Console messages that are not violations, kept so a failing test can show what else the page said. */
  readonly other: string[] = [];

  add(source: string, detail: string): void {
    this.violations.push({ source, detail });
  }

  clear(): void {
    this.violations.length = 0;
  }

  report(): string {
    return this.violations.map((v) => `${v.source}: ${v.detail}`).join("\n");
  }
}

/** Install the watchers on a page. Call before the first navigation. */
export async function watchCsp(page: Page, watch: CspWatch): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as { __lkViolations?: Array<Record<string, unknown>> };
    w.__lkViolations = [];
    document.addEventListener("securitypolicyviolation", (event) => {
      w.__lkViolations?.push({ directive: event.effectiveDirective, blocked: event.blockedURI, sample: event.sample, source: event.sourceFile, line: event.lineNumber, disposition: event.disposition });
    });
  });
  page.on("pageerror", (error) => watch.add("pageerror", error.message.slice(0, 300)));
  page.on("console", (message) => {
    const text = message.text();
    if (message.type() === "error" && POLICY_TEXT.test(text)) watch.add("console", text.slice(0, 300));
    else watch.other.push(`${message.type()}: ${text.slice(0, 200)}`);
  });
  page.on("worker", (worker) => {
    // Console output of a worker is not in page.on('console'); the editor worker runs under the API policy.
    worker.on("console", (message) => {
      if (POLICY_TEXT.test(message.text())) watch.add("worker console", message.text().slice(0, 300));
      else watch.other.push(`worker ${message.type()}: ${message.text().slice(0, 200)}`);
    });
  });
  const session = await page.context().newCDPSession(page);
  await session.send("Audits.enable");
  session.on("Audits.issueAdded", (event: { issue: { code: string; details?: { contentSecurityPolicyIssueDetails?: { violatedDirective?: string; blockedURL?: string } } } }) => {
    if (event.issue.code === "ContentSecurityPolicyIssue") {
      const d = event.issue.details?.contentSecurityPolicyIssueDetails;
      watch.add("audits", `${d?.violatedDirective ?? "?"} blocked ${d?.blockedURL ?? "inline"}`);
    }
  });
}

/** Violation events recorded by the page itself since it loaded. */
export async function pageViolations(page: Page): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(() => (window as unknown as { __lkViolations?: Array<Record<string, unknown>> }).__lkViolations ?? []);
}

/** Remove every route handler of the page and of its context; those still in flight are told to ignore their errors. Never throws. */
async function endRoutes(page: Page): Promise<void> {
  await page.unrouteAll({ behavior: "ignoreErrors" }).catch(() => undefined);
  await page.context().unrouteAll({ behavior: "ignoreErrors" }).catch(() => undefined);
}

export const test = base.extend<{ dist: string; scenario: string; harness: Harness; csp: CspWatch; routesEnd: undefined }>({
  // Which build the harness serves (`dist`, or `dist-perf` with the test pages) and which mock scenario starts.
  dist: ["dist", { option: true }],
  scenario: ["viewer", { option: true }],
  // Depends on `page` (every test has one: the `csp` fixture needs it), so that it tears down before the page and the context do.
  harness: async ({ dist, scenario, page }, use) => {
    const harness = await startHarness({ dist, scenario });
    try {
      await use(harness);
    } finally {
      // A route handler lives as long as its page, and one that calls `route.fetch()` can still be inside it when the test body ends (the
      // page asks once more just before the end). If the harness closed its connections first, the handler would throw `route.fetch: read
      // ECONNRESET` (or `aborted`); if the context closed first, `apiResponse.json: Response has been disposed`; and the throw becomes the
      // test's failure, which it did on main's last `web-e2e` run (e5ed17e) and about one run in fifty here. So the handlers go first, and
      // the ones in flight are told to ignore their errors (`behavior: "wait"` waits for requests that a test leaves unanswered on purpose,
      // so it never finishes). The test body is untouched, so what a test asserts is what it asserted before; the teardown itself is
      // proved by e2e/harness/teardown.spec.ts.
      await endRoutes(page);
      await harness.close();
    }
  },
  csp: [
    async ({ page }, use) => {
      const watch = new CspWatch();
      await watchCsp(page, watch);
      await use(watch);
    },
    { auto: true },
  ],
  // The same at the end of a test that never asked for the harness (the harness fixture does it first for the others).
  routesEnd: [
    async ({ page }, use) => {
      await use(undefined);
      await endRoutes(page);
    },
    { auto: true },
  ],
});

export { expect };
