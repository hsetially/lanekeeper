// P10: Lighthouse performance, desktop preset, against the harness. The command; the gate itself is lighthouse-run.mjs, which
// takes its parts as arguments so that a test can put stand-ins in their place (its header says what the gate does and why every
// wait in it has a bound).
//
//   node scripts/perf/lighthouse.mjs [--config lighthouserc.json] [--runs N]
//
// Exit codes: 0 every page's median reaches the score; 1 a page's median is under it; 2 a run could not be made (no browser,
// Lighthouse failed, no score, a bound was reached): nothing is written then, so a broken run cannot pass. The message that
// says why is on stderr.
import lighthouse, { desktopConfig } from "lighthouse";
import { chromiumPath, launchArgs } from "../../e2e/harness/browser.ts";
import { startHarness } from "../../e2e/harness/serve.ts";
import { runLighthouseGate } from "./lighthouse-run.mjs";

runLighthouseGate({
  argv: process.argv.slice(2),
  startHarness,
  lighthouse,
  lighthouseConfig: desktopConfig,
  // The flags that this machine needs come from one place (e2e/harness/browser.ts), the same one the Playwright config uses.
  chromium: { binary: chromiumPath, args: launchArgs },
}).then(
  (code) => process.exit(code),
  (/** @type {unknown} */ error) => {
    process.stderr.write(`lighthouse: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(2);
  },
);
