// The P10 numbers, written where `cargo xtask bench-check` reads them (crates/xtask/src/bench_check.rs).
//
// One JSON file per bench, in `${CARGO_TARGET_DIR:-<repo>/target}/perf-results/`, named `web-<name>.json`:
//   {"bench": "web/route_change_cached", "unit": "ms", "values": {"p50": .., "p95": .., "min": .., "max": ..}}
//   {"bench": "web/initial_js_gzip_kb", "unit": "kb", "statistic": "max", "value": 129.0}
//   {"bench": "web/grid_5000_rows_scroll_fps", "unit": "fps", "values": {"min": 190.0, ..}}
// The last one is a CAPACITY, not the display's frame rate: `values` are frames a second the grid can make with the browser's
// frame-rate limit off and the same scroll in every frame (minimum of five runs, not capped at 60), judged against a floor of
// 60. The frame-rate-limited number of W20's first definition is in the same file as `vsyncLocked`, for information, and the
// late-frame comparison with a control list is an assertion of e2e/perf/grid-5000.spec.ts (W20, decided 2026-10-17).
// The checker reads `values` as a map of numbers, so ONE value in it that is not a number makes the whole file "no result":
// everything else (the browser, the sample count, notes) goes beside it, at the top level, where the checker ignores it.
// A number that is not finite is refused here, never written, so a run that broke cannot pass by writing a NaN.
//
// Best effort: a read-only target directory (the image build) must not fail a build, so a write that cannot happen returns
// null. The gates that need the result (`web-bench-check`) fail on a missing file by themselves, because the budgets are
// registered.

import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";

const WEB_ROOT = new URL("../../", import.meta.url).pathname;

/**
 * `${CARGO_TARGET_DIR:-<repo>/target}/perf-results`. A relative CARGO_TARGET_DIR is relative to the repository, as cargo's is.
 * @param {Record<string, string | undefined>} [env]
 * @param {string} [webRoot]
 * @returns {string}
 */
export function resultsDir(env = process.env, webRoot = WEB_ROOT) {
  const repo = resolve(webRoot, "..");
  const target = env["CARGO_TARGET_DIR"];
  const base = target === undefined || target === "" ? join(repo, "target") : isAbsolute(target) ? target : resolve(repo, target);
  return join(base, "perf-results");
}

const BENCH = /^web\/([A-Za-z0-9_]+)$/;

/**
 * @param {string} bench
 * @returns {string}
 */
export function resultFileName(bench) {
  const match = BENCH.exec(bench);
  if (!match) throw new Error(`bench name "${bench}" is not web/<name> with letters, digits and underscores`);
  return `web-${match[1]}.json`;
}

/**
 * Nearest rank, as the checker computes it: the smallest value with at least `p` of the samples at or below it.
 * @param {number[]} sorted ascending
 * @param {number} p
 * @returns {number}
 */
export function percentile(sorted, p) {
  if (sorted.length === 0) return Number.NaN;
  const rank = Math.max(1, Math.ceil(p * sorted.length - 1e-9));
  return sorted[Math.min(rank, sorted.length) - 1];
}

/**
 * The statistics a budget can name, over finite samples.
 * @param {number[]} samples
 * @returns {{ n: number; min: number; p50: number; p95: number; p99: number; max: number; mean: number }}
 */
export function summarize(samples) {
  if (samples.length === 0) throw new Error("no samples");
  for (const s of samples) if (!Number.isFinite(s)) throw new Error(`a sample is not a finite number: ${String(s)}`);
  const sorted = [...samples].sort((a, b) => a - b);
  const sum = sorted.reduce((n, v) => n + v, 0);
  return { n: sorted.length, min: sorted[0], p50: percentile(sorted, 0.5), p95: percentile(sorted, 0.95), p99: percentile(sorted, 0.99), max: sorted[sorted.length - 1], mean: sum / sorted.length };
}

/**
 * @param {{ bench: string; unit: string; statistic?: string; value?: number; values?: Record<string, number> } & Record<string, unknown>} result
 * @param {{ dir?: string }} [options]
 * @returns {string | null} the path written, or null when it could not be
 */
export function writeResult(result, { dir = resultsDir() } = {}) {
  const { bench, unit, value, values } = result;
  const name = resultFileName(bench);
  if (typeof unit !== "string" || unit === "") throw new Error("a result names its unit");
  if ((value === undefined) === (values === undefined)) throw new Error("a result has one of `value` and `values`");
  if (value !== undefined && !Number.isFinite(value)) throw new Error(`value is not finite: ${String(value)}`);
  if (values !== undefined) {
    if (Object.keys(values).length === 0) throw new Error("`values` is empty");
    for (const [key, v] of Object.entries(values)) if (typeof v !== "number" || !Number.isFinite(v)) throw new Error(`values.${key} is not finite: ${String(v)}`);
  }
  try {
    mkdirSync(dir, { recursive: true });
    const path = join(dir, name);
    writeFileSync(path, `${JSON.stringify({ ...result, measuredAt: new Date().toISOString() }, null, 2)}\n`);
    return path;
  } catch {
    return null;
  }
}

/**
 * Remove this app's results (`web-*.json`), so that only a run's own numbers can pass. Returns the names removed.
 * @param {{ dir?: string }} [options]
 * @returns {string[]}
 */
export function clearResults({ dir = resultsDir() } = {}) {
  let names;
  try {
    names = readdirSync(dir).filter((n) => /^web-.+\.json$/.test(n));
  } catch {
    return [];
  }
  for (const n of names) rmSync(join(dir, n), { force: true });
  return names;
}
