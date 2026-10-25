// The XSS corpus for the one HTML sink of the app (S12): SanitisedDocHtml, which inserts `RenderedDoc.html`.
//
// The server sanitises that HTML (CG11) and the web never does. The Trusted Types policy of the sink, `lanekeeper-docs`, is a
// pass-through, so if the server's sanitiser ever FAILED, the only things between an attack and the person would be the Content-Security-
// Policy and Trusted Types. This spec makes the sanitiser fail on purpose: it answers the render request with a hostile page from the
// contract's own corpus (e2e/csp/xss-corpus.ts) and watches what the browser does under the REAL header, served by the hub's header
// code (the Rust `web_fixture`), never a copy.
//
// For every hostile page, through the reader's two addresses (`/docs/view?path=` and `/docs/:slug`), in light and dark:
//  - nothing ran: no alert, confirm, prompt or dialog, and the sentinel `window.__xss` was never written (also after every inline handler
//    was fired, every link clicked and every form submitted, and also from inside a frame);
//  - nothing left the origin by itself: no request to another origin while the page loads, and none from a form. A click on a link that
//    names another origin navigates there, as a link does, and the CSP cannot govern that (the server's allowlist is the control for an
//    href); the only off-origin request after a click is that document navigation, to the link's own address;
//  - the violations the browser reported are the expected ones (`blocks` of the case must all be there, and nothing outside the case's
//    and the category's lists may be), no console error other than the CSP reports, no uncaught exception;
//  - the hostile element is IN the DOM (`present`), so the policy, and not the app dropping the input, is what stopped it;
//  - the page and the sidebar keep working.
// The spec is not vacuous: it first asserts the served header (Trusted Types required, no `unsafe-` token), the negative controls show
// the watcher records what the policy refuses (a string to an HTML sink, eval, a second policy), and a mutation of the policy or of the
// sink turns it red (plans/08-frontend.blockers.md, "FIX round 1 (E2E)").
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { test as plain } from "@playwright/test";
import type { Page, Request, TestInfo } from "@playwright/test";
import { expect, test, type CspWatch } from "../fixtures/test";
import type { Harness } from "../harness/serve.ts";
import { CASES, CATEGORIES, CATEGORY_MAY, CATEGORY_TITLE, CHECKER_LABELS, CORPUS_NAMES, GAPS, type Category, type XssCase } from "./xss-corpus.ts";

const DIST = process.env["LK_E2E_DIST"] ?? "dist";
/** LK_XSS_REPORT=1 prints, for each category, what the browser reported (the record behind the `blocks` and `may` lists). */
const REPORT = process.env["LK_XSS_REPORT"] === "1";

// A doc the search scenario knows (a Git doc, read-only); only its rendered page is replaced.
const DOC_PATH = "/git/main/docs/account-sorting.md";
type Entry = "view" | "slug";
const ENTRY_URL: Record<Entry, string> = {
  view: `/docs/view?path=${encodeURIComponent(DOC_PATH)}`,
  slug: `/docs/${Buffer.from(DOC_PATH, "utf8").toString("base64url")}`,
};
const RENDER = /\/api\/v1\/docs\/fs\/render\?/;

test.use({ dist: DIST, scenario: "design-search-editor" });

// ---------------------------------------------------------------------------------------------------------------------------------
// The served policy

/** The production header, as the other CSP specs state it. A relaxed fixture would make everything below vacuous, so it is checked at every load. */
function productionPolicyProblems(csp: string | undefined): string[] {
  const problems: string[] = [];
  if (csp === undefined || csp === "") return ["the response has no Content-Security-Policy header"];
  if (!csp.includes("require-trusted-types-for 'script'")) problems.push("require-trusted-types-for 'script' is missing");
  if (!csp.includes("trusted-types lanekeeper-docs lanekeeper-monaco")) problems.push("the trusted-types list is not the two policy names");
  if (/unsafe-/.test(csp)) problems.push("the policy has an unsafe- token");
  if (!csp.includes("default-src 'none'")) problems.push("default-src 'none' is missing");
  if (!/script-src 'self' 'nonce-[0-9a-f]{32}'(;|$)/.test(csp)) problems.push("script-src is not 'self' plus a nonce");
  if (!/style-src 'self' 'nonce-[0-9a-f]{32}'(;|$)/.test(csp)) problems.push("style-src is not 'self' plus a nonce");
  if (!csp.includes("form-action 'self'")) problems.push("form-action 'self' is missing");
  if (!csp.includes("base-uri 'none'")) problems.push("base-uri 'none' is missing");
  if (!csp.includes("object-src 'none'")) problems.push("object-src 'none' is missing");
  return problems;
}

// ---------------------------------------------------------------------------------------------------------------------------------
// What the page did

type Phase = "load" | "interact";
type Reported = { directive: string; blocked: string; sample: string; frame: string };
/**
 * A request to another origin that REACHED the network: the policy did not stop it. (A request the CSP blocks is announced to the page's
 * watchers by the browser, and then fails; it never reaches the network, so it never reaches the route that answers it here. Counting
 * announced requests would count every request the policy stopped.)
 */
type Foreign = { url: string; navigation: boolean; type: string; method: string; phase: Phase };

class Observed {
  executed: string[] = [];
  violations: Reported[] = [];
  foreign: Foreign[] = [];
  /** Requests the page announced for another origin, whether or not the policy stopped them. */
  announced: string[] = [];
  clicked: string[] = [];
  phase: Phase = "load";
  reset(): void {
    this.executed = [];
    this.violations = [];
    this.foreign = [];
    this.announced = [];
    this.clicked = [];
    this.phase = "load";
  }
}

const isWeb = (u: URL): boolean => u.protocol === "http:" || u.protocol === "https:" || u.protocol === "ws:" || u.protocol === "wss:";

/** Install the watchers on a page. Call before the first navigation. */
async function observe(page: Page, origin: string): Promise<Observed> {
  const seen = new Observed();
  await page.exposeFunction("__lkXss", (kind: string, detail: string) => {
    if (kind === "csp") seen.violations.push(JSON.parse(detail) as Reported);
    else seen.executed.push(`${kind}: ${detail}`);
  });
  await page.addInitScript(() => {
    const w = window as unknown as Record<string, unknown> & { __lkXss?: (kind: string, detail: string) => Promise<void> };
    const tell = (kind: string, detail: unknown): void => {
      try {
        void w.__lkXss?.(kind, String(detail).slice(0, 300));
      } catch {
        // The binding is gone while the page is being torn down.
      }
    };
    // What a payload would call. The page keeps working; the call is recorded.
    for (const name of ["alert", "confirm", "prompt"]) w[name] = (message: unknown): void => tell(name, message);
    // The sentinel of the payloads that write instead of calling: any write to it, from any frame, is an execution.
    Object.defineProperty(window, "__xss", { configurable: true, get: () => undefined, set: (value: unknown) => tell("sentinel", value) });
    document.addEventListener(
      "securitypolicyviolation",
      (event) => {
        tell("csp", JSON.stringify({ directive: event.effectiveDirective, blocked: event.blockedURI, sample: event.sample, frame: location.href.slice(0, 40) }));
      },
      true,
    );
  });
  page.on("dialog", (dialog) => {
    seen.executed.push(`dialog ${dialog.type()}: ${dialog.message()}`);
    dialog.dismiss().catch(() => undefined);
  });
  page.on("request", (request: Request) => {
    const url = new URL(request.url());
    if (isWeb(url) && url.origin !== origin) seen.announced.push(request.url());
  });
  // A link with target=_blank opens a window of its own, which the page's watchers do not see; the window is closed at once.
  page.context().on("page", (popup) => {
    void popup.close().catch(() => undefined);
  });
  // Nothing in this run may reach a real network: a request the policy failed to stop arrives here, is recorded, and is answered.
  await page.context().route(
    (url) => isWeb(url) && url.origin !== origin,
    (route) => {
      const request = route.request();
      // The frame of a request from a window that was only just opened does not exist yet (Playwright throws): it is a navigation all the same.
      let navigation = request.isNavigationRequest();
      try {
        navigation = navigation && request.frame() === page.mainFrame();
      } catch {
        // keep the answer above
      }
      seen.foreign.push({ url: request.url(), navigation, type: request.resourceType(), method: request.method(), phase: seen.phase });
      return route.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html><title>offsite</title>" });
    },
  );
  return seen;
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Running one hostile page

const hashOf = (name: string): string => createHash("sha256").update(name).digest("hex");

type Run = { page: Page; harness: Harness; csp: CspWatch; seen: Observed; entry: Entry; scheme: "light" | "dark"; current: { html: string; hash: string }; problems: string[] };

async function chromeWorks(page: Page): Promise<void> {
  await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
  await expect(page.getByRole("complementary", { name: "Primary" })).toBeVisible();
}

/** Wait for what the page does on its own after it is inserted: images that settle, frames that load, handlers that fire. */
async function settle(page: Page, ms = 300): Promise<void> {
  await page
    .evaluate(async () => {
      await Promise.all(
        [...document.images].map(
          (image) =>
            image.complete
              ? Promise.resolve()
              : new Promise<void>((done) => {
                  image.addEventListener("load", () => done());
                  image.addEventListener("error", () => done());
                }),
        ),
      );
    })
    .catch(() => undefined);
  await page.waitForTimeout(ms);
}

/** Open the doc and wait for the hostile page to be inserted. False when the document is not there: the page left it by itself. */
async function visit(run: Run, wait = 8_000): Promise<boolean> {
  const { page, harness } = run;
  const response = await page.goto(`${harness.url}${ENTRY_URL[run.entry]}`);
  const policy = productionPolicyProblems(response?.headers()["content-security-policy"]);
  const said = `served policy: ${policy.join("; ")}`;
  if (policy.length > 0 && !run.problems.includes(said)) run.problems.push(said);
  try {
    await expect(page.locator(`[data-content-hash="${run.current.hash}"]`)).toBeAttached({ timeout: wait });
    await page.evaluate(() => {
      (window as unknown as { __lkDoc?: boolean }).__lkDoc = true;
    });
  } catch {
    // The document went away under the wait: a navigation the page started by itself.
    return false;
  }
  await settle(page);
  return true;
}

/** Whether the page is still the document `visit` loaded (a click or a submit that navigated made it another one). */
const sameDocument = (page: Page): Promise<boolean> => page.evaluate(() => (window as unknown as { __lkDoc?: boolean }).__lkDoc === true).catch(() => false);

/** Fire the event of every inline handler attribute in the page, as the person's pointer or the page's own events would. */
async function fireHandlers(page: Page): Promise<void> {
  await page
    .evaluate(() => {
      for (const element of document.querySelectorAll(".doc-prose *")) {
        for (const attribute of [...element.attributes]) {
          if (/^on[a-z]+$/i.test(attribute.name)) element.dispatchEvent(new Event(attribute.name.slice(2).toLowerCase(), { bubbles: true, cancelable: true }));
        }
        if (element instanceof HTMLElement) element.focus();
      }
    })
    .catch(() => undefined);
  await page.waitForTimeout(150);
}

const LINKS = ".doc-prose a, .doc-prose area";

/** Click link `index` as a person does (and by dispatching the click where it has no box to click, SVG and image maps). */
async function clickLink(run: Run, index: number): Promise<void> {
  const link = run.page.locator(LINKS).nth(index);
  const href = await link
    .evaluate((element) => {
      const raw = element.getAttribute("href") ?? element.getAttribute("xlink:href") ?? "";
      try {
        return new URL(raw, document.baseURI).href;
      } catch {
        return raw;
      }
    })
    .catch(() => "");
  run.seen.clicked.push(href);
  await link.click({ timeout: 2_000, noWaitAfter: true }).catch(() =>
    link.evaluate((element) => element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }))).catch(() => undefined),
  );
  await run.page.waitForTimeout(300);
}

/** Submit form `index`, as its submit button would (or by `requestSubmit`, then `submit`, where it has none). */
async function submitForm(run: Run, index: number): Promise<void> {
  await run.page
    .locator(".doc-prose form")
    .nth(index)
    .evaluate((form) => {
      const f = form as HTMLFormElement;
      const button = f.querySelector("button, input[type=submit]");
      try {
        f.requestSubmit(button instanceof HTMLButtonElement || button instanceof HTMLInputElement ? button : null);
      } catch {
        try {
          f.submit();
        } catch {
          // A form that cannot be submitted is not an attack.
        }
      }
    })
    .catch(() => undefined);
  await run.page.waitForTimeout(300);
}

const NO_CONSOLE_NOISE = [
  /Not allowed to navigate top frame to data URL/,
  /Blocked setting the .* cookie from a `<meta>` tag/,
  // A scheme the browser does not open from a page (vbscript:, and mailto: with no mail client).
  /^error: Not allowed to launch /,
  // The reader asked this origin for a file it does not have (`<image src=x>`, a relative link): the hub answers 404.
  /Failed to load resource: the server responded with a status of 40[0-9]/,
  // The browser's own refusals, before any policy: an SVG <use> of another origin, and a blob:, file: or view-source: URL.
  /^error: Unsafe attempt to load URL /,
  /^error: Not allowed to load local resource: /,
];

function judge(run: Run, c: XssCase, present: number | undefined, left: boolean): void {
  const { seen, csp, harness } = run;
  const fail = (message: string): void => {
    run.problems.push(`[${c.name}] ${message}`);
  };
  if (seen.executed.length > 0) fail(`SCRIPT RAN: ${seen.executed.join(" | ")}`);

  // A way out the policy does not close (GAPS) must be open, exactly as recorded: the page leaves the document by itself. Anything else
  // that leaves it is a failure, and so is a gap that has been closed (the record would go stale).
  if (c.gap === undefined && left) fail(`the hostile page is not in the document: it never rendered, or the page left by itself (now ${run.page.url().slice(0, 100)})`);
  if (c.gap !== undefined && !left) fail(`the known gap \`${c.gap}\` is closed: this page no longer leaves the document. Remove \`gap\` from the case and from GAPS.`);

  // Nothing left the origin by itself. After a click, the one request allowed is the navigation to the clicked link's own address.
  for (const request of seen.foreign) {
    if (c.gap !== undefined && request.phase === "load" && request.navigation && request.type === "document") continue; // the open gap
    const ownLink = request.navigation && request.type === "document" && request.method === "GET" && seen.clicked.some((href) => href.replace(/#.*$/, "") === request.url.replace(/#.*$/, ""));
    if (request.phase === "load" || !ownLink) fail(`a request left the origin (${request.phase}, ${request.type}${request.navigation ? ", navigation" : ""}): ${request.url.slice(0, 120)}`);
  }

  // The violations are the expected ones.
  const seenDirectives = new Set(seen.violations.map((v) => v.directive));
  const allowed = new Set([...CATEGORY_MAY[c.category], ...(c.may ?? []), ...(c.blocks ?? [])]);
  const unexpected = [...seenDirectives].filter((d) => !allowed.has(d));
  if (unexpected.length > 0) fail(`unexpected violations: ${unexpected.join(", ")} (allowed here: ${[...allowed].join(", ") || "none"})`);
  const missing = (c.blocks ?? []).filter((d) => !seenDirectives.has(d));
  if (missing.length > 0) fail(`the policy did not report ${missing.join(", ")} (it reported: ${[...seenDirectives].join(", ") || "nothing"}): is the directive relaxed?`);

  // The console says nothing but CSP reports, and the page raised no uncaught exception.
  const exceptions = csp.violations.filter((v) => v.source === "pageerror");
  if (exceptions.length > 0) fail(`uncaught exception: ${exceptions.map((v) => v.detail).join(" | ")}`);
  const errors = csp.other.filter((line) => /^(worker )?error:/.test(line)).filter((line) => !NO_CONSOLE_NOISE.some((re) => re.test(line)));
  if (errors.length > 0) fail(`console errors: ${errors.join(" | ")}`);

  // No write was accepted by the hub because of this page.
  const writes = harness.requests().filter((r) => r.method !== "GET" && r.kind !== "stream" && r.status < 400);
  if (writes.length > 0) fail(`the hub accepted a write: ${writes.map((r) => `${r.method} ${r.path} ${r.status}`).join(", ")}`);

  // The hostile element reached the DOM (so the policy stopped it, not the app).
  if (c.present !== undefined && present === 0) fail(`the hostile element \`${c.present}\` is not in the DOM: the page never reached the sink as it was written`);
}

type Result = { directives: string[]; reached: number; clicked: number; announced: number };
/** `reached`: requests to another origin that reached the network by themselves. `clicked`: navigations to the clicked link's own address. */
const resultOf = (seen: Observed): Result => ({
  directives: [...new Set(seen.violations.map((v) => v.directive))].sort(),
  reached: seen.foreign.filter((f) => f.phase === "load" || !f.navigation).length,
  clicked: seen.foreign.filter((f) => f.phase === "interact" && f.navigation).length,
  announced: seen.announced.length,
});

/** The violations a case must see arrive from the browser a moment after the page does; give a slow machine up to three seconds before calling one missing. */
async function waitForBlocks(run: Run, c: XssCase): Promise<void> {
  const missing = (): number => (c.blocks ?? []).filter((d) => !run.seen.violations.some((v) => v.directive === d)).length;
  for (let waited = 0; missing() > 0 && waited < 3_000; waited += 100) await run.page.waitForTimeout(100);
}

async function runCase(run: Run, c: XssCase): Promise<Result> {
  const { page, harness, csp, seen } = run;
  seen.reset();
  csp.clear();
  csp.other.length = 0;
  harness.clearRequests();
  run.current = { html: c.html, hash: hashOf(`${run.entry}/${run.scheme}/${c.name}`) };

  // A page with a known gap leaves at once; waiting the full time for a document that is not coming only slows the run.
  if (!(await visit(run, c.gap === undefined ? 8_000 : 4_000))) {
    judge(run, c, undefined, true);
    return resultOf(seen);
  }
  expect(await page.evaluate(() => document.documentElement.classList.contains("dark"))).toBe(run.scheme === "dark");
  await chromeWorks(page);
  const present = c.present === undefined ? undefined : await page.locator(`.doc-prose ${c.present}`).count();

  seen.phase = "interact";
  await fireHandlers(page);
  // A page that sends the person away by itself (a meta refresh) does it within milliseconds; if the document is not the one that was loaded, it has.
  const left = !(await sameDocument(page));
  if (left) {
    await waitForBlocks(run, c);
    judge(run, c, present, true);
    return resultOf(seen);
  }
  const links = await page.locator(LINKS).count();
  const forms = await page.locator(".doc-prose form").count();
  for (let i = 0; i < links; i += 1) {
    if (!(await sameDocument(page)) && !(await visit(run))) break;
    await clickLink(run, i);
  }
  for (let i = 0; i < forms; i += 1) {
    if (!(await sameDocument(page)) && !(await visit(run))) break;
    await submitForm(run, i);
  }
  await waitForBlocks(run, c);
  judge(run, c, present, false);
  return resultOf(seen);
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The specs

/** LK_XSS_ONLY=<regular expression> runs only the cases whose names match it (to look at one). */
const ONLY = process.env["LK_XSS_ONLY"] === undefined ? undefined : new RegExp(process.env["LK_XSS_ONLY"]);
/** Every page runs in light through `/docs/view`; the small `core` set (one or more per category) also runs in dark and through `/docs/:slug`. */
const byCategory = (category: Category, scheme: "light" | "dark", entry: Entry): XssCase[] =>
  CASES.filter((c) => c.category === category && (c.core === true || (scheme === "light" && entry === "view")) && (ONLY === undefined || ONLY.test(c.name)));

/** Answer the render request with the page the case names. The route is made once; `run.current` says which. */
async function serveHostile(run: Run): Promise<void> {
  await run.page.route(RENDER, (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      headers: { "cache-control": "no-store" },
      body: JSON.stringify({ path: DOC_PATH, html: run.current.html, headings: [], content_hash: run.current.hash }),
    }),
  );
}

async function makeRun(page: Page, harness: Harness, csp: CspWatch, entry: Entry, scheme: "light" | "dark"): Promise<Run> {
  const seen = await observe(page, new URL(harness.url).origin);
  const run: Run = { page, harness, csp, seen, entry, scheme, current: { html: "", hash: "" }, problems: [] };
  await serveHostile(run);
  return run;
}

const note = (info: TestInfo, title: string, rows: Array<Record<string, unknown>>): void => {
  void info.attach(title, { body: JSON.stringify(rows, null, 1), contentType: "application/json" });
  if (REPORT) process.stdout.write(`${title}\n${rows.map((r) => `  ${JSON.stringify(r)}`).join("\n")}\n`);
};

for (const scheme of ["light", "dark"] as const) {
  for (const entry of ["view", "slug"] as const) {
    test.describe(`hostile docs, ${scheme}, ${entry === "view" ? "/docs/view?path=" : "/docs/:slug"}`, () => {
      test.use({ colorScheme: scheme });

      for (const category of CATEGORIES) {
        const cases = byCategory(category, scheme, entry);
        if (cases.length === 0) continue;
        test(`${CATEGORY_TITLE[category]} (${cases.length})`, async ({ page, harness, csp }, info) => {
          test.setTimeout(60_000 + cases.length * 9_000);
          const run = await makeRun(page, harness, csp, entry, scheme);
          const rows: Array<Record<string, unknown>> = [];
          for (const c of cases) {
            const result = await runCase(run, c);
            rows.push({ case: c.name, violations: result.directives, reachedNetwork: result.reached, navigatedByClick: result.clicked, stoppedRequests: Math.max(0, result.announced - result.reached - result.clicked) });
          }
          note(info, `xss ${scheme} ${entry} ${category}: ${cases.length} pages`, rows);
          for (const c of cases) if (c.gap !== undefined) info.annotations.push({ type: "known-gap", description: `${c.name}: ${GAPS[c.gap]}` });
          // What went wrong with the pages comes first (it is the cause); the last step then says whether the app still works.
          expect.soft(run.problems, `${CATEGORY_TITLE[category]}, ${scheme}, ${entry}`).toEqual([]);
          // With a hostile page in the reader (the last one that stays; a page that sends the person away by itself cannot show this), the app
          // still works: the sidebar takes the person to another screen.
          const staying = [...cases].reverse().find((c) => c.gap === undefined);
          if (staying !== undefined) {
            run.current = { html: staying.html, hash: hashOf(`${entry}/${scheme}/${staying.name}`) };
            expect(await visit(run), "the last hostile page stays in the reader").toBe(true);
            await page.getByRole("complementary", { name: "Primary" }).getByRole("link", { name: "Swimlanes" }).click();
            await expect(page.getByRole("heading", { level: 1, name: "Swimlanes" })).toBeVisible();
          }
        });
      }
    });
  }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The spec is not vacuous

/**
 * What a hostile page would try from script, as one expression run in the page: each attempt must throw. It is a string on purpose: the
 * calls are the very ones the lint bans (eval, new Function, a string to setTimeout) and they are made here to be refused.
 */
const ATTEMPTS = `(() => {
  const results = {};
  const attempt = (name, fn) => { try { fn(); results[name] = 'allowed'; } catch (error) { results[name] = 'refused: ' + error.name; } };
  // Run by the page itself, from a timer: code a debugger runs may be allowed to eval, and that is not the page.
  setTimeout(() => {
    attempt('insertAdjacentHTML(string)', () => document.body.insertAdjacentHTML('beforeend', '<b id="tt-string">x</b>'));
    attempt('iframe.srcdoc = string', () => { document.createElement('iframe').srcdoc = '<b>x</b>'; });
    attempt('script.textContent = string', () => { const s = document.createElement('script'); s.textContent = 'window.__xss = 1'; document.body.append(s); });
    attempt('eval(string)', () => (0, eval)('window.__xss = 2'));
    attempt('new Function(string)', () => new Function('window.__xss = 3')());
    attempt('setTimeout(string)', () => setTimeout('window.__xss = 4', 0));
    attempt('a second policy', () => window.trustedTypes.createPolicy('evil', { createHTML: (s) => s }));
    attempt('lanekeeper-docs again', () => window.trustedTypes.createPolicy('lanekeeper-docs', { createHTML: (s) => s }));
    window.__lkAttempts = results;
  }, 0);
})()`;

test.describe("the watcher and the policy are live (negative controls)", () => {
  test("the page is served with the production policy: Trusted Types required, two names, no unsafe- token, a nonce", async ({ page, harness }) => {
    const response = await page.goto(`${harness.url}${ENTRY_URL.view}`);
    expect(productionPolicyProblems(response?.headers()["content-security-policy"])).toEqual([]);
    // The check itself can fail: each way of relaxing the header is named.
    const header = response?.headers()["content-security-policy"] ?? "";
    expect(productionPolicyProblems(header.replace("require-trusted-types-for 'script'", ""))).toContain("require-trusted-types-for 'script' is missing");
    expect(productionPolicyProblems(`${header}; script-src 'unsafe-inline'`)).toContain("the policy has an unsafe- token");
    expect(productionPolicyProblems(header.replace("form-action 'self'", "form-action *"))).toContain("form-action 'self' is missing");
    expect(productionPolicyProblems(undefined)).toHaveLength(1);
  });

  test("a string that reaches a sink is refused and reported; eval, inline script and a second policy are refused too", async ({ page, harness, csp }) => {
    const run = await makeRun(page, harness, csp, "view", "light");
    run.current = { html: '<h2 id="doc-a">a</h2><p>fine</p>', hash: hashOf("negative-controls") };
    await visit(run);
    await chromeWorks(page);
    run.seen.reset();
    await page.evaluate(ATTEMPTS);
    await page.waitForFunction(() => "__lkAttempts" in window);
    const outcome = await page.evaluate(() => (window as unknown as { __lkAttempts: Record<string, string> }).__lkAttempts);
    await page.waitForTimeout(300);
    // Every attempt was refused by the browser, and the sentinel was never written.
    for (const [name, result] of Object.entries(outcome)) expect(result, name).toMatch(/^refused/);
    expect(Object.keys(outcome)).toHaveLength(8);
    expect(await page.locator("#tt-string").count()).toBe(0);
    expect(run.seen.executed).toEqual([]);
    // And the watcher SAW them: this is what "zero unexpected violations" in the corpus is measured with.
    const directives = new Set(run.seen.violations.map((v) => v.directive));
    for (const wanted of ["require-trusted-types-for", "trusted-types"]) expect([...directives], `the watcher saw ${wanted}`).toContain(wanted);
    csp.clear();
  });

  test("behind the reader: the policy refuses a base and a stylesheet link of another origin that are added to the page without it", async ({ page, harness, csp }) => {
    // The reader removes `meta`, `base` and `link` from a doc (`withoutHeadTags`), so no corpus page can show what the CSP does with a base
    // or a stylesheet link. This control adds one of each to the page with DOM calls (no string, so Trusted Types has nothing to say) and
    // watches the policy: the base must not move the page's base address, and the stylesheet must not reach the network. (A `meta` refresh
    // is the one head element the policy cannot refuse; the reader's removal is the only thing between a doc and it.)
    const run = await makeRun(page, harness, csp, "view", "light");
    run.current = { html: "<p>fine</p>", hash: hashOf("head-controls") };
    await visit(run);
    await chromeWorks(page);
    run.seen.reset();
    const before = await page.evaluate(() => document.baseURI);
    await page.evaluate(() => {
      const base = document.createElement("base");
      base.setAttribute("href", "https://evil.example/");
      document.head.append(base);
      const link = document.createElement("link");
      link.setAttribute("rel", "stylesheet");
      link.setAttribute("href", "https://evil.example/x.css");
      document.head.append(link);
    });
    await expect.poll(() => run.seen.violations.map((v) => v.directive).sort(), { timeout: 5_000 }).toEqual(expect.arrayContaining(["base-uri", "style-src-elem"]));
    expect(await page.evaluate(() => document.baseURI), "the base did not move the page's base address").toBe(before);
    expect(run.seen.foreign, "nothing reached another origin").toEqual([]);
    expect(run.seen.executed).toEqual([]);
    csp.clear();
  });

  test("the docs policy is created once, at the first page, and a hostile page cannot make it again", async ({ page, harness, csp }) => {
    const run = await makeRun(page, harness, csp, "view", "light");
    run.current = { html: "<p>fine</p>", hash: hashOf("policy-once") };
    await visit(run);
    run.seen.reset();
    const again = await page.evaluate<string>(`(() => {
      try { window.trustedTypes.createPolicy('lanekeeper-docs', { createHTML: (s) => s }); return 'created'; } catch (error) { return error.name; }
    })()`);
    expect(again).toBe("TypeError");
    await page.waitForTimeout(150);
    expect(run.seen.violations.map((v) => v.directive)).toContain("trusted-types");
    csp.clear();
  });
});

// ---------------------------------------------------------------------------------------------------------------------------------
// The corpus is the contract's

plain.describe("the fixture holds the contract's inputs", () => {
  const rust = (path: string): string => readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8");

  plain("every entry of XSS_CORPUS (crates/ports) is a case, with the Rust name", () => {
    const source = rust("crates/ports/src/conformance/docs.rs");
    const block = source.slice(source.indexOf("pub const XSS_CORPUS"), source.indexOf("];", source.indexOf("pub const XSS_CORPUS")));
    const names = [...block.matchAll(/\(\s*"([a-z0-9-]+)",/g)].map((m) => m[1] ?? "");
    expect(names.length).toBeGreaterThan(40);
    expect([...CORPUS_NAMES].sort()).toEqual([...names].sort());
  });

  plain("every label of the checker's table of refused pages is a case, with the Rust label", () => {
    const source = rust("crates/ports/tests/web_contract_docs.rs");
    const start = source.indexOf("fn the_checker_refuses_every_way_out_of_the_allowlist");
    const block = source.slice(start, source.indexOf("let labels", start));
    // A tuple `("label", html)` or `(\n "label",`; not the `format!("...", ..)` calls that build a page.
    const labels = [...block.matchAll(/(?<!format!)\(\s*"([^"]+)",/g)].map((m) => m[1] ?? "");
    expect(labels.length).toBeGreaterThan(60);
    expect([...CHECKER_LABELS].sort()).toEqual([...labels].sort());
  });

  plain("case names are unique and every category has a case", () => {
    const names = CASES.map((c) => c.name);
    expect(new Set(names).size).toBe(names.length);
    for (const category of CATEGORIES) expect(CASES.some((c) => c.category === category), category).toBe(true);
  });
});
