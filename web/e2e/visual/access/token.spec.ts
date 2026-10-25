// Tier 1, the GitHub token screens: board "Access and Onboarding" frames 1g to 1t (onboarding in six states, and "My GitHub token")
// against the app on the `design-access-*` scenarios, 1280 wide.
//
// The design draws onboarding as a single focused page with a header of its own (name, role, Sign out); the app draws the same card
// inside the app shell ("GitHub token onboarding": drawn inside the app shell). So the frame cannot be compared whole: the whole frame is
// masked, and what the two draw the same, the heading, the sentence under it and the card, is compared where each is (the card is
// 112 px further right in the app, the shell's sidebar being 224 px wide and the page centred in what is left).
import type { Page } from "@playwright/test";
import { tier1, type FrameCase } from "./lib/matrix";
import type { Aligned, Mask } from "./lib/compare";
import { api, holdMethod } from "./lib/steps";

const BOARD = "Lanekeeper Access and Onboarding.dc.html";
const V = { width: 1280, height: 760 };
const ALL: Mask = { x: 0, y: 0, width: 1280, height: 760, why: "GitHub token onboarding: drawn inside the app shell, not the chrome-free page with its own header" };
// The heading and the sentence under it (y 70 to 154), and the card of instructions (172 to 373): the same in both.
const HEADING: Aligned = { ref: { x: 320, y: 70, width: 640, height: 84 }, app: { x: 432, y: 70, width: 640, height: 84 }, why: "heading and sentence" };
const INSTRUCTIONS: Aligned = { ref: { x: 320, y: 172, width: 640, height: 202 }, app: { x: 432, y: 172, width: 640, height: 202 }, why: "the card of instructions" };

async function paste(page: Page): Promise<void> {
  await page.getByLabel("2 · Paste the token").fill(`github_pat_${"x".repeat(40)}`);
  await page.getByRole("button", { name: "Verify and save" }).click();
}

const answered = (scenario: string, ids: readonly string[]): FrameCase => ({
  name: `onboarding, ${scenario.replace("design-access-onboarding-", "")} answer`,
  board: BOARD,
  ids,
  scenario,
  path: "/access/onboarding",
  viewport: V,
  prepare: async (page) => {
    await paste(page);
    await page.getByRole("alert").first().waitFor();
    await page.mouse.move(1279, 759);
  },
  masks: [ALL],
  aligned: [HEADING, INSTRUCTIONS],
});

const cases: FrameCase[] = [
  { name: "onboarding, empty", board: BOARD, ids: ["1g", "1h"], scenario: "design-access-onboarding", path: "/access/onboarding", viewport: V, masks: [ALL], aligned: [HEADING, INSTRUCTIONS] },
  {
    name: "onboarding, verifying",
    board: BOARD,
    ids: ["1i", "1j"],
    scenario: "design-access-onboarding",
    path: "/access/onboarding",
    viewport: V,
    before: async (page) => {
      await holdMethod(page, api("/me/github-token"), "PUT");
    },
    prepare: async (page) => {
      await paste(page);
      await page.getByRole("status").filter({ hasText: "Verifying with GitHub" }).waitFor();
      await page.mouse.move(1279, 759);
    },
    masks: [ALL],
    aligned: [HEADING, INSTRUCTIONS],
  },
  {
    name: "onboarding, saved: the login and the expiry only",
    board: BOARD,
    ids: ["1k", "1l"],
    scenario: "design-access-onboarding",
    path: "/access/onboarding",
    viewport: V,
    prepare: async (page) => {
      await paste(page);
      await page.getByText("mdiaz").first().waitFor();
      await page.mouse.move(1279, 759);
    },
    masks: [ALL],
    aligned: [HEADING],
  },
  answered("design-access-onboarding-classic", ["1m", "1n"]),
  answered("design-access-onboarding-repo", ["1o", "1p"]),
  answered("design-access-onboarding-mismatch", ["1q", "1r"]),
  {
    name: "my GitHub token: later visits, the token ends in 5 days",
    board: BOARD,
    ids: ["1s", "1t"],
    scenario: "design-access-mytoken",
    path: "/me/github-token",
    viewport: { width: 1280, height: 640 },
    prepare: async (page) => {
      await page.getByRole("button", { name: "Toggle sidebar" }).click();
      await page.mouse.move(1279, 639);
    },
    // The page differs from the design's by rows the contract cannot fill ("My GitHub token": Added and Last used dropped, UTC dates), by the
    // sidebar's collapsed rail (the app's has the findings count and the groups' gaps) and by the top bar (a breadcrumb "Account > ..." and no
    // swimlane switcher in the design). The banner is the design's 40 px and the app's 42 (its border is outside the button and padding), which
    // moves the page 2 px. What is the same in both is compared where each is: the heading with its sentence and badge, and the actions with
    // their warning, which are pixel for pixel the same.
    masks: [{ x: 0, y: 0, width: 1280, height: 640, why: "My GitHub token: the rows, the dates (UTC), the rail and the top bar differ" }],
    aligned: [
      { ref: { x: 72, y: 104, width: 720, height: 58 }, app: { x: 72, y: 106, width: 720, height: 58 }, why: "the heading, its sentence and the expiry badge" },
      { ref: { x: 72, y: 430, width: 660, height: 32 }, app: { x: 72, y: 398, width: 660, height: 32 }, why: "Replace token, Delete token and the sentence" },
    ],
  },
];
tier1(cases);
