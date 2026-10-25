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

export const test = base.extend<{ dist: string; scenario: string; harness: Harness; csp: CspWatch }>({
  // Which build the harness serves (`dist`, or `dist-perf` with the test pages) and which mock scenario starts.
  dist: ["dist", { option: true }],
  scenario: ["viewer", { option: true }],
  harness: async ({ dist, scenario }, use) => {
    const harness = await startHarness({ dist, scenario });
    try {
      await use(harness);
    } finally {
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
});

export { expect };
