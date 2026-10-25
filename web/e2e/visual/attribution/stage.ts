// The stage the pieces of the attribution chip are photographed on, in the design's page and in the app's (see elements.ts and
// elements.spec.ts). Moved out of the spec so that stage.spec.ts can photograph the same stage in a browser with other font settings.
import type { Page } from "@playwright/test";
import type { DesignPick } from "./elements";

// ----- the pieces, on a stage -------------------------------------------------------------------------------------------------------------
// Both functions run in a page (the design's, the app's) and name nothing outside themselves. Each copies the pieces it finds into a stage at
// the corner of the window and returns their boxes there. A chip is copied whole (its row or its button), so the words and the meter keep
// the type and the gaps they have, and the design's "Revert" button is left out of its row.

// The stage is `will-change: transform` on both sides, on purpose: a layer with it never draws sub-pixel (LCD) text, so every piece is
// drawn with grey anti-aliasing here and in the design's page. Without it the compositor decides per page, and on a machine whose font
// configuration asks for LCD text (the runner of the visual-baselines job) it decided differently for the app's stage and the design's:
// the same piece was LCD in one and grey in the other and the stems of "High" differed in 7 of 1134 pixels (stage.spec.ts). It is not a
// tolerance and no region is skipped: the pieces are compared as before, only the way their text is anti-aliased no longer depends on the machine.

export type Boxes = Record<string, { x: number; y: number; width: number; height: number }>;

/** In the design's frame: the chips of its rows (the words and the meter), the severity badges and the unknown-author badge. */
export const designPieces_: DesignPick = (frame) => {
  const text = (el: Element): string => (el.textContent ?? "").replace(/\s+/g, " ").trim();
  const stage = document.createElement("div");
  // Through the style object, not the `style` attribute: the app's CSP refuses an inline style attribute, and the test would be the violation.
  Object.assign(stage.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", display: "flex", flexDirection: "column", alignItems: "flex-start", gap: "12px", padding: "0", width: "max-content", background: "var(--background)", color: "var(--foreground)", willChange: "transform" });
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
export const appStage = (within: string) => (page: Page): Promise<Boxes> =>
  page.evaluate((scope) => {
    const LEVEL: Record<string, string> = { certain: "Certain", high: "High", medium: "Medium", low: "Low", none: "Unknown" };
    const text = (el: Element): string => (el.textContent ?? "").replace(/\s+/g, " ").trim();
    const root = document.querySelector(scope) ?? document.body;
    const stage = document.createElement("div");
    // Through the style object, not the `style` attribute: the app's CSP refuses an inline style attribute, and the test would be the violation.
  Object.assign(stage.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", display: "flex", flexDirection: "column", alignItems: "flex-start", gap: "12px", padding: "0", width: "max-content", background: "var(--background)", color: "var(--foreground)", willChange: "transform" });
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
