import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { aggregate, parseConfig, retryUnrecorded, toPoints } from "./lighthouse-score.mjs";

const rc = (patch = {}) =>
  JSON.stringify({
    ci: {
      collect: { numberOfRuns: 3, url: ["/a", "/b/c"], settings: { preset: "desktop", onlyCategories: ["performance"] } },
      assert: { assertions: { "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "median" }] } },
    },
    lanekeeper: { dist: "dist-perf", scenario: "perf" },
    ...patch,
  });

describe("parseConfig", () => {
  it("reads the repository's lighthouserc.json: three real routes of the production build, three runs, a median of 90", () => {
    const config = parseConfig(readFileSync(new URL("../../lighthouserc.json", import.meta.url), "utf8"));
    expect(config.runs).toBe(3);
    expect(config.minScore).toBe(0.9);
    expect(config.urls).toHaveLength(3);
    expect(config.urls[0]).toMatch(/^\/compare\?file=services%2Fpayments%2Fapplication\.yml&lanes=swimlane-01,.*,swimlane-40&only=all$/);
    expect(config.urls[1]).toBe("/swimlanes/sitb/files?path=security%2Fsecurity-roles.yml&tab=diff");
    expect(config.urls[2]).toBe("/swimlanes");
    expect(config).toMatchObject({ dist: "dist", scenario: "perf-routes" });
    expect(config.pages.map((p) => p.scenario)).toEqual(["design-compare-large", "design-file-diff-large", "perf-routes"]);
    // No page is a test page of the perf build.
    expect(config.urls.some((u) => u.startsWith("/testpages"))).toBe(false);
  });

  it("matches the budget it feeds: the same threshold as perf/budgets.toml", () => {
    const budgets = readFileSync(new URL("../../../perf/budgets.toml", import.meta.url), "utf8");
    const block = /id = "P10\.lighthouse_perf"[\s\S]*?(?=\n\[\[budget\]\]|$)/.exec(budgets)?.[0] ?? "";
    expect(Number(/^threshold = (\d+)/m.exec(block)?.[1])).toBe(parseConfig(readFileSync(new URL("../../lighthouserc.json", import.meta.url), "utf8")).minScore * 100);
  });

  it.each([
    ["no pages", { ci: { collect: { numberOfRuns: 3, url: [], settings: { preset: "desktop" } }, assert: { assertions: { "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "median" }] } } } }],
    ["a full URL, not a path", { ci: { collect: { numberOfRuns: 3, url: ["http://example.com/"], settings: { preset: "desktop" } }, assert: { assertions: { "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "median" }] } } } }],
    ["a path with a fragment", { ci: { collect: { numberOfRuns: 3, url: ["/a#x"], settings: { preset: "desktop" } }, assert: { assertions: { "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "median" }] } } } }],
    ["a scenario for a page that is not one of the pages", { lanekeeper: { dist: "dist", scenario: "s", scenarios: { "/other": "x" } } }],
    ["a mobile preset", { ci: { collect: { numberOfRuns: 3, url: ["/a"], settings: { preset: "mobile" } }, assert: { assertions: { "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "median" }] } } } }],
    ["no runs", { ci: { collect: { numberOfRuns: 0, url: ["/a"], settings: { preset: "desktop" } }, assert: { assertions: { "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "median" }] } } } }],
    ["a warning where an error is wanted", { ci: { collect: { numberOfRuns: 3, url: ["/a"], settings: { preset: "desktop" } }, assert: { assertions: { "categories:performance": ["warn", { minScore: 0.9, aggregationMethod: "median" }] } } } }],
    ["a best-run aggregation", { ci: { collect: { numberOfRuns: 3, url: ["/a"], settings: { preset: "desktop" } }, assert: { assertions: { "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "optimistic" }] } } } }],
    ["a threshold above 1", { ci: { collect: { numberOfRuns: 3, url: ["/a"], settings: { preset: "desktop" } }, assert: { assertions: { "categories:performance": ["error", { minScore: 90, aggregationMethod: "median" }] } } } }],
  ])("refuses %s", (_name, patch) => {
    expect(() => parseConfig(rc(patch))).toThrow(/lighthouserc\.json/);
  });

  it("accepts a valid file", () => {
    expect(parseConfig(rc())).toEqual({ urls: ["/a", "/b/c"], runs: 3, minScore: 0.9, dist: "dist-perf", scenario: "perf", pages: [{ url: "/a", scenario: "perf" }, { url: "/b/c", scenario: "perf" }] });
  });

  it("accepts a page with a query, and gives a page its own scenario where the file says so", () => {
    const text = JSON.stringify({
      ci: { collect: { numberOfRuns: 3, url: ["/compare?file=a%2Fb.yml&lanes=x,y&only=all", "/b"], settings: { preset: "desktop" } }, assert: { assertions: { "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "median" }] } } },
      lanekeeper: { dist: "dist", scenario: "default", scenarios: { "/compare?file=a%2Fb.yml&lanes=x,y&only=all": "special" } },
    });
    expect(parseConfig(text).pages).toEqual([
      { url: "/compare?file=a%2Fb.yml&lanes=x,y&only=all", scenario: "special" },
      { url: "/b", scenario: "default" },
    ]);
  });
});

describe("toPoints", () => {
  it("rounds a 0 to 1 score to whole points", () => {
    expect(toPoints(0.9)).toBe(90);
    expect(toPoints(0.895)).toBe(90);
    expect(toPoints(1)).toBe(100);
    expect(toPoints(0)).toBe(0);
  });

  it.each([null, undefined, Number.NaN, -0.1, 1.1, "0.9"])("refuses %s", (score) => {
    expect(() => toPoints(score)).toThrow(/performance score/);
  });
});

describe("aggregate", () => {
  const want = { runs: 3, minScore: 0.9 };

  it("takes the median of each page, then the lowest page", () => {
    const result = aggregate(
      [
        { url: "/a", scores: [1, 0.99, 0.97] },
        { url: "/b", scores: [0.91, 0.95, 0.5] },
      ],
      want,
    );
    expect(result.pages.map((p) => p.median)).toEqual([99, 91]);
    expect(result.values.p50).toBe(91);
    expect(result.values.min).toBe(50);
    expect(result.values.max).toBe(100);
    expect(result.passed).toBe(true);
  });

  it("fails when any page's median is under the score, however good the others are", () => {
    const result = aggregate(
      [
        { url: "/fast", scores: [1, 1, 1] },
        { url: "/slow", scores: [0.89, 0.89, 0.95] },
      ],
      want,
    );
    expect(result.values.p50).toBe(89);
    expect(result.pages.map((p) => p.passed)).toEqual([true, false]);
    expect(result.passed).toBe(false);
  });

  it("is not moved by one bad run among three, nor by one lucky one", () => {
    expect(aggregate([{ url: "/a", scores: [0.95, 0.95, 0.4] }], want).values.p50).toBe(95);
    expect(aggregate([{ url: "/a", scores: [0.8, 0.8, 1] }], want).values.p50).toBe(80);
  });

  it("refuses to answer from fewer runs than were asked for", () => {
    expect(() => aggregate([{ url: "/a", scores: [0.95, 0.95] }], want)).toThrow(/2 runs, 3 were asked for/);
  });

  it("refuses a run with no score instead of leaving it out", () => {
    expect(() => aggregate([{ url: "/a", scores: [0.95, null, 0.95] }], want)).toThrow(/performance score/);
  });

  it("refuses no pages", () => {
    expect(() => aggregate([], want)).toThrow(/no pages/);
  });

  it("writes only numbers a result file can hold", () => {
    const { values } = aggregate([{ url: "/a", scores: [0.93, 0.94, 0.95] }], want);
    for (const v of Object.values(values)) expect(Number.isFinite(v)).toBe(true);
    expect(values).toEqual({ p50: 94, min: 93, max: 95, mean: 94 });
  });
});

describe("retryUnrecorded", () => {
  const flakes = new Set(["NO_NAVSTART", "PROTOCOL_TIMEOUT"]);
  const failing = (code) => Object.assign(new Error(code), { lighthouseCode: code });

  it("returns a run that worked with no retries", async () => {
    expect(await retryUnrecorded(() => Promise.resolve(7), { flakes, maxRetries: 2 })).toEqual({ value: 7, retries: 0 });
  });

  it("makes a run again when nothing was recorded, and says how many tries it took", async () => {
    let calls = 0;
    const seen = [];
    const out = await retryUnrecorded(
      () => {
        calls += 1;
        return calls < 3 ? Promise.reject(failing("NO_NAVSTART")) : Promise.resolve("score");
      },
      { flakes, maxRetries: 2, onRetry: (code, attempt) => seen.push([code, attempt]) },
    );
    expect(out).toEqual({ value: "score", retries: 2 });
    expect(seen).toEqual([["NO_NAVSTART", 1], ["NO_NAVSTART", 2]]);
  });

  it("gives up after the bounded number of retries and throws the last error", async () => {
    let calls = 0;
    await expect(
      retryUnrecorded(
        () => {
          calls += 1;
          return Promise.reject(failing("PROTOCOL_TIMEOUT"));
        },
        { flakes, maxRetries: 2 },
      ),
    ).rejects.toThrow("PROTOCOL_TIMEOUT");
    expect(calls).toBe(3);
  });

  it("never repeats an error about the page (it did not paint) or an error with no Lighthouse code", async () => {
    let calls = 0;
    await expect(
      retryUnrecorded(
        () => {
          calls += 1;
          return Promise.reject(failing("NO_FCP"));
        },
        { flakes, maxRetries: 2 },
      ),
    ).rejects.toThrow("NO_FCP");
    await expect(retryUnrecorded(() => Promise.reject(new Error("boom")), { flakes, maxRetries: 2 })).rejects.toThrow("boom");
    expect(calls).toBe(1);
  });

  it("never repeats a run that was recorded and scored low: a score is a value, not an error", async () => {
    let calls = 0;
    const out = await retryUnrecorded(
      () => {
        calls += 1;
        return Promise.resolve(0.3);
      },
      { flakes, maxRetries: 2 },
    );
    expect(out).toEqual({ value: 0.3, retries: 0 });
    expect(calls).toBe(1);
  });
});
