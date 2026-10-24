import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { launchArgs } from "../../e2e/harness/browser.ts";

// The Chromium of Lighthouse is started by scripts/perf/chromium-process.mjs with `launchArgs()`; Playwright starts its own and
// adds `--no-sandbox` by itself (it does unless `chromiumSandbox: true`; playwright-core 1.63, coreBundle.js), which is the likely
// reason the end-to-end job ran on GitHub and the Lighthouse step did not (a hypothesis: nothing here can reproduce the runner):
// on `ubuntu-latest` (24.04) AppArmor does not let an unprivileged user make the namespaces that Chromium's sandbox needs, and
// the runner is not root, so a browser with the sandbox on exits before it opens a port ("No usable sandbox!"). The flag is not
// given everywhere: a person who runs the gates on a workstation keeps the sandbox. It is off for root (Chromium refuses to start
// as root with it on), on GitHub Actions (a throw-away machine that holds no secret of ours and loads only our own pages through
// the harness), and when someone asks (LK_NO_SANDBOX=1).

const USER = 1000;

describe("launchArgs", () => {
  it("keeps the sandbox for an ordinary user on an ordinary machine", () => {
    expect(launchArgs({}, USER)).toEqual(["--disable-dev-shm-usage"]);
    expect(launchArgs({ CI: "true" }, USER)).not.toContain("--no-sandbox");
    expect(launchArgs({ GITHUB_ACTIONS: "false" }, USER)).not.toContain("--no-sandbox");
    expect(launchArgs({ LK_NO_SANDBOX: "0" }, USER)).not.toContain("--no-sandbox");
    expect(launchArgs({ LK_NO_SANDBOX: "" }, USER)).not.toContain("--no-sandbox");
  });

  it("turns the sandbox off on GitHub Actions, where the runner is not root and 24.04 does not allow the sandbox", () => {
    expect(launchArgs({ GITHUB_ACTIONS: "true" }, 1001)).toContain("--no-sandbox");
  });

  it("turns the sandbox off for root, as before (Chromium refuses to run as root with it)", () => {
    expect(launchArgs({}, 0)).toContain("--no-sandbox");
  });

  it("turns the sandbox off when someone asks for it with LK_NO_SANDBOX=1, and only then", () => {
    expect(launchArgs({ LK_NO_SANDBOX: "1" }, USER)).toContain("--no-sandbox");
  });

  it("always keeps /dev/shm out of it (the small /dev/shm of containers)", () => {
    for (const env of [{}, { GITHUB_ACTIONS: "true" }, { LK_NO_SANDBOX: "1" }]) expect(launchArgs(env, USER)).toContain("--disable-dev-shm-usage");
  });

  it("does not hold a flag twice", () => {
    const args = launchArgs({ GITHUB_ACTIONS: "true", LK_NO_SANDBOX: "1" }, 0);
    expect(new Set(args).size).toBe(args.length);
  });

  it("reads the process's own environment and user when it is given none", () => {
    const expected = typeof process.getuid === "function" && process.getuid() === 0;
    const own = process.env["GITHUB_ACTIONS"] === "true" || process.env["LK_NO_SANDBOX"] === "1";
    expect(launchArgs().includes("--no-sandbox")).toBe(expected || own);
  });
});

describe("the Lighthouse runner", () => {
  it("starts its browser with launchArgs() and nothing that turns the sandbox off by itself", () => {
    const entry = readFileSync(new URL("./lighthouse.mjs", import.meta.url), "utf8");
    const run = readFileSync(new URL("./lighthouse-run.mjs", import.meta.url), "utf8");
    const process_ = readFileSync(new URL("./chromium-process.mjs", import.meta.url), "utf8");
    expect(entry).toMatch(/args: launchArgs/);
    expect(entry).toMatch(/import \{[^}]*launchArgs[^}]*\} from "\.\.\/\.\.\/e2e\/harness\/browser\.ts"/);
    for (const source of [entry, run, process_]) expect(source.replace(/\/\/.*$/gm, "")).not.toContain("--no-sandbox");
  });
});
