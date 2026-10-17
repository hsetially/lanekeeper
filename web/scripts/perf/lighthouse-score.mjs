// What a set of Lighthouse runs is worth, as a number the P10 budget can judge. Pure: the runner (lighthouse.mjs) drives the
// browser, and this reads the configuration and turns scores into the result.
//
// The configuration is `web/lighthouserc.json`, in the format of Lighthouse CI's rc file, so that the runner and the CI tool
// it stands in for (W31) are told the same thing: which pages, how many runs of each, and the score the median must reach.
//
// The score a budget sees: each page's score is the MEDIAN of its runs (so one noisy run does not decide a page), and the
// budget is the LOWEST page, so a slow page cannot be outvoted by fast ones. A run that has no performance score (Lighthouse
// failed, the page did not load) is an error here and not a gap: a result is never written from fewer runs than were asked.

import { summarize } from "./results.mjs";

/**
 * @typedef {{ urls: string[]; runs: number; minScore: number; dist: string; scenario: string }} LighthouseConfig
 */

const PATH = /^\/[A-Za-z0-9/_.-]*$/;

/**
 * @param {string} text the contents of lighthouserc.json
 * @returns {LighthouseConfig}
 */
export function parseConfig(text) {
  /** @type {any} */
  const rc = JSON.parse(text);
  const collect = rc?.ci?.collect;
  const assertion = rc?.ci?.assert?.assertions?.["categories:performance"];
  const urls = collect?.url;
  if (!Array.isArray(urls) || urls.length === 0) throw new Error("lighthouserc.json: ci.collect.url is a non-empty list");
  for (const u of urls) if (typeof u !== "string" || !PATH.test(u)) throw new Error(`lighthouserc.json: "${String(u)}" is not a path on the harness (it starts with / and has no query or host)`);
  const runs = collect?.numberOfRuns;
  if (!Number.isInteger(runs) || runs < 1 || runs > 10) throw new Error("lighthouserc.json: ci.collect.numberOfRuns is a whole number from 1 to 10");
  if (collect?.settings?.preset !== "desktop") throw new Error('lighthouserc.json: the preset is "desktop" (P10)');
  if (!Array.isArray(assertion) || assertion[0] !== "error") throw new Error('lighthouserc.json: categories:performance is asserted at level "error"');
  const minScore = assertion[1]?.minScore;
  if (typeof minScore !== "number" || !(minScore > 0 && minScore <= 1)) throw new Error("lighthouserc.json: the performance minScore is a number in (0, 1]");
  if (assertion[1]?.aggregationMethod !== "median") throw new Error('lighthouserc.json: the performance score is aggregated by "median"');
  const dist = rc?.lanekeeper?.dist;
  const scenario = rc?.lanekeeper?.scenario;
  if (typeof dist !== "string" || typeof scenario !== "string") throw new Error("lighthouserc.json: lanekeeper.dist and lanekeeper.scenario say which build and which mock scenario");
  return { urls, runs, minScore, dist, scenario };
}

/**
 * A category score of a Lighthouse run (0 to 1, two decimals) as the whole number a person reads (0 to 100).
 * @param {unknown} score
 * @returns {number}
 */
export function toPoints(score) {
  if (typeof score !== "number" || !Number.isFinite(score) || score < 0 || score > 1) throw new Error(`a Lighthouse performance score is a number from 0 to 1, got ${String(score)}`);
  return Math.round(score * 100);
}

/**
 * @param {Array<{ url: string; scores: Array<number | null | undefined> }>} pages the 0 to 1 scores of each page's runs
 * @param {{ runs: number; minScore: number }} want
 * @returns {{
 *   values: { p50: number; min: number; max: number; mean: number };
 *   pages: Array<{ url: string; runs: number[]; median: number; passed: boolean }>;
 *   passed: boolean;
 * }}
 */
export function aggregate(pages, { runs, minScore }) {
  if (pages.length === 0) throw new Error("no pages were measured");
  const need = Math.round(minScore * 100);
  const out = pages.map((page) => {
    if (page.scores.length !== runs) throw new Error(`${page.url}: ${page.scores.length} runs, ${runs} were asked for`);
    const points = page.scores.map(toPoints);
    const median = summarize(points).p50;
    return { url: page.url, runs: points, median, passed: median >= need };
  });
  const all = out.flatMap((p) => p.runs);
  const overall = summarize(all);
  return {
    values: { p50: Math.min(...out.map((p) => p.median)), min: overall.min, max: overall.max, mean: Math.round(overall.mean * 10) / 10 },
    pages: out,
    passed: out.every((p) => p.passed),
  };
}

/**
 * Make a run again when it measured nothing, and only then. `run` throws an error with a `lighthouseCode` when Lighthouse
 * gave up; a code in `flakes` (it could not record the navigation, the protocol timed out) means no measurement was made, so
 * the run is repeated, at most `maxRetries` times. Any other error, and a run that was recorded and scored low, is final: a
 * score is never repeated until it is better.
 * @template T
 * @param {() => Promise<T>} run
 * @param {{ flakes: ReadonlySet<string>; maxRetries: number; onRetry?: (code: string, attempt: number) => void }} options
 * @returns {Promise<{ value: T; retries: number }>}
 */
export async function retryUnrecorded(run, { flakes, maxRetries, onRetry }) {
  for (let retries = 0; ; retries++) {
    try {
      return { value: await run(), retries };
    } catch (/** @type {unknown} */ error) {
      const code = error instanceof Error && "lighthouseCode" in error ? String(error.lighthouseCode) : undefined;
      if (code === undefined || !flakes.has(code) || retries >= maxRetries) throw error;
      onRetry?.(code, retries + 1);
    }
  }
}
