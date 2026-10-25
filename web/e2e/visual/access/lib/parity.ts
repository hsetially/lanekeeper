// Tier 2 of the plan: for a state the design draws as a small specimen (a dialog 600 px wide, a band of a screen), the words the design shows
// must be in the app's state, line by line, so that a sentence the design says and the app does not is a failure and not something a
// person has to notice. What the design says and the app does not is LISTED, each with its reason (a row of design/DEVIATIONS.md, or
// the contract that cannot fill it): the list is compared both ways, so a difference that is gone fails as well as one that appeared,
// and the list cannot rot.
//
// A line is a line of the browser's `innerText` of the frame (so what is laid out as one text block is one line, however it wraps), with its
// white space collapsed. A line is present when the app's whole text contains it. Names of people are not compared (personal names are
// masked: the design's Priya Shah and Marco Diaz are the scenario's, and the scenario's own are what the test sees).
import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../../../fixtures/test";
import { baseline } from "./matrix";
import { pin, settle, type Clock, type Prepare, type Viewport } from "./app";
import { frameOf, referenceText, referencesAvailable } from "./reference";

export const collapse = (text: string): string => text.replace(/\s+/g, " ").trim();
export const linesOf = (text: string): string[] => text.split("\n").map(collapse).filter((line) => line !== "");

/** The lines of the reference that the app's text does not contain. */
export function missingLines(reference: string, appText: string): string[] {
  const haystack = collapse(appText);
  return linesOf(reference).filter((line) => !haystack.includes(line));
}

export type Difference = { text: string; why: string };

export type ParityCase = {
  name: string;
  board: string;
  id: string;
  scenario: string;
  path: string;
  /** A specimen has no theme of its own in its frame; "dark" for the board's dark group. */
  theme?: "light" | "dark";
  viewport?: Viewport;
  clock?: Clock;
  before?: Prepare;
  /** Open the state in the app. */
  prepare?: Prepare;
  /** Where the state is: the dialog, the panel. */
  region: (page: Page) => Locator;
  /** The reference's lines that the app does not say, and why. */
  differences?: readonly Difference[];
  /** Regions that move by themselves, for the baseline. */
  moving?: (page: Page) => readonly Locator[];
};

const PEOPLE = /Priya Shah|Marco Diaz|Sam Okafor/g;

export function parity(cases: readonly ParityCase[]): void {
  for (const c of cases) {
    const frame = frameOf(c.board, c.id);
    const theme = c.theme ?? (frame.theme === "dark" ? "dark" : "light");
    test(`tier 2 · ${c.board.replace(/^Lanekeeper /, "").replace(/\.dc\.html$/, "")} ${c.id} (${theme}): ${c.name}`, async ({ page, harness, browser, csp }) => {
      test.setTimeout(150_000);
      const missingReference = referencesAvailable();
      if (missingReference !== null) {
        if (process.env["CI"]) throw new Error(`the reference of frame ${c.id} cannot be rendered in CI: ${missingReference}`);
        test.skip(true, `NOT RUN: ${missingReference}`);
      }
      const reference = (await referenceText(browser, c.board, c.id)).replace(PEOPLE, "");
      harness.setScenario(c.scenario);
      await pin(page, theme, c.viewport ?? { width: 1440, height: 900 }, c.clock);
      if (c.before) await c.before(page, harness);
      await page.goto(`${harness.url}${c.path}`);
      await settle(page);
      if (c.prepare) {
        await c.prepare(page, harness);
        await settle(page, { park: false });
      }
      const region = c.region(page);
      await region.waitFor();
      const text = (await region.innerText()).replace(PEOPLE, "");
      const missing = missingLines(reference, text);
      const listed = (c.differences ?? []).map((d) => collapse(d.text));
      if (process.env["LK_PARITY_LOG"]) console.log(`PARITY ${c.id} ${c.name}\n${JSON.stringify(missing, null, 1)}`);
      expect(missing.sort(), `frame ${c.id}: the lines of the design that the app does not say, against the listed differences`).toEqual([...listed].sort());
      expect(csp.report(), "CSP and Trusted Types violations").toBe("");
      await baseline(page, `${c.id}-${c.name.replace(/\W+/g, "-").toLowerCase()}-${theme}.png`, { element: region, mask: c.moving?.(page) ?? [] });
    });
  }
}

