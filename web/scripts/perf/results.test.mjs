import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { clearResults, percentile, resultFileName, resultsDir, summarize, writeResult } from "./results.mjs";

// The P10 numbers reach `cargo xtask bench-check` as JSON files in target/perf-results/ (crates/xtask/src/bench_check.rs:
// `{"bench": "...", "unit": "...", "value": 1.0}` or `{"bench": "...", "unit": "...", "values": {"p95": 1.0}}`). `values` is
// read as a map of numbers: ONE non-number in it and the whole file is "no result". These tests hold the writer to that, and
// to the percentile rule the checker uses, so a number measured here is the number judged there.

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "lk-results-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

describe("percentile (the checker's nearest-rank rule)", () => {
  const hundred = Array.from({ length: 100 }, (_, i) => i + 1);
  it("takes the smallest value with at least p of the samples at or below it", () => {
    expect(percentile(hundred, 0.95)).toBe(95);
    expect(percentile(hundred, 0.5)).toBe(50);
    expect(percentile(hundred, 0.99)).toBe(99);
    expect(percentile(hundred, 1)).toBe(100);
    expect(percentile(hundred, 0)).toBe(1);
  });
  it("does not round 95 of 100 up to 96, the checker's epsilon", () => {
    expect(percentile(hundred, 0.95)).not.toBe(96);
  });
  it("gives the checker's answers for the small counts the runs use", () => {
    const twenty = Array.from({ length: 20 }, (_, i) => i + 1);
    expect(percentile(twenty, 0.95)).toBe(19);
    expect(percentile([10, 20, 30], 0.5)).toBe(20);
    expect(percentile([7], 0.95)).toBe(7);
  });
  it("is NaN for no samples, which no budget accepts", () => {
    expect(percentile([], 0.5)).toBeNaN();
  });
});

describe("summarize", () => {
  it("sorts, and reports the statistics a budget can name", () => {
    const s = summarize([30, 10, 20, 50, 40]);
    expect(s).toEqual({ n: 5, min: 10, p50: 30, p95: 50, p99: 50, max: 50, mean: 30 });
  });
  it("does not change its input", () => {
    const input = [3, 1, 2];
    summarize(input);
    expect(input).toEqual([3, 1, 2]);
  });
  it("refuses samples that are not finite numbers, so a broken run cannot pass", () => {
    expect(() => summarize([1, Number.NaN])).toThrow(/finite/);
    expect(() => summarize([1, Number.POSITIVE_INFINITY])).toThrow(/finite/);
    expect(() => summarize([])).toThrow(/no samples/);
  });
});

describe("resultsDir", () => {
  it("is target/perf-results beside the repository when CARGO_TARGET_DIR is not set", () => {
    expect(resultsDir({}, "/repo/web")).toBe("/repo/target/perf-results");
  });
  it("follows CARGO_TARGET_DIR, absolute or relative to the repository", () => {
    expect(resultsDir({ CARGO_TARGET_DIR: "/somewhere/target" }, "/repo/web")).toBe("/somewhere/target/perf-results");
    expect(resultsDir({ CARGO_TARGET_DIR: "build/t" }, "/repo/web")).toBe("/repo/build/t/perf-results");
  });
});

describe("writeResult", () => {
  const sample = { bench: "web/route_change_cached", unit: "ms", values: { p50: 12, p95: 31.5, p99: 40, max: 44, min: 8, mean: 15 }, samples: 30 };

  it("writes exactly one file for a bench, named web-<name>.json, and replaces it on the next run", () => {
    const first = writeResult(sample, { dir });
    expect(first).toBe(join(dir, "web-route_change_cached.json"));
    writeResult({ ...sample, values: { ...sample.values, p95: 50 } }, { dir });
    expect(readdirSync(dir)).toEqual(["web-route_change_cached.json"]);
    expect(JSON.parse(readFileSync(first, "utf8")).values.p95).toBe(50);
  });

  it("keeps `values` to numbers, and everything else beside it, so the checker can read the file", () => {
    const path = writeResult({ ...sample, extra: { browser: "Chromium 141", note: "x" } }, { dir });
    const parsed = JSON.parse(readFileSync(path, "utf8"));
    expect(parsed.bench).toBe("web/route_change_cached");
    expect(parsed.unit).toBe("ms");
    for (const v of Object.values(parsed.values)) expect(typeof v).toBe("number");
    expect(parsed.extra.browser).toBe("Chromium 141");
    expect(typeof parsed.measuredAt).toBe("string");
  });

  it("writes a single value with the statistic it is", () => {
    const path = writeResult({ bench: "web/initial_js_gzip_kb", unit: "kb", statistic: "max", value: 129.04 }, { dir });
    expect(JSON.parse(readFileSync(path, "utf8"))).toMatchObject({ bench: "web/initial_js_gzip_kb", unit: "kb", statistic: "max", value: 129.04 });
  });

  it("refuses a result with no number, a number that is not finite, or `values` that holds anything else", () => {
    expect(() => writeResult({ bench: "web/x", unit: "ms" }, { dir })).toThrow(/value/);
    expect(() => writeResult({ bench: "web/x", unit: "ms", value: Number.NaN }, { dir })).toThrow(/finite/);
    expect(() => writeResult({ bench: "web/x", unit: "ms", values: { p95: "12" } }, { dir })).toThrow(/finite/);
    expect(() => writeResult({ bench: "web/x", unit: "ms", values: { p95: 1 }, value: 2 }, { dir })).toThrow(/one of/);
    expect(readdirSync(dir)).toEqual([]);
  });

  it("refuses a bench that is not a web budget, or a name that could leave the directory", () => {
    expect(() => writeResult({ bench: "hub/thing", unit: "ms", value: 1 }, { dir })).toThrow(/web\//);
    expect(() => writeResult({ bench: "web/../../etc", unit: "ms", value: 1 }, { dir })).toThrow(/name/);
    expect(() => writeResult({ bench: "web/a/b", unit: "ms", value: 1 }, { dir })).toThrow(/name/);
  });

  it("does not fail the build when the directory cannot be made (the image build has a read-only target)", () => {
    const blocker = join(dir, "file");
    writeFileSync(blocker, "x");
    expect(writeResult({ ...sample }, { dir: join(blocker, "inside") })).toBeNull();
  });

  it("makes the directory when it does not exist", () => {
    const nested = join(dir, "a", "b");
    expect(writeResult(sample, { dir: nested })).toBe(join(nested, "web-route_change_cached.json"));
    expect(existsSync(nested)).toBe(true);
  });
});

describe("resultFileName and clearResults", () => {
  it("names the file after the bench", () => {
    expect(resultFileName("web/grid_5000_rows_scroll_fps")).toBe("web-grid_5000_rows_scroll_fps.json");
  });

  it("removes this app's results and no one else's", () => {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "web-a.json"), "{}");
    writeFileSync(join(dir, "web-b.json"), "{}");
    writeFileSync(join(dir, "hub-writes-x.json"), "{}");
    writeFileSync(join(dir, "notes.txt"), "x");
    expect(clearResults({ dir }).sort()).toEqual(["web-a.json", "web-b.json"]);
    expect(readdirSync(dir).sort()).toEqual(["hub-writes-x.json", "notes.txt"]);
  });

  it("is quiet when the directory is not there", () => {
    expect(clearResults({ dir: join(dir, "missing") })).toEqual([]);
  });
});
