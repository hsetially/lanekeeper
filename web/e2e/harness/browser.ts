// Which Chromium runs the e2e, Lighthouse and performance gates, and a record of its version (every P10 number in the
// evidence file names the browser it was measured in).
//
// Order: LK_CHROMIUM, then Playwright's own build for the installed @playwright/test, then a Chromium found under
// PLAYWRIGHT_BROWSERS_PATH (the sandbox has Chromium 141 there while the pinned Playwright expects a newer revision). It
// fails closed: with none of them there is no browser and the gates must not pass by skipping.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const WEB_ROOT = fileURLToPath(new URL("../../", import.meta.url));

function probe(root: string): string | undefined {
  if (!existsSync(root)) return undefined;
  const dirs = readdirSync(root)
    .filter((d) => /^chromium-\d+$/.test(d))
    .sort()
    .reverse();
  for (const dir of dirs) {
    for (const rel of ["chrome-linux/chrome", "chrome-linux64/chrome", "chrome-mac/Chromium.app/Contents/MacOS/Chromium", "chrome-win/chrome.exe"]) {
      const candidate = join(root, dir, rel);
      if (existsSync(candidate)) return candidate;
    }
  }
  return undefined;
}

export function chromiumPath(): string {
  const fromEnv = process.env["LK_CHROMIUM"];
  if (fromEnv) {
    if (!existsSync(fromEnv)) throw new Error(`LK_CHROMIUM=${fromEnv} does not exist`);
    return fromEnv;
  }
  const own = chromium.executablePath();
  if (existsSync(own)) return own;
  const found = probe(process.env["PLAYWRIGHT_BROWSERS_PATH"] ?? join(process.env["HOME"] ?? "", ".cache", "ms-playwright"));
  if (found) return found;
  throw new Error("no Chromium found: set LK_CHROMIUM, or run `pnpm exec playwright install chromium`");
}

/**
 * The flags that the machine needs, for a browser that this code starts itself (Lighthouse, scripts/perf/chromium-process.mjs) or
 * that Playwright starts with `executablePath` (playwright.config.ts). The sandbox is left ON unless one of three things says
 * otherwise:
 *  - the user is root: Chromium refuses to run as root with it on, and the sandbox of a container is the container;
 *  - `GITHUB_ACTIONS=true`: `ubuntu-latest` is 24.04, where AppArmor does not let an unprivileged user make the namespaces that
 *    the sandbox needs, and the runner is not root, so a Chromium started with the sandbox exits before it opens a port ("No
 *    usable sandbox!"; the likely reason the Lighthouse browser of the first run of the perf job exited, which then hung the step). The machine is thrown away after the run, nothing of
 *    ours that is secret is on it (the workflow reads no secret; only the job that commits baselines holds a token, and it runs
 *    Playwright, which has the sandbox off in every environment), and the pages are our own build behind the harness;
 *  - `LK_NO_SANDBOX=1`: someone on a machine with the same restriction asks for it, knowing what it is.
 * A person who runs the gates on a workstation as an ordinary user keeps the sandbox. Playwright adds `--no-sandbox` itself to
 * every browser it starts (unless `chromiumSandbox: true`), so it makes no difference to the specs.
 * @param env defaults to the process's environment
 * @param uid defaults to the process's user (undefined where there is none, on Windows)
 */
export function launchArgs(env: NodeJS.ProcessEnv = process.env, uid: number | undefined = typeof process.getuid === "function" ? process.getuid() : undefined): string[] {
  const args = ["--disable-dev-shm-usage"];
  if (uid === 0 || env["GITHUB_ACTIONS"] === "true" || env["LK_NO_SANDBOX"] === "1") args.push("--no-sandbox");
  return args;
}

/** Write target/web-gates/browser.json and return the version string. */
export function recordBrowser(): { path: string; version: string } {
  const path = chromiumPath();
  let version = "unknown";
  try {
    version = execFileSync(path, ["--version"], { encoding: "utf8", timeout: 10_000 }).trim();
  } catch {
    // The version is a record, not a gate.
  }
  const target = process.env["CARGO_TARGET_DIR"] ?? resolve(WEB_ROOT, "..", "target");
  const dir = join(target, "web-gates");
  try {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "browser.json"), `${JSON.stringify({ path, version, playwright: "@playwright/test", recordedAt: new Date().toISOString() }, null, 2)}\n`);
  } catch {
    // A read-only target dir (an image build) must not stop the run.
  }
  return { path, version };
}
