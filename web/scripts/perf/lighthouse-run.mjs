// P10: Lighthouse performance, desktop preset, against the harness (the hub's own header code with the mock API behind it).
//
// This is the plan's W31 fallback for `@lhci/cli` (nine audit findings in its tree): lighthouse 13.5.0 and this runner, told
// the same things by the same file (`lighthouserc.json`). For every page, N runs, each in a Chromium of its own with a clean
// profile; the page's score is the median of its runs and the budget is the lowest page (lighthouse-score.mjs). The result is
// written for `cargo xtask bench-check` as `web/lighthouse_performance` with `values.p50` in points (score times 100), and
// carries what Lighthouse measured (FCP, LCP, TBT, CLS, Speed Index) so that a miss can be read and not guessed at.
//
// Throttling is Lighthouse's own simulation of the desktop preset (10 Mbps, 40 ms round trip, no CPU slowdown). The hub sends
// no Content-Encoding (W28), so the pages are measured as the hub serves them.
//
// Everything here is bounded (AGENTS.md rule 5), because the first run on GitHub hung for 38 minutes with nothing in the log:
// the browser (chromium-process.mjs: it starts in `startTimeoutMs`, a browser that exits is reported with what it printed, and
// stop() cannot wait for ever), one Lighthouse run (`runTimeoutMs`, or the moment the browser dies), the harness's close
// (`closeTimeoutMs`) and the whole gate (`deadlineMs`). Each ends in an error that names what was waited for.
//
// This module takes the parts that are not its own (the harness, Lighthouse, which browser) so that a test can put stand-ins in
// their place; lighthouse.mjs is the command that gives it the real ones.
//
// Exit codes of the command: 0 every page's median reaches the score; 1 a page's median is under it; 2 a run could not be made
// (no browser, Lighthouse failed, no score, a bound was reached): nothing is written then, so a broken run cannot pass.
import { readFileSync } from "node:fs";
import { cpus } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { within } from "./bounded.mjs";
import { killAllChromium, startChromium } from "./chromium-process.mjs";
import { aggregate, parseConfig, retryUnrecorded, toPoints } from "./lighthouse-score.mjs";
import { resultsDir as defaultResultsDir, writeResult } from "./results.mjs";

const WEB_ROOT = fileURLToPath(new URL("../../", import.meta.url));

/**
 * A run takes about a minute (54 s measured on a quiet 4-core machine: Lighthouse waits for a network that the live event stream
 * never lets go quiet), so the nine runs of the gate take about eight minutes. `runTimeoutMs` is three times a run, and
 * `deadlineMs` two and a half times the gate, retries included: a slower runner passes, a stall ends.
 */
export const LIMITS = Object.freeze({
  startTimeoutMs: 30_000,
  runTimeoutMs: 180_000,
  stopTimeoutMs: 10_000,
  closeTimeoutMs: 10_000,
  deadlineMs: 20 * 60_000,
});

// A run that Lighthouse could not record (no trace of the navigation, the debugging protocol timed out) measured nothing, and is
// made again, at most twice. A run that was recorded and scored low is never made again, and neither is one that failed for a
// reason about the page (it never painted) or one that hung: those are results.
const FLAKE_CODES = new Set(["NO_NAVSTART", "PROTOCOL_TIMEOUT", "TARGET_CRASHED"]);
const MAX_RETRIES = 2;

/** @param {string[]} argv */
function options(argv) {
  const out = { config: join(WEB_ROOT, "lighthouserc.json"), runs: undefined };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--config") out.config = argv[++i] ?? out.config;
    else if (argv[i] === "--runs") out.runs = Number(argv[++i]);
    else throw new Error(`unknown argument ${argv[i]}`);
  }
  return out;
}

/**
 * @param {{
 *   argv: string[];
 *   startHarness: (options: { dist: string; scenario: string }) => Promise<{ url: string; setScenario: (id: string) => void; close: () => Promise<void> }>;
 *   lighthouse: (url: string, flags: Record<string, unknown>, config: unknown) => Promise<any>;
 *   lighthouseConfig: unknown;
 *   chromium: { binary: () => string; args: () => string[] };
 *   limits?: Partial<typeof LIMITS>;
 *   resultsDir?: string;
 *   profileRoot?: string;
 *   out?: { write: (text: string) => unknown };
 *   err?: { write: (text: string) => unknown };
 * }} parts
 * @returns {Promise<number>} the exit code; rejects when the gate could not be run (the command exits 2)
 */
export async function runLighthouseGate({ argv, startHarness, lighthouse, lighthouseConfig, chromium, limits: given = {}, resultsDir = defaultResultsDir(), profileRoot, out = process.stdout, err = process.stderr }) {
  const limits = { ...LIMITS, ...given };
  const args = options(argv);
  const config = parseConfig(readFileSync(args.config, "utf8"));
  const runs = args.runs ?? config.runs;

  /** One Lighthouse run of `url` in a fresh browser. `label` says which run it is, for the messages. */
  async function runOnce(url, label) {
    const browser = await startChromium({
      binary: chromium.binary(),
      args: chromium.args(),
      startTimeoutMs: limits.startTimeoutMs,
      stopTimeoutMs: limits.stopTimeoutMs,
      report: (text) => err.write(text),
      ...(profileRoot === undefined ? {} : { profileRoot }),
    });
    try {
      const run = await browser.whileAlive(
        within(
          lighthouse(url, { port: browser.port, output: "json", logLevel: "error", onlyCategories: ["performance"] }, lighthouseConfig),
          limits.runTimeoutMs,
          () =>
            `Lighthouse did not finish ${label} in ${limits.runTimeoutMs} ms: it was connected to Chromium on port ${browser.port} and had not answered (the debugging protocol, the navigation and the trace can each stall).${browser.tail() === "" ? " Chromium has printed nothing." : ` Chromium has printed:\n${browser.tail()}`}`,
        ),
      );
      const lhr = run?.lhr;
      if (!lhr) throw new Error("Lighthouse returned no report");
      if (lhr.runtimeError) throw Object.assign(new Error(`${lhr.runtimeError.code}: ${lhr.runtimeError.message}`), { lighthouseCode: lhr.runtimeError.code });
      const metric = (id) => {
        const value = lhr.audits?.[id]?.numericValue;
        return typeof value === "number" && Number.isFinite(value) ? Math.round(value * 10) / 10 : null;
      };
      return {
        score: lhr.categories?.performance?.score,
        metrics: { fcpMs: metric("first-contentful-paint"), lcpMs: metric("largest-contentful-paint"), tbtMs: metric("total-blocking-time"), cls: metric("cumulative-layout-shift"), speedIndexMs: metric("speed-index") },
        lighthouse: lhr.lighthouseVersion,
        userAgent: lhr.environment?.hostUserAgent,
      };
    } finally {
      await browser.stop();
    }
  }

  /** `runOnce`, made again when Lighthouse could not record the run (see FLAKE_CODES). Answers with how many tries it took. */
  async function runMeasured(url, label) {
    const { value, retries } = await retryUnrecorded(() => runOnce(url, label), {
      flakes: FLAKE_CODES,
      maxRetries: MAX_RETRIES,
      onRetry: (code, attempt) => out.write(`lighthouse ${url}: ${code}, nothing was measured; making the run again (${attempt}/${MAX_RETRIES})\n`),
    });
    return { ...value, retries };
  }

  const harness = await startHarness({ dist: config.dist, scenario: config.pages[0]?.scenario ?? config.scenario });
  const measured = [];
  let lighthouseVersion = "unknown";
  let userAgent = "unknown";
  // Where the gate is, for the message of the deadline; and whether it has given up, so that the work it left behind starts nothing.
  const progress = { page: 0, run: 0, path: "" };
  let abandoned = false;

  async function measure() {
    for (const [index, { url: path, scenario }] of config.pages.entries()) {
      // Each page has the data its screen needs (lighthouserc.json, `scenarios`).
      harness.setScenario(scenario);
      const scores = [];
      const detail = [];
      for (let i = 0; i < runs; i++) {
        if (abandoned) throw new Error("the Lighthouse gate had given up");
        Object.assign(progress, { page: index + 1, run: i + 1, path });
        const result = await runMeasured(`${harness.url}${path}`, `run ${i + 1} of ${runs} of ${path}`);
        scores.push(result.score);
        detail.push({ points: toPoints(result.score), ...result.metrics, retries: result.retries });
        lighthouseVersion = result.lighthouse ?? lighthouseVersion;
        userAgent = result.userAgent ?? userAgent;
        out.write(`lighthouse ${path} run ${i + 1}/${runs}: ${toPoints(result.score)} (LCP ${String(result.metrics.lcpMs)} ms, TBT ${String(result.metrics.tbtMs)} ms, CLS ${String(result.metrics.cls)})\n`);
      }
      measured.push({ url: path, scores, detail });
    }
  }

  try {
    await within(measure(), limits.deadlineMs, () => {
      abandoned = true;
      killAllChromium();
      return `the Lighthouse gate did not finish in ${limits.deadlineMs} ms (it was at page ${progress.page} of ${config.pages.length}, run ${progress.run} of ${runs}, ${progress.path}); a run takes seconds, so something stalled`;
    });
  } finally {
    // The score is already measured when this fails, so a harness that will not close is reported and does not decide the gate.
    try {
      await within(harness.close(), limits.closeTimeoutMs, `the harness did not close in ${limits.closeTimeoutMs} ms`);
    } catch (/** @type {unknown} */ error) {
      err.write(`lighthouse: ${error instanceof Error ? error.message : String(error)}\n`);
    }
  }

  const answer = aggregate(measured, { runs, minScore: config.minScore });
  const path = writeResult(
    {
      bench: "web/lighthouse_performance",
      unit: "score",
      values: answer.values,
      samples: runs * config.urls.length,
      pages: answer.pages.map((page) => ({ ...page, detail: measured.find((m) => m.url === page.url)?.detail })),
      note: `Lighthouse ${lighthouseVersion}, desktop preset (simulated throttling), performance category; a page is the median of ${runs} runs in a fresh Chromium each, the budget is the lowest page; the pages are real routes of the production build behind the harness (the hub's headers, no Content-Encoding)`,
      environment: { browser: userAgent, cpus: cpus().length, node: process.version },
    },
    { dir: resultsDir },
  );
  if (path === null) throw new Error(`could not write the result to ${resultsDir}`);
  for (const page of answer.pages) out.write(`lighthouse ${page.url}: median ${page.median} of ${JSON.stringify(page.runs)} ${page.passed ? "ok" : `UNDER ${Math.round(config.minScore * 100)}`}\n`);
  out.write(`lighthouse: p50 ${answer.values.p50} (the lowest page), min ${answer.values.min}; written to ${path}\n`);
  return answer.passed ? 0 : 1;
}
