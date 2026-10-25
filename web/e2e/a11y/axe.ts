// axe-core in the real browser, for the accessibility specs of T4 (design/13 "Accessibility"; plan, "Keyboard, accessibility,
// time and browsers").
//
// Why this is not `@axe-core/playwright`: the plan lists that package, but it is not installed and package.json is a shared
// file. The package does exactly this (it hands `axe.source` to the page and calls `axe.run`), so this helper does the same
// with `axe-core@4.13.0`, which IS a dependency. `page.evaluate(string)` goes through the DevTools protocol, which the page's
// Content-Security-Policy does not govern, so the production policy is never relaxed to run it (no `bypassCSP`, no inline
// <script>, no `unsafe-eval`).
//
// What is checked, and how strictly:
//   - the tags wcag2a, wcag2aa, wcag21a, wcag21aa, wcag22aa and best-practice (best-practice carries `region`, the landmark
//     rule), and the rules `color-contrast` and `target-size` by name, so that a rule the tags happen to leave out cannot go
//     missing without the spec saying so (`rulesRan` is asserted);
//   - ZERO violations;
//   - ZERO "incomplete" (axe could not decide), except the ones listed in EXPLAINED_INCOMPLETE below, each with its reason.
import axe from "axe-core";
import { appendFileSync } from "node:fs";
import { expect, type Page } from "@playwright/test";
import { resolveContrast, resolveControls } from "./resolve.ts";

export const AXE_TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"] as const;
/** Rules that must have RUN (not been inapplicable-by-omission) on every page: the ones the plan names. */
export const REQUIRED_RULES = ["color-contrast", "target-size", "region", "page-has-heading-one", "landmark-one-main", "heading-order"] as const;

export type AxeNode = { target: unknown[]; html: string; failureSummary?: string | undefined; reason?: string | undefined };
export type AxeRule = { id: string; impact?: string | null | undefined; help: string; helpUrl: string; nodes: AxeNode[] };
export type AxeOutcome = { violations: AxeRule[]; incomplete: AxeRule[]; ranIds: string[]; passes: number };

/**
 * "Incomplete" results that are understood and accepted. Each entry names the rule, a selector fragment that must appear in the
 * node's target and the reason axe cannot decide. An entry that matches nothing is not an error (the screen may not have the
 * element), but an incomplete that matches no entry fails the spec.
 *
 * The list is empty on purpose: every incomplete that turned up in the first runs was a real finding and was fixed (or is a
 * defect listed in the hand-back). Add an entry here only with a reason a reviewer can check.
 */
export const EXPLAINED_INCOMPLETE: ReadonlyArray<{ rule: string; target: RegExp; why: string }> = [];

/** Wait until nothing animates (reduced motion is pinned, so this is short), then one frame more. */
export async function settle(page: Page, ms = 150): Promise<void> {
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  await page
    .waitForFunction(() => document.getAnimations().every((a) => a.effect?.getTiming().iterations === Infinity || a.playState !== "running"), undefined, { timeout: 3000 })
    .catch(() => undefined);
  await page.waitForTimeout(ms);
}

type Scope = { include?: string[]; exclude?: string[] };

/** Run axe on the page as it is now. */
export async function runAxe(page: Page, scope: Scope = {}): Promise<AxeOutcome> {
  // The source is a plain script that defines `window.axe`; the DevTools evaluate is not subject to the page's CSP.
  const loaded = await page.evaluate("typeof window.axe === 'object' && typeof window.axe.run === 'function'");
  if (loaded !== true) await page.evaluate(axe.source);
  const result = await page.evaluate(
    async ({ tags, rules, scope: s }) => {
      type Check = { message?: string; data?: { messageKey?: string } };
      const w = window as unknown as { axe: { run: (ctx: unknown, opts: unknown) => Promise<Record<string, Array<{ id: string; impact?: string | null; help: string; helpUrl: string; nodes: Array<{ target: unknown[]; html: string; failureSummary?: string; any?: Check[]; all?: Check[]; none?: Check[] }> }>>> } };
      const context = s.include || s.exclude ? { ...(s.include ? { include: s.include.map((x) => [x]) } : {}), ...(s.exclude ? { exclude: s.exclude.map((x) => [x]) } : {}) } : document;
      const out = await w.axe.run(context, {
        runOnly: { type: "tag", values: tags },
        rules: Object.fromEntries(rules.map((id) => [id, { enabled: true }])),
        resultTypes: ["violations", "incomplete", "passes", "inapplicable"],
      });
      const slim = (list: typeof out.violations) => (list ?? []).map((r) => ({ id: r.id, impact: r.impact ?? null, help: r.help, helpUrl: r.helpUrl, nodes: r.nodes.map((n) => ({ target: n.target, html: n.html.slice(0, 300), failureSummary: n.failureSummary?.slice(0, 400), reason: [...(n.any ?? []), ...(n.all ?? []), ...(n.none ?? [])].map((c) => `${c.data?.messageKey ?? ""} ${c.message ?? ""}`.trim()).join(" | ").slice(0, 240) })) }));
      return {
        violations: slim(out["violations"] ?? []),
        incomplete: slim(out["incomplete"] ?? []),
        ranIds: [...(out["violations"] ?? []), ...(out["incomplete"] ?? []), ...(out["passes"] ?? []), ...(out["inapplicable"] ?? [])].map((r) => r.id),
        passes: (out["passes"] ?? []).length,
      };
    },
    { tags: [...AXE_TAGS], rules: ["color-contrast", "target-size"], scope },
  );
  return result;
}

function describe(rules: AxeRule[]): string {
  return rules
    .map((r) => `  ${r.id} [${r.impact ?? "?"}] ${r.help}\n${r.nodes
      .slice(0, 6)
      .map((n) => `      ${JSON.stringify(n.target)}  ${n.html.replace(/\s+/g, " ").slice(0, 160)}\n        ${(n.reason ?? n.failureSummary ?? "").replace(/\s+/g, " ").slice(0, 260)}`)
      .join("\n")}${r.nodes.length > 6 ? `\n      ... ${r.nodes.length - 6} more` : ""}`)
    .join("\n");
}

/**
 * With a modal dialog open, axe cannot say whether the page behind it (aria-hidden, with its controls still in the DOM) can be tabbed to:
 * `aria-hidden-focus` is "incomplete: check that the focusable elements are not tabbable in the current state" (rule `focusable-modal-open`).
 * This answers it with the real keyboard: Tab and Shift+Tab from where focus is, a dozen times each way, and focus has to stay inside the
 * dialog every time. When it does, those incompletes are explained; when it does not, they stay and the spec fails with the stop that escaped.
 */
export async function modalHoldsFocus(page: Page): Promise<{ ok: boolean; detail: string }> {
  const open = await page.evaluate(() => [...document.querySelectorAll("[role='dialog'], [role='alertdialog']")].some((d) => d.getClientRects().length > 0 && getComputedStyle(d).visibility !== "hidden"));
  if (!open) return { ok: false, detail: "no dialog is open" };
  for (const key of ["Tab", "Shift+Tab"]) {
    for (let i = 0; i < 14; i++) {
      await page.keyboard.press(key);
      const where = await page.evaluate(() => {
        const dialogs = [...document.querySelectorAll("[role='dialog'], [role='alertdialog']")].filter((d) => d.getClientRects().length > 0);
        const active = document.activeElement;
        if (active === null) return "nothing has focus";
        if (dialogs.some((d) => d.contains(active))) return null;
        return `${active.tagName.toLowerCase()} "${(active.getAttribute("aria-label") ?? active.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 30)}"`;
      });
      if (where !== null) return { ok: false, detail: `${key} #${i + 1} moved focus out of the dialog, to ${where}` };
    }
  }
  return { ok: true, detail: "28 key presses, focus stayed in the dialog" };
}

/** The incomplete results that are not explained above. */
export function unexplained(outcome: AxeOutcome): AxeRule[] {
  return outcome.incomplete
    .map((rule) => ({ ...rule, nodes: rule.nodes.filter((n) => !EXPLAINED_INCOMPLETE.some((e) => e.rule === rule.id && e.target.test(JSON.stringify(n.target)))) }))
    .filter((rule) => rule.nodes.length > 0);
}

/** What the spec itself decided where axe could not (see resolve.ts), for the summary of a run. */
export const TALLY = { contrastByAxe: 0, contrastByRatio: 0, modalFocus: 0, controls: 0 };

/** Axe on the page as it is: no violation, no unexplained incomplete, and the plan's rules did run. */
export async function expectAxeClean(page: Page, label: string, scope: Scope = {}): Promise<AxeOutcome> {
  await settle(page);
  const outcome = await runAxe(page, scope);
  for (const id of REQUIRED_RULES) {
    // A scoped run (a dialog only) leaves page-level rules inapplicable; they still appear in the list of rules axe knows.
    expect(outcome.ranIds, `${label}: axe did not know the rule ${id}`).toContain(id);
  }
  let rest = unexplained(outcome);
  const violations = [...outcome.violations];
  if (rest.some((r) => r.id === "aria-hidden-focus")) {
    const held = await modalHoldsFocus(page);
    if (held.ok) {
      rest = rest.filter((r) => r.id !== "aria-hidden-focus");
      TALLY.modalFocus += 1;
    } else expect.soft(held.ok, `${label}: a modal is open and axe cannot tell whether the page behind it can be tabbed to; the keyboard says: ${held.detail}`).toBe(true);
  }
  const popup = rest.find((r) => r.id === "aria-valid-attr-value");
  if (popup) {
    const controls = await resolveControls(page, popup.nodes.filter((n) => (n.reason ?? "").startsWith("controlsWithinPopup")));
    TALLY.controls += controls.resolved;
    expect.soft(controls.failed, `${label}: aria-controls on a popup trigger`).toEqual([]);
    rest = rest.map((r) => (r.id === "aria-valid-attr-value" ? { ...r, nodes: r.nodes.filter((n) => !(n.reason ?? "").startsWith("controlsWithinPopup")) } : r)).filter((r) => r.nodes.length > 0);
  }
  const contrast = rest.find((r) => r.id === "color-contrast");
  if (contrast) {
    const resolution = await resolveContrast(page, contrast.nodes);
    TALLY.contrastByAxe += resolution.resolvedByAxe;
    TALLY.contrastByRatio += resolution.resolvedByRatio;
    if (resolution.violations.length > 0) violations.push({ id: "color-contrast", impact: "serious", help: "contrast computed by the spec (axe could not decide)", helpUrl: "", nodes: resolution.violations.map((n) => ({ ...n, reason: `ratio ${n.ratio} needed ${n.needed}: ${n.reason ?? ""}` })) });
    rest = rest.map((r) => (r.id === "color-contrast" ? { ...r, nodes: resolution.remaining } : r)).filter((r) => r.nodes.length > 0);
  }
  // LK_A11Y_OUT=<file> keeps every finding as one JSON line, for a summary across all screens (a development aid, not a gate).
  const out = process.env["LK_A11Y_OUT"];
  if (out) {
    const lines = [...violations.map((r) => ["violation", r] as const), ...rest.map((r) => ["incomplete", r] as const)].flatMap(([kind, r]) =>
      r.nodes.map((n) => JSON.stringify({ label, kind, rule: r.id, target: n.target, reason: n.reason, html: n.html.slice(0, 160) })),
    );
    if (lines.length > 0) appendFileSync(out, `${lines.join("\n")}\n`);
  }
  // Soft: a screen that fails at one width is still checked at the next one, and the report lists everything it found.
  expect.soft(violations.length, `${label}: axe violations\n${describe(violations)}`).toBe(0);
  expect.soft(rest.length, `${label}: axe incomplete (cannot decide)\n${describe(rest)}`).toBe(0);
  return outcome;
}
