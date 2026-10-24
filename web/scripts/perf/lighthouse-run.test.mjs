import { chmodSync, existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { LIMITS, runLighthouseGate } from "./lighthouse-run.mjs";

// The whole gate with stand-ins for the three things it cannot have here: the harness (a fake with a URL), Lighthouse (a
// function) and the browser (a `#!/bin/sh` file each, see chromium-process.test.mjs). The first run of the perf job on GitHub
// printed nothing for 38 minutes because a browser that had exited was waited for. These tests make every failure of the
// Lighthouse step end, soon, with a message that says what was waited for and what the browser printed, and make the step
// leave no browser, no helper process and no profile behind.

let dir;
let resultsDir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "lk-lighthouse-run-"));
  resultsDir = join(dir, "perf-results");
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const LISTENING = "DevTools listening on ws://127.0.0.1:43211/devtools/browser/0b1c2d3e-0000-4000-8000-000000000000";

/** @param {string} name @param {string} body */
function standIn(name, body) {
  const path = join(dir, name);
  writeFileSync(path, `#!/bin/sh\n${body}\n`);
  chmodSync(path, 0o755);
  return path;
}

const browsers = {
  ok: () => standIn("ok", `echo '${LISTENING}' >&2\nsleep 600 &\nwait`),
  exits: () => standIn("exits", `echo '[1:1:FATAL:zygote_host_impl_linux.cc(132)] No usable sandbox! If you are running on Ubuntu 23.10+' >&2\nexit 133`),
  silent: () => standIn("silent", `echo 'booting' >&2\nsleep 600 &\nwait`),
  dies: () => standIn("dies", `echo '${LISTENING}' >&2\nsleep 0.4\necho 'Check failed: renderer crashed' >&2\nexit 7`),
};

const report = (score) => ({
  lhr: {
    lighthouseVersion: "13.5.0",
    environment: { hostUserAgent: "FakeChrome/1" },
    categories: { performance: { score } },
    audits: Object.fromEntries(["first-contentful-paint", "largest-contentful-paint", "total-blocking-time", "cumulative-layout-shift", "speed-index"].map((id) => [id, { numericValue: 100 }])),
  },
});

function harness() {
  const scenarios = [];
  const state = { closed: 0 };
  return {
    url: "http://127.0.0.1:1",
    setScenario: (id) => scenarios.push(id),
    close: () => {
      state.closed++;
      return Promise.resolve();
    },
    scenarios,
    state,
  };
}

/** One run of the gate, with the stand-ins; `--runs 1` over the three pages of lighthouserc.json. */
function gate({ browser, lighthouse = () => Promise.resolve(report(0.95)), limits = {}, fake = harness(), argv = ["--runs", "1"] }) {
  const out = [];
  const err = [];
  const promise = runLighthouseGate({
    argv,
    startHarness: () => Promise.resolve(fake),
    lighthouse,
    lighthouseConfig: { extends: "lighthouse:default" },
    chromium: { binary: () => browser, args: () => ["--disable-dev-shm-usage", "--no-sandbox"] },
    limits: { startTimeoutMs: 3000, runTimeoutMs: 3000, stopTimeoutMs: 2000, closeTimeoutMs: 2000, deadlineMs: 20_000, ...limits },
    resultsDir,
    out: { write: (s) => out.push(s) },
    err: { write: (s) => err.push(s) },
    profileRoot: dir,
  });
  return { promise, out, err, fake };
}

const outcome = (promise) =>
  promise.then(
    (code) => ({ code }),
    (/** @type {unknown} */ error) => ({ error }),
  );

const profiles = () => readdirSync(dir).filter((name) => name.startsWith("lk-lighthouse-"));

describe("a gate that works", () => {
  it("measures every page in a browser of its own, writes the result for bench-check, and leaves nothing behind", async () => {
    const calls = [];
    const run = gate({
      browser: browsers.ok(),
      lighthouse: (url, flags, config) => {
        calls.push({ url, flags, config });
        return Promise.resolve(report(0.95));
      },
    });
    const result = await outcome(run.promise);
    expect(result).toEqual({ code: 0 });
    expect(calls).toHaveLength(3);
    for (const call of calls) {
      expect(call.url).toMatch(/^http:\/\/127\.0\.0\.1:1\//);
      expect(call.flags).toMatchObject({ port: 43211, output: "json", onlyCategories: ["performance"] });
      expect(call.config).toEqual({ extends: "lighthouse:default" });
    }
    expect(run.fake.scenarios).toHaveLength(3);
    expect(run.fake.state.closed).toBe(1);
    expect(profiles()).toEqual([]);
    const written = JSON.parse(readFileSync(join(resultsDir, "web-lighthouse_performance.json"), "utf8"));
    expect(written).toMatchObject({ bench: "web/lighthouse_performance", unit: "score", values: { p50: 95, min: 95 }, samples: 3 });
    expect(written.environment.browser).toBe("FakeChrome/1");
  });

  it("returns 1, and still writes the number, when a page's median is under the minimum score", async () => {
    const run = gate({ browser: browsers.ok(), lighthouse: () => Promise.resolve(report(0.5)) });
    expect(await outcome(run.promise)).toEqual({ code: 1 });
    expect(JSON.parse(readFileSync(join(resultsDir, "web-lighthouse_performance.json"), "utf8")).values.p50).toBe(50);
  });
});

describe("a gate whose browser does not start (it used to hang, with nothing in the log)", () => {
  it("fails soon when the browser exits at once, with its exit code and what it printed, and writes nothing", async () => {
    const started = Date.now();
    const run = gate({ browser: browsers.exits() });
    const result = await outcome(run.promise);
    expect(Date.now() - started).toBeLessThan(4000);
    expect(result.error?.message).toMatch(/exited with code 133 before it opened a debugging port/);
    expect(result.error?.message).toContain("No usable sandbox!");
    expect(run.fake.state.closed).toBe(1);
    expect(existsSync(resultsDir)).toBe(false);
    expect(profiles()).toEqual([]);
  });

  it("fails within startTimeoutMs when the browser runs and never opens a port, and says what it waited for", async () => {
    const started = Date.now();
    const run = gate({ browser: browsers.silent(), limits: { startTimeoutMs: 300 } });
    const result = await outcome(run.promise);
    expect(Date.now() - started).toBeLessThan(4000);
    expect(result.error?.message).toMatch(/did not open a debugging port in 300 ms/);
    expect(result.error?.message).toContain("booting");
    expect(run.fake.state.closed).toBe(1);
    expect(profiles()).toEqual([]);
  });
});

describe("a gate with no browser to start", () => {
  it("fails with the reason when the browser cannot be found (nothing is started, the harness is closed, nothing is written)", async () => {
    const fake = harness();
    const failing = runLighthouseGate({
      argv: ["--runs", "1"],
      startHarness: () => Promise.resolve(fake),
      lighthouse: () => Promise.resolve(report(0.95)),
      lighthouseConfig: {},
      chromium: {
        binary: () => {
          throw new Error("no Chromium found: set LK_CHROMIUM, or run `pnpm exec playwright install chromium`");
        },
        args: () => [],
      },
      resultsDir,
      profileRoot: dir,
    });
    expect((await outcome(failing)).error?.message).toMatch(/no Chromium found: set LK_CHROMIUM/);
    expect(fake.state.closed).toBe(1);
    expect(existsSync(resultsDir)).toBe(false);
    expect(profiles()).toEqual([]);
  });
});

describe("a gate whose Lighthouse run does not end", () => {
  it("fails at runTimeoutMs, names the page and the run, prints what the browser printed, and is not made again (a hang is not a flake)", async () => {
    let calls = 0;
    const started = Date.now();
    const run = gate({
      browser: browsers.ok(),
      lighthouse: () => {
        calls++;
        return new Promise(() => undefined);
      },
      limits: { runTimeoutMs: 400 },
    });
    const result = await outcome(run.promise);
    expect(Date.now() - started).toBeLessThan(4000);
    expect(calls).toBe(1);
    expect(result.error?.message).toMatch(/Lighthouse did not finish run 1 of 1 of \/compare\?file=.* in 400 ms/);
    expect(result.error?.message).toContain("port 43211");
    expect(result.error?.message).toContain(LISTENING.slice(0, 20));
    expect(run.fake.state.closed).toBe(1);
    expect(profiles()).toEqual([]);
  });

  it("fails at once, with the browser's last words, when the browser dies in the middle of the run (it does not wait for runTimeoutMs)", async () => {
    const started = Date.now();
    const run = gate({ browser: browsers.dies(), lighthouse: () => new Promise(() => undefined), limits: { runTimeoutMs: 15_000 } });
    const result = await outcome(run.promise);
    expect(Date.now() - started).toBeLessThan(5000);
    expect(result.error?.message).toMatch(/exited with code 7 while it was in use/);
    expect(result.error?.message).toContain("renderer crashed");
    expect(profiles()).toEqual([]);
  });

  it("still makes a run again when Lighthouse says that it recorded nothing (NO_NAVSTART), as it always did, and only twice", async () => {
    let calls = 0;
    const run = gate({
      browser: browsers.ok(),
      lighthouse: () => {
        calls++;
        return Promise.resolve({ lhr: { runtimeError: { code: "NO_NAVSTART", message: "no navigation" } } });
      },
    });
    const result = await outcome(run.promise);
    expect(calls).toBe(3);
    expect(result.error?.message).toMatch(/NO_NAVSTART/);
    expect(run.out.join("")).toMatch(/NO_NAVSTART, nothing was measured; making the run again \(2\/2\)/);
  });
});

describe("a gate that takes too long as a whole", () => {
  it("gives up at deadlineMs with the page and run it was at, ends its browser, and does not start another once the work it left behind comes back", async () => {
    // The first run answers at once; the second one is held until the deadline has passed, so which run the gate was at does not
    // depend on how fast the machine is.
    let calls = 0;
    /** @type {() => void} */
    let release = () => undefined;
    const held = new Promise((resolve) => {
      release = () => resolve(undefined);
    });
    const started = Date.now();
    const run = gate({
      browser: browsers.ok(),
      lighthouse: async () => {
        calls++;
        if (calls === 2) await held;
        return report(0.95);
      },
      limits: { deadlineMs: 1500 },
    });
    const result = await outcome(run.promise);
    expect(Date.now() - started).toBeLessThan(6000);
    expect(result.error?.message).toMatch(/did not finish in 1500 ms/);
    expect(result.error?.message).toMatch(/page 2 of 3, run 1 of 1/);
    release();
    await new Promise((resolve) => setTimeout(resolve, 600));
    expect(calls).toBe(2);
    expect(run.fake.state.closed).toBe(1);
    expect(profiles()).toEqual([]);
  });
});

describe("a harness that does not close", () => {
  it("does not hold the gate: the close is bounded and reported, and the measured score is kept", async () => {
    const fake = harness();
    fake.close = () => new Promise(() => undefined);
    const started = Date.now();
    const run = gate({ browser: browsers.ok(), fake, limits: { closeTimeoutMs: 300 } });
    expect(await outcome(run.promise)).toEqual({ code: 0 });
    expect(Date.now() - started).toBeLessThan(4000);
    expect(run.err.join("")).toMatch(/the harness did not close in 300 ms/);
    expect(existsSync(join(resultsDir, "web-lighthouse_performance.json"))).toBe(true);
  });
});

describe("the arguments", () => {
  it("refuses one it does not know", async () => {
    const run = gate({ browser: browsers.ok(), argv: ["--nope"] });
    const result = await outcome(run.promise);
    expect(result.error?.message).toMatch(/unknown argument --nope/);
  });
});

describe("the limits of the real gate", () => {
  it("are all finite, leave a run and the whole gate twice the time they take on a quiet machine (about a minute, about eight minutes), and still end a stall", () => {
    for (const [name, ms] of Object.entries(LIMITS)) {
      expect(Number.isFinite(ms) && ms > 0, name).toBe(true);
    }
    expect(LIMITS.runTimeoutMs).toBeGreaterThanOrEqual(2 * 60_000);
    expect(LIMITS.runTimeoutMs).toBeLessThanOrEqual(5 * 60_000);
    expect(LIMITS.deadlineMs).toBeGreaterThanOrEqual(16 * 60_000);
    expect(LIMITS.deadlineMs).toBeLessThanOrEqual(25 * 60_000);
    expect(LIMITS.startTimeoutMs).toBeLessThanOrEqual(60_000);
    expect(LIMITS.stopTimeoutMs).toBeLessThanOrEqual(30_000);
    expect(LIMITS.closeTimeoutMs).toBeLessThanOrEqual(30_000);
  });

  it("cannot be changed from outside (a frozen object)", () => {
    expect(Object.isFrozen(LIMITS)).toBe(true);
  });
});
