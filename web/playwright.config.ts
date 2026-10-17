import { defineConfig } from "@playwright/test";
import { chromiumPath, launchArgs, recordBrowser } from "./e2e/harness/browser.ts";

// The e2e runs a hub's header code (the Rust `web_fixture` example) behind the harness (e2e/harness/serve.ts), started by
// the tests themselves (e2e/fixtures/test.ts), so there is no `webServer` here. The browser is whatever
// e2e/harness/browser.ts finds, and its version is written to target/web-gates/browser.json.
recordBrowser();

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
  use: {
    // Pinned so a screenshot, a time string and a media query mean the same everywhere.
    timezoneId: "UTC",
    locale: "en-GB",
    colorScheme: "light",
    reducedMotion: "reduce",
    viewport: { width: 1440, height: 900 },
    launchOptions: { executablePath: chromiumPath(), args: launchArgs() },
  },
  projects: [{ name: "chromium" }],
});
