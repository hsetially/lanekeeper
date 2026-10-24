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
//   node scripts/perf/lighthouse.mjs [--config lighthouserc.json] [--runs N]
//
// Exit codes: 0 every page's median reaches the score; 1 a page's median is under it; 2 a run could not be made (no browser,
// Lighthouse failed, no score): nothing is written then, so a broken run cannot pass.
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { cpus, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import lighthouse, { desktopConfig } from "lighthouse";
import { chromiumPath, launchArgs } from "../../e2e/harness/browser.ts";
import { startHarness } from "../../e2e/harness/serve.ts";
import { aggregate, parseConfig, retryUnrecorded, toPoints } from "./lighthouse-score.mjs";
import { removeDir } from "./remove-dir.mjs";
import { resultsDir, writeResult } from "./results.mjs";

const WEB_ROOT = fileURLToPath(new URL("../../", import.meta.url));
const LISTENING = /DevTools listening on ws:\/\/[^:]+:(\d+)\//;
const START_TIMEOUT_MS = 30_000;
const RUN_TIMEOUT_MS = 120_000;
// A run that Lighthouse could not record (no trace of the navigation, the debugging protocol timed out) measured nothing, and is
// made again, at most twice. A run that was recorded and scored low is never made again, and neither is one that failed for a
// reason about the page (it never painted): those are results.
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

/** A headless Chromium with a debugging port on a free port and a profile of its own. Resolves with the port and a stop(). */
async function startChromium() {
  const profile = mkdtempSync(join(tmpdir(), "lk-lighthouse-"));
  const child = spawn(chromiumPath(), ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-background-networking", ...launchArgs(), "about:blank"], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  const stop = async () => {
    child.kill("SIGKILL");
    await new Promise((resolve) => child.once("exit", resolve));
    // SIGKILL ends the browser and not its helper processes, which keep writing into the profile for a moment: a single remove
    // died with ENOTEMPTY once, after the score was measured, and nothing was written. removeDir waits for them (remove-dir.mjs).
    // The profile is only a temporary folder, so one that still cannot be removed is reported and never costs the measurement.
    try {
      await removeDir(profile);
    } catch (/** @type {unknown} */ error) {
      process.stderr.write(`lighthouse: could not remove ${profile}: ${error instanceof Error ? error.message : String(error)}\n`);
    }
  };
  const port = await new Promise((resolve, reject) => {
    let seen = "";
    const timer = setTimeout(() => reject(new Error(`Chromium did not report a debugging port in ${START_TIMEOUT_MS} ms`)), START_TIMEOUT_MS);
    child.stderr.on("data", (chunk) => {
      seen += String(chunk);
      const match = LISTENING.exec(seen);
      if (match) {
        clearTimeout(timer);
        resolve(Number(match[1]));
      }
    });
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`Chromium exited with ${String(code)} before it was ready: ${seen.slice(-300)}`));
    });
  }).catch(async (/** @type {unknown} */ error) => {
    await stop();
    throw error;
  });
  return { port: /** @type {number} */ (port), stop };
}

/** One Lighthouse run of `url` in a fresh browser. */
async function runOnce(url) {
  const browser = await startChromium();
  try {
    const run = await Promise.race([
      lighthouse(url, { port: browser.port, output: "json", logLevel: "error", onlyCategories: ["performance"] }, desktopConfig),
      new Promise((_, reject) => setTimeout(() => reject(new Error(`no answer in ${RUN_TIMEOUT_MS} ms`)), RUN_TIMEOUT_MS)),
    ]);
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
async function runMeasured(url) {
  const { value, retries } = await retryUnrecorded(() => runOnce(url), {
    flakes: FLAKE_CODES,
    maxRetries: MAX_RETRIES,
    onRetry: (code, attempt) => process.stdout.write(`lighthouse ${url}: ${code}, nothing was measured; making the run again (${attempt}/${MAX_RETRIES})\n`),
  });
  return { ...value, retries };
}

async function main() {
  const args = options(process.argv.slice(2));
  const config = parseConfig(readFileSync(args.config, "utf8"));
  const runs = args.runs ?? config.runs;
  const harness = await startHarness({ dist: config.dist, scenario: config.pages[0]?.scenario ?? config.scenario });
  const measured = [];
  let lighthouseVersion = "unknown";
  let userAgent = "unknown";
  try {
    for (const { url: path, scenario } of config.pages) {
      // Each page has the data its screen needs (lighthouserc.json, `scenarios`).
      harness.setScenario(scenario);
      const scores = [];
      const detail = [];
      for (let i = 0; i < runs; i++) {
        const result = await runMeasured(`${harness.url}${path}`);
        scores.push(result.score);
        detail.push({ points: toPoints(result.score), ...result.metrics, retries: result.retries });
        lighthouseVersion = result.lighthouse ?? lighthouseVersion;
        userAgent = result.userAgent ?? userAgent;
        process.stdout.write(`lighthouse ${path} run ${i + 1}/${runs}: ${toPoints(result.score)} (LCP ${String(result.metrics.lcpMs)} ms, TBT ${String(result.metrics.tbtMs)} ms, CLS ${String(result.metrics.cls)})\n`);
      }
      measured.push({ url: path, scores, detail });
    }
  } finally {
    await harness.close();
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
    { dir: resultsDir() },
  );
  if (path === null) throw new Error(`could not write the result to ${resultsDir()}`);
  for (const page of answer.pages) process.stdout.write(`lighthouse ${page.url}: median ${page.median} of ${JSON.stringify(page.runs)} ${page.passed ? "ok" : `UNDER ${Math.round(config.minScore * 100)}`}\n`);
  process.stdout.write(`lighthouse: p50 ${answer.values.p50} (the lowest page), min ${answer.values.min}; written to ${path}\n`);
  return answer.passed ? 0 : 1;
}

main().then(
  (code) => process.exit(code),
  (/** @type {unknown} */ error) => {
    process.stderr.write(`lighthouse: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(2);
  },
);
