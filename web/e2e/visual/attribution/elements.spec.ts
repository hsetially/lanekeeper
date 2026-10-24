// Tier 1, the pieces of the attribution chip against the design's (see elements.ts): the words "Marco Diaz · via Lanekeeper", the confidence
// meter, the severity badge and the "unknown author" badge, each photographed in the design's own frame and in the app, and compared.
//
// A piece that the design draws and the app draws differently, by a row of design/DEVIATIONS.md, is not compared: `differs` names it with
// its row, and the test asserts that it DIFFERS (so that the row stays true, and fails the day the app is aligned and the piece should be
// compared). Without the React files the test is NOT RUN, and in CI it fails: a missing reference is never a pass.
import { test, expect } from "../../fixtures/test";
import type { Page } from "@playwright/test";
import { openApp, type FrameDef } from "../search/kit";
import { attributedDrift, HISTORY } from "./frames";
import { MAX_RATIO, appPieces, comparePieces, designPieces, expectSamePiece, umd, type DesignPick } from "./elements";

const IN_CI = Boolean(process.env["CI"]);
const FILE_VIEW = "Lanekeeper File View.dc.html";
const DETAIL = "Lanekeeper Swimlane Detail.dc.html";
const AUDIT_BOARD = "Lanekeeper Approvals and Audit.dc.html";
const DIST = process.env["LK_DIST"] ?? "dist";
test.use({ dist: DIST });

// ----- the pieces, on a stage -------------------------------------------------------------------------------------------------------------
// Both functions run in a page (the design's, the app's) and name nothing outside themselves. Each copies the pieces it finds into a stage at
// the corner of the window and returns their boxes there. A chip is copied whole (its row or its button), so the words and the meter keep
// the type and the gaps they have, and the design's "Revert" button is left out of its row.

type Boxes = Record<string, { x: number; y: number; width: number; height: number }>;

/** In the design's frame: the chips of its rows (the words and the meter), the severity badges and the unknown-author badge. */
const designPieces_: DesignPick = (frame) => {
  const text = (el: Element): string => (el.textContent ?? "").replace(/\s+/g, " ").trim();
  const stage = document.createElement("div");
  // Through the style object, not the `style` attribute: the app's CSP refuses an inline style attribute, and the test would be the violation.
  Object.assign(stage.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", display: "flex", flexDirection: "column", alignItems: "flex-start", gap: "12px", padding: "0", width: "max-content", background: "var(--background)", color: "var(--foreground)" });
  frame.appendChild(stage);
  const out: Boxes = {};
  const put = (name: string, el: Element): void => {
    const r = el.getBoundingClientRect();
    if (!(name in out)) out[name] = { x: r.x, y: r.y, width: r.width, height: r.height };
  };
  const copy = (el: Element): Element => {
    const clone = el.cloneNode(true) as Element;
    stage.appendChild(clone);
    return clone;
  };
  // A chip is the flex element that holds the words and a meter (and, in History, the Revert button).
  const meters = Array.from(frame.querySelectorAll("[title^='Attribution confidence:']")).filter((m) => m.getBoundingClientRect().height >= 10 && !stage.contains(m));
  for (const meter of meters) {
    const row = meter.parentElement;
    if (row === null) continue;
    const clone = copy(row);
    for (const extra of Array.from(clone.querySelectorAll("button, a"))) extra.remove();
    const [words, score] = Array.from(clone.children);
    if (words === undefined || score === undefined) continue;
    // The meter's vertical extent is the chip's: the words' own box is a line taller in the design (18 px) than in the app (16 px).
    const m = score.getBoundingClientRect();
    const w = words.getBoundingClientRect();
    const level = (meter.getAttribute("title") ?? "").replace("Attribution confidence: ", "");
    // The meter is photographed alone, at a whole pixel: where it sits in its row decides the fraction of a pixel it is drawn at.
    if (!(`meter ${level}` in out)) put(`meter ${level}`, copy(meter));
    if (!(`words ${text(words)}` in out)) out[`words ${text(words)}`] = { x: w.x, y: m.y, width: w.width, height: m.height };
  }
  const seen = new Set<string>();
  for (const badge of Array.from(frame.querySelectorAll("span"))) {
    if (stage.contains(badge)) continue;
    const r = badge.getBoundingClientRect();
    const label = text(badge);
    if (["Critical", "High", "Medium", "Low"].includes(label) && badge.children.length === 2 && r.height >= 19.5 && !seen.has(label)) {
      seen.add(label);
      put(`sev ${label}`, copy(badge));
    }
    if (label === "unknown author" && getComputedStyle(badge).borderStyle === "dashed" && !seen.has(label)) {
      seen.add(label);
      put("unknown author", copy(badge));
    }
  }
  return out;
};

/** In the app, inside `within`: the same pieces. */
const appStage = (within: string) => (page: Page): Promise<Boxes> =>
  page.evaluate((scope) => {
    const LEVEL: Record<string, string> = { certain: "Certain", high: "High", medium: "Medium", low: "Low", none: "Unknown" };
    const text = (el: Element): string => (el.textContent ?? "").replace(/\s+/g, " ").trim();
    const root = document.querySelector(scope) ?? document.body;
    const stage = document.createElement("div");
    // Through the style object, not the `style` attribute: the app's CSP refuses an inline style attribute, and the test would be the violation.
  Object.assign(stage.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", display: "flex", flexDirection: "column", alignItems: "flex-start", gap: "12px", padding: "0", width: "max-content", background: "var(--background)", color: "var(--foreground)" });
    document.body.appendChild(stage);
    const out: Boxes = {};
    const put = (name: string, el: Element): void => {
      const r = el.getBoundingClientRect();
      if (!(name in out)) out[name] = { x: r.x, y: r.y, width: r.width, height: r.height };
    };
    const copy = (el: Element): Element => {
      const clone = el.cloneNode(true) as Element;
      stage.appendChild(clone);
      return clone;
    };
    for (const chip of Array.from(root.querySelectorAll("[data-slot='attribution'] button"))) {
      if (stage.contains(chip)) continue;
      const clone = copy(chip);
      const [words, score] = Array.from(clone.children);
      if (words === undefined || score === undefined) continue;
      const m = score.getBoundingClientRect();
      const w = words.getBoundingClientRect();
      const level = LEVEL[score.getAttribute("data-level") ?? ""] ?? "?";
      if (!(`meter ${level}` in out)) put(`meter ${level}`, copy(score));
      if (!(`words ${text(words)}` in out)) out[`words ${text(words)}`] = { x: w.x, y: m.y, width: w.width, height: m.height };
    }
    const seen = new Set<string>();
    for (const badge of Array.from(root.querySelectorAll("[data-slot='severity-badge']"))) {
      const severity = badge.getAttribute("data-severity") ?? "";
      const label = `${severity.charAt(0).toUpperCase()}${severity.slice(1)}`;
      if (seen.has(label)) continue;
      seen.add(label);
      put(`sev ${label}`, copy(badge));
    }
    const unknown = root.querySelector("[data-slot='unknown-author']");
    if (unknown !== null) put("unknown author", copy(unknown));
    return out;
  }, within);

// ----- the groups ------------------------------------------------------------------------------------------------------------------------

const SEVERITY = "design/DEVIATIONS.md: \"Severity badge (every screen)\": one badge from the Findings page's `.sv-*`; the design's File and Detail pages draw High, Medium and Low in other colours and icons";
const CRITICAL = "design/DEVIATIONS.md: \"Severity badge (every screen)\", \"Findings: severity\": Critical is told from High by its shield, its word and a heavier weight (600); the design's Detail page draws it at 500";
const GAP = "design/DEVIATIONS.md: \"Attribution chip\": the design's Detail page puts a 6 px gap between the name and the phrase (its File and Activity pages a space); the app draws a space everywhere";

type Group = {
  name: string;
  design: { board: string; id: string; theme: "light" | "dark" };
  app: Pick<FrameDef, "scenario" | "path" | "before" | "steps"> & { scope: string };
  /** The pieces that are compared. */
  same: readonly string[];
  /** Pieces the design and the app draw differently by a row of design/DEVIATIONS.md: asserted to differ, so that the row stays true. */
  differs?: Readonly<Record<string, string>>;
};

const asFrame = (g: Group): FrameDef => ({ board: "states", frame: "E", kind: "app", title: g.name, why: "elements", ...g.app });

const HISTORY_SCOPE = "aside[aria-label='History']";
const ACTIVITY = "/swimlanes/sitb/activity";
const DRIFT = "/swimlanes/sitb/drift";

const groups: Group[] = [
  {
    name: "File 1j, the History panel: the words and the meters of three versions, and two severities",
    design: { board: FILE_VIEW, id: "1j", theme: "light" },
    app: { scenario: "design-file-core", path: HISTORY, scope: HISTORY_SCOPE },
    same: ["words Marco Diaz · via Lanekeeper", "meter Certain", "words NFS client (no sync running)", "meter Medium", "meter High"],
    differs: { "sev Medium": SEVERITY, "sev Low": SEVERITY },
  },
  {
    name: "File 1j, the History panel: the unknown author's meter, and a High severity",
    design: { board: FILE_VIEW, id: "1j", theme: "light" },
    app: { scenario: "design-attribution-history", path: HISTORY, scope: HISTORY_SCOPE },
    same: ["meter Unknown"],
    differs: { "sev High": SEVERITY },
  },
  {
    name: "Detail 1e, the Drift tab",
    design: { board: DETAIL, id: "1e", theme: "light" },
    app: { scenario: "design-swimlanes-six", path: DRIFT, before: attributedDrift, scope: "main" },
    same: ["meter Certain", "meter Medium", "meter High", "meter Unknown"],
    differs: { "words Marco Diaz · via Lanekeeper": GAP, "sev Critical": CRITICAL, "sev High": SEVERITY, "sev Medium": SEVERITY, "sev Low": SEVERITY },
  },
  {
    name: "Detail 1f, the Drift tab, dark",
    design: { board: DETAIL, id: "1f", theme: "dark" },
    app: { scenario: "design-swimlanes-six", path: DRIFT, before: attributedDrift, scope: "main" },
    same: ["meter Certain", "meter Medium", "meter High", "meter Unknown"],
    differs: { "sev Critical": CRITICAL, "sev High": SEVERITY },
  },
  {
    name: "Detail 1i, the Activity tab",
    design: { board: DETAIL, id: "1i", theme: "light" },
    app: { scenario: "design-attribution-audit", path: ACTIVITY, scope: "main" },
    same: ["words Marco Diaz · via Lanekeeper", "words NFS client (no sync running)", "meter Certain", "meter High", "meter Medium", "meter Unknown"],
  },
  {
    name: "Audit 1v, the unknown author's badge",
    design: { board: AUDIT_BOARD, id: "1v", theme: "light" },
    app: { scenario: "design-attribution-audit", path: "/audit", scope: "main" },
    same: ["unknown author"],
  },
  {
    name: "Audit 1an, the unknown author's badge, dark",
    design: { board: AUDIT_BOARD, id: "1an", theme: "dark" },
    app: { scenario: "design-attribution-audit", path: "/audit", scope: "main" },
    same: ["unknown author"],
  },
];

for (const group of groups) {
  test(`tier 1 · pieces of the chip · ${group.name}`, async ({ page, harness, browser, csp }) => {
    test.setTimeout(150_000);
    if (!umd.ok) {
      if (IN_CI) throw new Error(`NOT RUN is not allowed in CI: ${umd.reason ?? "no React files"}`);
      test.skip(true, `NOT RUN: ${umd.reason ?? "no React files"}`);
      return;
    }
    const size = { width: 1440, height: 900 };
    await openApp(asFrame(group), { page, harness, size }, group.design.theme);
    await page.evaluate(() => document.fonts.ready);
    await page.mouse.move(size.width - 1, size.height - 1);
    await page.waitForTimeout(250);
    const design = await designPieces(browser, group.design.board, group.design.id, group.design.theme, "chips", designPieces_);
    const app = await appPieces(page, appStage(group.app.scope));
    const scratch = await browser.newPage();
    try {
      const names = [...group.same, ...Object.keys(group.differs ?? {})];
      const compared = await comparePieces(scratch, design, app, names, group.name);
      test.info().annotations.push({ type: "pieces", description: JSON.stringify(compared.map((c) => ("error" in c ? { name: c.name, error: c.error } : { name: c.name, ratio: Number((c.ratio * 100).toFixed(3)), size: `${c.width}x${c.height}` }))) });
      for (const piece of compared) {
        const row = group.differs?.[piece.name];
        if (row === undefined) {
          await expectSamePiece(piece, { scratch, design, app, group: group.name });
        } else {
          // A documented difference: it has to still be one. A piece that is now the same is a row to delete and a piece to compare.
          const differs = "error" in piece || piece.ratio > MAX_RATIO;
          expect.soft(differs, `${piece.name} is now the same as the design's, so the row "${row}" is out of date: compare the piece`).toBe(true);
        }
      }
    } finally {
      await scratch.close();
    }
    expect(csp.report(), "CSP and Trusted Types violations").toBe("");
  });
}
