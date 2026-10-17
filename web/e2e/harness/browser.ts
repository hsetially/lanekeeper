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

export function launchArgs(): string[] {
  const args = ["--disable-dev-shm-usage"];
  // Chromium refuses to run as root without it. The sandbox of CI containers is the container.
  if (typeof process.getuid === "function" && process.getuid() === 0) args.push("--no-sandbox");
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
