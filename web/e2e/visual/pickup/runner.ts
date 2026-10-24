// The runner of the T6 frames (pickup chips, notify and restart, As served): the frame tables of those three directories are registered
// here. It is the T5 kit's (e2e/visual/search/kit.ts) frame, geometry, `settle` and pixel rules, with four differences that these screens need:
//
//  - a frame says what its state IS in words (`check`), and the test runs the state and its check whether or not there is a baseline. The
//    kit skips a frame without a baseline before it opens the app, so a run in a sandbox (where no baseline is made) proves nothing about the
//    frames; here every run reaches every state and reads it, and only the photograph is NOT RUN (the test is reported skipped, never
//    passed, and in CI a missing baseline fails: Playwright is told to create none, e2e/visual/baseline-rule.mjs);
//  - a frame whose state IS a hover (a tooltip over a pickup chip, over a disabled button) says `hold`, and the pointer is not parked in the
//    corner before the photograph, which would close the tooltip. The kit always parks it;
//  - no request of a frame goes unanswered: a screen that asked for something its scenario cannot answer is photographed as an error panel,
//    and a baseline of that would guard the wrong thing. `unanswered` lists the paths (without the query) a frame leaves unanswered on purpose;
//  - the clock can be the `Date`-only one (`clock: "date"`) that a page which types into the editor needs: the file page after a save.
//
// None of the frames is a comparison with the design: the handoff has no frame for any T6 surface (plan F8, W6), so each is the app against
// its own baseline, which CI generates (the plan's "Baseline bootstrap"). Each frame's `why` names the rows of design/DEVIATIONS.md that
// make it so.
import { existsSync } from "node:fs";
import { expect, test } from "../../fixtures/test";
import { baselineIsNotRun } from "../baseline-rule.mjs";
import { pin, type Clock } from "../access/lib/app";
import { THRESHOLD, MAX_RATIO, geometryOf, settle, type Env, type FrameDef, type Step } from "../search/kit";

export type { Env, Step };

export type Frame = FrameDef & {
  /** What the state is, said in words: runs in every run, with or without a baseline. */
  check?: Step;
  /** The state is a hover: the pointer stays where a step put it. */
  hold?: boolean;
  /**
   * How time is frozen (access/lib/app.ts `pin`): "fixed" is Playwright's clock, which every other frame uses; "date" freezes `Date` only, which a
   * page that types into the editor needs (Monaco's input-latency probe reads `performance` entries that the fixed clock replaces).
   */
  clock?: Clock;
  /** Paths (no query) that the scenario does not answer and the frame does not need answered (a request that is held, refused or aborted by the frame itself). */
  unanswered?: readonly string[];
};

const IN_CI = Boolean(process.env["CI"]);
/** Which build the harness serves: `dist`, or another directory for a run on a private build (LK_DIST). */
const DIST = process.env["LK_DIST"] ?? "dist";

/** Several steps, one after the other. */
export const then = (...steps: Step[]): Step => async (env) => {
  for (const step of steps) await step(env);
};

/** A reason names the rows of the deviations file that make a frame the app's own: the file, a colon, and a quoted row. */
const NAMES_A_ROW = /design\/DEVIATIONS\.md: "[^"]+"/;

/** A frame of the T6 matrix: an app-owned baseline of a state of the States index. */
export const frame = (code: string, def: Omit<Frame, "board" | "frame" | "kind">): Frame => ({ board: "states", frame: code, kind: "app", ...def });

/** The same frame in light and in dark: one code, two baselines (the theme is in the file name). */
export const pair = (code: string, def: Omit<Frame, "board" | "frame" | "kind" | "theme">): Frame[] => [frame(code, { ...def, theme: "light" }), frame(code, { ...def, theme: "dark" })];

/** Bring the app to the frame: scenario, size, scheme, frozen clock, the address, the steps; return when nothing is loading (or the frame's `ready`). */
async function openApp(def: Frame, env: Env, theme: "light" | "dark"): Promise<void> {
  const { page, harness, size } = env;
  // A step that waits for something that never comes fails in seconds, not after the test's own limit.
  page.setDefaultTimeout(15_000);
  harness.setScenario(def.scenario);
  await pin(page, theme, size, def.clock ?? "fixed");
  await def.before?.(env);
  await page.goto(`${harness.url}${def.path}`);
  await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
  if (def.ready) {
    await def.steps?.(env);
    await def.ready(env);
    return;
  }
  await settle(page, harness);
  await def.steps?.(env);
  await settle(page, harness);
}

/** Register the frames of one area as tests. Call once per spec file, at the top level. `area` names the baselines and the group. */
export function registerFrames(area: string, defs: readonly Frame[]): void {
  const seen = new Set<string>();
  for (const def of defs) {
    if (def.kind !== "app" || def.board !== "states") throw new Error(`frame ${def.frame}: the T6 frames are app baselines on no board`);
    if (def.why === undefined || !NAMES_A_ROW.test(def.why)) throw new Error(`frame ${def.frame}: an app baseline says which row of the deviations file makes it one (\`why\` names it, quoted)`);
    const key = `${def.frame}-${geometryOf(def).theme}`;
    if (seen.has(key)) throw new Error(`frame ${key} is registered twice: two frames would share one baseline`);
    seen.add(key);
  }

  test.describe(`visual matrix: ${area}`, () => {
    test.use({ dist: DIST });

    for (const def of defs) {
      const { theme, size } = geometryOf(def);
      const label = `app baseline frame ${def.frame} (${theme}): ${def.title}`;
      test(label, async ({ page, harness, csp }) => {
        test.setTimeout(150_000);
        test.info().annotations.push({ type: "why", description: def.why ?? "" });
        const env: Env = { page, harness, size };
        await openApp(def, env, theme);
        await def.check?.(env);

        const unanswered = harness
          .requests()
          .filter((r) => r.unmocked === true && !(def.unanswered ?? []).includes(r.path.split("?")[0] ?? ""))
          .map((r) => `${r.method} ${r.path}`);
        expect(unanswered, "requests the scenario had no answer for: the frame would photograph an error").toEqual([]);
        expect(csp.report(), "CSP and Trusted Types violations").toBe("");

        // The state was reached and read; without a baseline the photograph is not taken, and the test says so by being skipped.
        const file = `${area}-${def.frame}-${theme}.png`;
        if (baselineIsNotRun({ exists: existsSync(test.info().snapshotPath(file)), ci: IN_CI, updateSnapshots: test.info().config.updateSnapshots })) {
          test.info().annotations.push({ type: "baseline NOT RUN", description: `${file}: no baseline yet (baselines are generated in CI only)` });
          test.skip(true, "NOT RUN: no baseline yet (baselines are generated in CI only)");
          return;
        }
        await page.evaluate(() => document.fonts.ready);
        if (def.hold !== true) await page.mouse.move(size.width - 1, size.height - 1);
        await page.waitForTimeout(250);
        await expect(page).toHaveScreenshot(file, {
          animations: "disabled",
          caret: "hide",
          clip: { x: 0, y: 0, ...size },
          // The live pill is the one live region of a frame: while the stream is refused it counts down to the next try with a jitter.
          mask: [page.locator("[data-slot='live-indicator']"), ...(def.mask?.(page) ?? [])],
          threshold: THRESHOLD,
          maxDiffPixelRatio: MAX_RATIO,
        });
      });
    }
  });
}
