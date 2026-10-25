// Frame 1l of the Consequence Dialogs board ("Drift list · PR badges sit beside the drift status and link to the PR") against the app's Drift tab.
//
// The frame is a card drawn in the board itself, not a frame of its own, so it has no reference image. It is rendered from the design in the
// same browser as the app (e2e/visual/attribution/elements.ts) and compared in two ways:
//  - the words: every word the design's card says (a badge, a note, a path) must be on the app's Drift tab, in a badge, in a note (the badge's
//    `title`) or in a row. What the app does not say is LISTED with its reason (design/DEVIATIONS.md: "Pull request badges", "Swimlane detail: Drift tab"), and the list is
//    compared both ways, so a difference that is gone fails as well as one that appeared (the plan's tier 2, as e2e/visual/access/lib/parity.ts).
//  - the badges: "PR #418 open" and "Fix merged, awaiting sync" photographed in the design and in the app, and compared pixel by pixel (tier 1).
// Without the React files the tests are NOT RUN, and in CI they fail: a missing reference is never a pass.
import type { Page } from "@playwright/test";
import { expect, test } from "../../fixtures/test";
import { collapse } from "../access/lib/parity";
import { appPieces, comparePieces, designPieces, expectSamePiece, umd, type DesignPick } from "../attribution/elements";
import { openApp, type FrameDef } from "../search/kit";

const IN_CI = Boolean(process.env["CI"]);
const BOARD = "Lanekeeper Consequence Dialogs.dc.html";
const CARD = "[id='1l'] .card";
test.use({ dist: process.env["LK_DIST"] ?? "dist" });

const DRIFT: FrameDef = {
  board: "states", frame: "E", kind: "app", scenario: "design-prs-drift-badges", path: "/swimlanes/sitb/drift", title: "badges", why: "badges",
};

const NOTE = {
  open: "An open PR carries this change. Clears when it merges or closes.",
  merged: "The PR merged but sitb hasn't synced yet. Clears on the next sync, no action needed.",
};

const WHY = {
  noPr: "design/DEVIATIONS.md: \"Pull request badges\": no \"No PR\" in a row that has none; the badge is drawn only where there is a pull request",
  notes: "design/DEVIATIONS.md: \"Pull request badges\": the notes spell out \"pull request\" and \"the swimlane\", where the design's say \"PR\" and the swimlane's name",
  lane: "dataset: the Drift tab is one swimlane's (sitb); the design's table lists the files of two, and #409 is sitc's",
  files: "dataset: the scenario's files are the Swimlanes slice's (application.properties, teller-ui.yml), not the design's example paths",
};


async function designWords(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const root = document.querySelector("[id='1l']");
    if (root === null) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    const add = (text: string): void => {
      const t = text.replace(/\s+/g, " ").trim();
      if (t !== "" && !seen.has(t)) {
        seen.add(t);
        out.push(t);
      }
    };
    for (const badge of Array.from(root.querySelectorAll(".bd"))) {
      const t = (badge.textContent ?? "").trim();
      if (t.startsWith("PR #") || t.startsWith("Fix merged")) add(t);
    }
    for (const cell of Array.from(root.querySelectorAll("td.mono"))) add(cell.textContent ?? "");
    for (const note of Array.from(root.querySelectorAll(".mut"))) add(note.textContent ?? "");
    return out;
  });
}

test("tier 2 · Consequence Dialogs 1l: the words of the design's card are on the Drift tab, but for the listed differences", async ({ page, harness, browser, csp }) => {
  test.setTimeout(150_000);
  if (!umd.ok) {
    if (IN_CI) throw new Error(`NOT RUN is not allowed in CI: ${umd.reason ?? "no React files"}`);
    test.skip(true, `NOT RUN: ${umd.reason ?? "no React files"}`);
    return;
  }
  // The design's words: the board is drawn in a page of its own, and the card's text is read from it.
  const reference = await browser.newPage();
  let words: string[];
  try {
    const { startReferenceServer } = await import("../render-references.mjs");
    const server = await startReferenceServer();
    try {
      await reference.addInitScript(({ react, reactDom }) => { (0, eval)(react); (0, eval)(reactDom); }, { react: umd.react, reactDom: umd.reactDom });
      await reference.goto(`${server.origin}/${encodeURIComponent(BOARD)}`);
      await reference.waitForSelector("[id='1l'] .card");
      await reference.evaluate(() => document.fonts.ready);
      words = await designWords(reference);
    } finally {
      await server.close();
    }
  } finally {
    await reference.close();
  }
  expect(words, "the card's words were read").toEqual(expect.arrayContaining(["PR #418 open", "Fix merged, awaiting sync", NOTE.open, NOTE.merged]));

  await openApp(DRIFT, { page, harness, size: { width: 1440, height: 900 } }, "light");
  const text = await page.evaluate(() => {
    const main = document.querySelector("main")?.innerText ?? "";
    const titles = Array.from(document.querySelectorAll("[data-slot='pr-badge']")).map((el) => el.getAttribute("title") ?? "");
    return [main, ...titles].join("\n");
  });
  const haystack = collapse(text);
  const missing = words.filter((word) => !haystack.includes(collapse(word)));
  const listed: Array<{ text: string; why: string }> = [
    { text: "No PR", why: WHY.noPr },
    { text: "PR #409 open", why: WHY.lane },
    { text: "sitc", why: WHY.lane },
    { text: NOTE.open, why: WHY.notes },
    { text: NOTE.merged, why: WHY.notes },
    { text: "tx-infinity-api/application-sit7.properties", why: WHY.files },
    { text: "ui/remote-itm-teller/ui-common-config.yaml", why: WHY.files },
    { text: "tx-infinity-api/tx-infinity-gateway.yml", why: WHY.lane },
  ];
  expect([...missing].sort(), "the words of the design's frame 1l that the app does not say, against the listed differences").toEqual(listed.map((l) => collapse(l.text)).sort());
  expect(csp.report(), "CSP and Trusted Types violations").toBe("");
});

// ----- the badges, pixel by pixel ----------------------------------------------------------------------------------------------------------

type Boxes = Record<string, { x: number; y: number; width: number; height: number }>;

/** In the design's card: the first "PR #n open" badge and the first "Fix merged" one, copied to a stage at the window's corner. */
const designBadges: DesignPick = (root) => {
  const stage = document.createElement("div");
  Object.assign(stage.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", display: "flex", flexDirection: "column", alignItems: "flex-start", gap: "12px", width: "max-content", background: "var(--background)", color: "var(--foreground)" });
  root.appendChild(stage);
  const out: Boxes = {};
  for (const [name, selector] of [["badge open", ".bd-pr"], ["badge merged", ".bd-merged"]] as const) {
    const source = root.querySelector(selector);
    if (source === null || stage.contains(source)) continue;
    const clone = source.cloneNode(true) as Element;
    stage.appendChild(clone);
    const r = clone.getBoundingClientRect();
    out[name] = { x: r.x, y: r.y, width: r.width, height: r.height };
  }
  return out;
};

const appBadges = (page: Page): Promise<Boxes> =>
  page.evaluate(() => {
    const stage = document.createElement("div");
    Object.assign(stage.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", display: "flex", flexDirection: "column", alignItems: "flex-start", gap: "12px", width: "max-content", background: "var(--background)", color: "var(--foreground)" });
    document.body.appendChild(stage);
    const out: Record<string, { x: number; y: number; width: number; height: number }> = {};
    for (const [name, state] of [["badge open", "open"], ["badge merged", "merged"]] as const) {
      const source = document.querySelector(`main [data-slot='pr-badge'][data-pr-state='${state}']`);
      if (source === null) continue;
      const clone = source.cloneNode(true) as Element;
      stage.appendChild(clone);
      const r = clone.getBoundingClientRect();
      out[name] = { x: r.x, y: r.y, width: r.width, height: r.height };
    }
    return out;
  });

test("tier 1 · Consequence Dialogs 1l: the badges \"PR #418 open\" and \"Fix merged, awaiting sync\" are the design's", async ({ page, harness, browser, csp }) => {
  test.setTimeout(150_000);
  if (!umd.ok) {
    if (IN_CI) throw new Error(`NOT RUN is not allowed in CI: ${umd.reason ?? "no React files"}`);
    test.skip(true, `NOT RUN: ${umd.reason ?? "no React files"}`);
    return;
  }
  await openApp(DRIFT, { page, harness, size: { width: 1440, height: 900 } }, "light");
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(1439, 899);
  const design = await designPieces(browser, BOARD, "1l", "light", "badges", designBadges, CARD);
  const app = await appPieces(page, appBadges);
  const scratch = await browser.newPage();
  try {
    const compared = await comparePieces(scratch, design, app, ["badge open", "badge merged"], "Consequence Dialogs 1l badges");
    test.info().annotations.push({ type: "pieces", description: JSON.stringify(compared.map((c) => ("error" in c ? { name: c.name, error: c.error } : { name: c.name, ratio: Number((c.ratio * 100).toFixed(3)), size: `${c.width}x${c.height}` }))) });
    for (const piece of compared) await expectSamePiece(piece, { scratch, design, app, group: "Consequence Dialogs 1l badges" });
  } finally {
    await scratch.close();
  }
  expect(csp.report(), "CSP and Trusted Types violations").toBe("");
});
