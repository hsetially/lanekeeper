// The states of the masking pieces of T5 (slice C): a value the server hid, drawn as a token ("Hidden value", with its length) and, for an
// Editor, a Reveal button; the Reveal dialog (a required reason, and every answer the hub gives: 429, 503, 404); the value once it is
// shown (a Hide button, "Revealed · recorded in the audit log", one at a time); a line or a whole text the browser will not draw because
// the hub's masks cannot be trusted (fail closed); and the file whose content the server withholds (denied, or not checkable).
import { expect } from "@playwright/test";
import type { Case } from "./helpers.ts";
import { modal } from "./helpers.ts";
import { CORE_FILE, openReveal, patchJson, reveal, sendReveal, withAdjacentSecrets } from "./t5.ts";

const CONTENT = /\/api\/v1\/swimlanes\/sitb\/files\/content/;

/** The masks of the NFS text, changed: what a hub that is wrong (or an attacker in the middle) could send. */
const masks = (change: (mask: Record<string, unknown>) => Record<string, unknown>) => async ({ page }: { page: import("@playwright/test").Page }): Promise<void> => {
  await patchJson(page, CONTENT, (content, url) => (url.searchParams.get("source") === "nfs" && Array.isArray(content["masks"]) ? { ...content, masks: (content["masks"] as Array<Record<string, unknown>>).map(change) } : content));
};

export const t5MaskingCases: Case[] = [
  // ---- The token and the Reveal control -----------------------------------------------------------------------------------------------------
  {
    id: "masking / a hidden value with its Reveal button (an Editor)",
    scenario: "design-masking-reveal",
    url: CORE_FILE,
    async open({ page }) {
      await expect(page.locator("[data-slot='hidden-value']").first()).toBeVisible();
      await expect(page.getByRole("button", { name: /^Reveal value of / })).toBeVisible();
    },
  },
  {
    id: "masking / a viewer: Reveal is off and says why (focused)",
    scenario: "design-masking-viewer",
    url: CORE_FILE,
    async open({ page }) {
      const button = page.getByRole("button", { name: /^Reveal value of / });
      await expect(button).toBeVisible();
      await button.focus();
      await page.waitForTimeout(500);
    },
  },
  {
    id: "masking / three kinds of mask: revealable, no setting, recognised by form",
    scenario: "design-masking-multi",
    url: CORE_FILE,
    async open({ page }) {
      await expect(page.locator("[data-slot='hidden-value']")).toHaveCount(3);
    },
  },

  {
    // Secrets sit side by side in a file (an id, a secret, a token): each has a Reveal of its own, in rows of 18 px.
    id: "masking / three revealable values on consecutive lines",
    scenario: "design-masking-reveal",
    url: `${CORE_FILE}&line=20`,
    async before({ page }) {
      await patchJson(page, CONTENT, (content, url) => (url.searchParams.get("source") === "nfs" ? withAdjacentSecrets(content) : content));
    },
    async open({ page }) {
      await expect(page.getByRole("button", { name: /^Reveal value of / })).toHaveCount(3);
    },
  },

  // ---- The Reveal dialog ---------------------------------------------------------------------------------------------------------------
  { id: "masking / the Reveal dialog", scenario: "design-masking-reveal", url: CORE_FILE, open: ({ page }) => openReveal(page) },
  {
    id: "masking / the Reveal dialog: no reason typed",
    scenario: "design-masking-reveal",
    url: CORE_FILE,
    async open({ page }) {
      await openReveal(page);
      await modal(page).getByRole("button", { name: "Reveal value", exact: true }).click();
      await expect(modal(page).getByText("Enter a reason")).toBeVisible();
    },
  },
  {
    id: "masking / the Reveal dialog: over the limit (429)",
    scenario: "design-masking-throttled",
    url: CORE_FILE,
    async open({ page }) {
      await openReveal(page);
      await sendReveal(page);
      await expect(modal(page).getByRole("alert").or(modal(page).getByText(/too many|wait/i)).first()).toBeVisible();
    },
  },
  {
    id: "masking / the Reveal dialog: the audit log is down (503)",
    scenario: "design-masking-unavailable",
    url: CORE_FILE,
    async open({ page }) {
      await openReveal(page);
      await sendReveal(page);
      await expect(modal(page).getByRole("alert").or(modal(page).getByText(/unavailable|audit/i)).first()).toBeVisible();
    },
  },
  {
    id: "masking / the Reveal dialog: nothing to reveal any more (404)",
    scenario: "design-masking-gone",
    url: CORE_FILE,
    async open({ page }) {
      await openReveal(page);
      await sendReveal(page);
      await expect(modal(page).getByRole("alert").or(modal(page).getByText(/no longer|not found|gone/i)).first()).toBeVisible();
    },
  },

  // ---- The revealed value ----------------------------------------------------------------------------------------------------------------
  {
    id: "masking / a value revealed (Hide has focus, the announcement is polite)",
    scenario: "design-masking-reveal",
    url: CORE_FILE,
    async open({ page }) {
      await reveal(page);
      await expect(page.getByText("Revealed · recorded in the audit log")).toBeVisible();
      await expect(page.getByRole("button", { name: /^Hide value of / })).toBeFocused();
    },
  },
  {
    id: "masking / a second value revealed: the first is hidden again (one at a time)",
    scenario: "design-masking-multi",
    url: CORE_FILE,
    async open({ page }) {
      await reveal(page, 0);
      await expect(page.locator("[data-slot='revealed-value']")).toHaveCount(1);
      await reveal(page, 0);
      await expect(page.locator("[data-slot='revealed-value']")).toHaveCount(1);
    },
  },

  {
    // The value is drawn in a row of one line: past 2,000 characters it is clipped, and the row says so.
    id: "masking / a revealed value of 2,500 characters is clipped and says so",
    scenario: "design-masking-reveal",
    url: CORE_FILE,
    async before({ page }) {
      await page.route(/\/api\/v1\/swimlanes\/sitb\/files\/reveal$/, (route) => route.fulfill({ headers: { "cache-control": "no-store" }, json: { value: "k9".repeat(1_250) } }));
    },
    async open({ page }) {
      await reveal(page);
      await expect(page.getByText(/first 2,000 characters/)).toBeVisible();
    },
  },

  // ---- A hub that is wrong: the browser fails closed ------------------------------------------------------------------------------------------
  {
    // A mask that is not exactly a marker: the whole line is hidden, none of it is drawn.
    id: "masking / a mask that is not a marker hides its line",
    scenario: "design-masking-reveal",
    url: CORE_FILE,
    async before(ctx) {
      await masks((m) => ({ ...m, start: Number(m["start"]) + 1 }))(ctx);
    },
    async open({ page }) {
      await expect(page.locator("[data-slot='hidden-line']").first()).toBeVisible();
    },
  },
  {
    // A mask that names a line the text does not have: the text cannot be placed, so none of it is shown.
    id: "masking / a mask that names a line the text does not have refuses the text",
    scenario: "design-masking-reveal",
    url: CORE_FILE,
    async before(ctx) {
      await masks((m) => ({ ...m, line: 9_999 }))(ctx);
    },
    async open({ page }) {
      await expect(page.getByText("This text can't be shown safely")).toBeVisible();
    },
  },

  // ---- Content the server withholds ---------------------------------------------------------------------------------------------------------
  {
    id: "masking / withheld: the hub could not check the file for secrets",
    scenario: "design-file-core",
    url: CORE_FILE,
    async before({ page }) {
      await patchJson(page, CONTENT, (content) => ({ ...content, text: null, masks: undefined, content_withheld: true }));
    },
    async open({ page }) {
      await expect(page.getByText("Content withheld: could not be checked for secrets")).toBeVisible();
    },
  },
];
