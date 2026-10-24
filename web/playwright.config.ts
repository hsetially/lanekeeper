import { defineConfig } from "@playwright/test";
import { chromiumPath, launchArgs, recordBrowser } from "./e2e/harness/browser.ts";

// The e2e runs a hub's header code (the Rust `web_fixture` example) behind the harness (e2e/harness/serve.ts), started by
// the tests themselves (e2e/fixtures/test.ts), so there is no `webServer` here. The browser is whatever
// e2e/harness/browser.ts finds, and its version is written to target/web-gates/browser.json.
recordBrowser();

// The three timing specs of the P10 gate run alone, one worker, in their own project (`just web-perf`): a measurement that
// shares the machine with another test measures the other test. Everything else, the functional checks of the same pages
// included, is the `chromium` project (`just web-e2e`).
const TIMING = /e2e[\\/]perf[\\/](route-change|diff-6000|grid-5000)\.spec\.ts$/;

export default defineConfig({
  testDir: "./e2e",
  testMatch: /.*\.spec\.ts/,
  fullyParallel: true,
  forbidOnly: true,
  retries: 0,
  workers: 2,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: [["list"]],
  outputDir: "test-results",
  // The app-owned baselines of the visual specs live in one folder per spec file, under e2e/visual/__baselines__/. They are made on the
  // runner that compares them (the `visual-baselines` job of .github/workflows/web-e2e.yml, started by hand or by the label
  // `bootstrap-baselines`), and that job commits them to the pull request's branch with `git add -f`: web/.gitignore keeps the folder
  // out of an ordinary `git add`, so a run on a developer's machine cannot put a PNG in a commit by accident, and a baseline that is
  // tracked is compared like any other file (the ignore line does not touch it). The `{testFilePath}` keeps two specs that name a
  // baseline alike apart.
  snapshotPathTemplate: "{testDir}/visual/__baselines__/{testFilePath}/{arg}{ext}",
  // A missing baseline is never created silently (e2e/visual/baseline-rule.mjs): only the job that makes them says so.
  updateSnapshots: process.env["LK_UPDATE_BASELINES"] ? "missing" : "none",
  use: {
    // Pinned so a screenshot, a time string and a media query mean the same everywhere.
    timezoneId: "UTC",
    locale: "en-GB",
    colorScheme: "light",
    reducedMotion: "reduce",
    viewport: { width: 1440, height: 900 },
    launchOptions: { executablePath: chromiumPath(), args: launchArgs() },
  },
  projects: [
    { name: "chromium", testIgnore: TIMING },
    { name: "perf", testMatch: TIMING, fullyParallel: false, timeout: 300_000 },
  ],
});
