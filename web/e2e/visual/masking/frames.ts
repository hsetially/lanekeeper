// The frames of masking (T5, slice C): the masked value on the File page and its Reveal (board "File View", frames 1d, 1e and 1f), the
// answers the hub can give a reveal, a file with several masks, a mask the page cannot trust, and a file whose content the server withholds
// (frame 1n).
//
// None of them is a pixel comparison with the design. The design draws a masked value as five dots, a Reveal dialog with its own words and a
// revealed value in a code line; the app draws a token of words ("Hidden value", the length), a dialog that names what the reveal records and
// how long the value stays, and a revealed value that hides again by itself (design/DEVIATIONS.md: "File: masked value", "File: Reveal dialog",
// "File: revealed value"). frames.ts compares nothing; e2e/visual/file/view.spec.ts (T4) compares the chrome of 1d against the design, and the
// revealed line of 1f is compared with the design's in masking.spec.ts. What is here is the app against its own baseline, which CI generates.
import { expect } from "../../fixtures/test";
import type { Page } from "@playwright/test";
import { api, rewrite, signedInAs } from "../access/lib/steps";
import type { Env, FrameDef, Step } from "../search/kit";

const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";
const FILE = `/swimlanes/sitb/files?path=${CORE}`;
export const REASON = "Teller login fails on sit7; checking the secret matches the vault entry (OPS-2291).";

const MASK_WHY = "the masked value is a token, with a Reveal button named for its setting (design/DEVIATIONS.md: \"File: masked value\")";
const DIALOG_WHY = "the dialog says what a reveal records and how long the value stays, and asks for a reason (design/DEVIATIONS.md: \"File: Reveal dialog\")";
const SHOWN_WHY = "the revealed value hides again by itself and is never copied (design/DEVIATIONS.md: \"File: revealed value\")";
const FAIL_CLOSED_WHY = "a mask the page cannot check hides its line, and one it cannot place refuses the text (design/DEVIATIONS.md: \"File: masks the hub gets wrong\"): the design has no such state";
const WITHHELD_WHY = "the card for a file whose content is withheld (design/DEVIATIONS.md: \"File: content withheld\")";

// ----- steps -----------------------------------------------------------------------------------------------------------------------

/** The design's Marco Diaz, an Editor, as in frames 1d to 1f. */
const asEditor: Step = async ({ page }) => {
  await signedInAs(page, "editor");
};

const revealButton = (page: Page) => page.getByRole("button", { name: /^Reveal value of / });

const openDialog: Step = async ({ page }) => {
  await revealButton(page).first().click();
  await page.getByRole("alertdialog", { name: "Reveal masked value?" }).waitFor();
};
const typeReason: Step = async ({ page }) => {
  await page.getByRole("alertdialog").getByRole("textbox").fill(REASON);
};
const submit: Step = async ({ page }) => {
  await page.getByRole("alertdialog").getByRole("button", { name: "Reveal value" }).click();
};
const then = (...steps: Step[]): Step => async (env: Env) => {
  for (const step of steps) await step(env);
};
const reveal = then(openDialog, typeReason, submit, async ({ page }) => {
  await page.getByRole("button", { name: "Hide" }).waitFor();
});

/** What the dialog says after the hub refused: the words are the reason, and nothing was shown. */
const refused = (words: RegExp): Step => then(openDialog, typeReason, submit, async ({ page }) => {
  await expect(page.getByRole("alertdialog")).toContainText(words);
});

/** The hub's content for the file, with its masks changed (what a hub that gets a mask wrong would send). */
const masksChanged = (change: (mask: Record<string, unknown>) => Record<string, unknown>): Step => async ({ page }) => {
  await rewrite(page, api("/swimlanes/sitb/files/content"), (body) => ({ ...body, masks: (body["masks"] as Array<Record<string, unknown>> | undefined)?.map(change) }));
};

/** The same file, with its content withheld: a class the hub does not deny, so the page says it could not be checked. */
const unchecked: Step = async ({ page }) => {
  await rewrite(page, api("/swimlanes/sitb/files/content"), (body) => ({ ...body, text: null, masks: undefined, content_withheld: true, class: "structured" }));
  await rewrite(page, api("/swimlanes/sitb/tree"), (body) => ({
    ...body,
    items: (body["items"] as Array<Record<string, unknown>>).map((entry) => (entry["path"] === "tx-infinity-api/tx-infinity-core-sit7.yml" ? { ...entry, content_withheld: true } : entry)),
  }));
};

// ----- frames ----------------------------------------------------------------------------------------------------------------------

const frame = (code: string, def: Omit<FrameDef, "board" | "frame" | "kind">): FrameDef => ({ board: "states", frame: code, kind: "app", ...def });

export const frames: FrameDef[] = [
  // Frames 1d, 1e, 1f: a masked value, the dialog, the value shown; light and dark.
  frame("C01", { scenario: "design-masking-reveal", path: FILE, title: "File 1d: a masked value with Reveal, for an Editor", why: MASK_WHY, before: asEditor }),
  frame("C02", { scenario: "design-masking-reveal", path: FILE, theme: "dark", title: "File 1d, dark", why: MASK_WHY, before: asEditor }),
  frame("C03", { scenario: "design-masking-reveal", path: FILE, title: "File 1e: Reveal asks for a reason and says what is recorded", why: DIALOG_WHY, before: asEditor, steps: then(openDialog, typeReason) }),
  frame("C04", { scenario: "design-masking-reveal", path: FILE, theme: "dark", title: "File 1e, dark", why: DIALOG_WHY, before: asEditor, steps: then(openDialog, typeReason) }),
  frame("C05", { scenario: "design-masking-reveal", path: FILE, theme: "dark", title: "File 1f: the value is shown, with Hide and the note about the audit log", why: SHOWN_WHY, before: asEditor, steps: reveal }),
  frame("C06", { scenario: "design-masking-reveal", path: FILE, title: "File 1f, light", why: SHOWN_WHY, before: asEditor, steps: reveal }),

  // The dialog's other states.
  frame("C07", { scenario: "design-masking-reveal", path: FILE, title: "dialog: no reason, so it asks for one", why: DIALOG_WHY, before: asEditor, steps: then(openDialog, submit, async ({ page }) => {
    await page.getByRole("alertdialog").getByText("Enter a reason").waitFor();
  }) }),
  frame("C08", { scenario: "design-masking-throttled", path: FILE, before: asEditor, title: "dialog: too many reveals (429), with how long to wait", why: DIALOG_WHY, steps: refused(/wait|too many|30/i) }),
  frame("C09", { scenario: "design-masking-unavailable", path: FILE, before: asEditor, title: "dialog: the audit log is down (503), nothing is shown", why: DIALOG_WHY, steps: refused(/audit|unavailable|try again/i) }),
  frame("C10", { scenario: "design-masking-gone", path: FILE, before: asEditor, title: "dialog: the value is no longer hidden or the setting is gone (404)", why: DIALOG_WHY, steps: refused(/no longer|not found|gone|nothing to reveal/i) }),
  frame("C11", { scenario: "design-masking-throttled", path: FILE, theme: "dark", before: asEditor, title: "dialog: too many reveals, dark", why: DIALOG_WHY, steps: refused(/wait|too many|30/i) }),

  // Who may reveal.
  frame("C12", { scenario: "design-masking-viewer", path: FILE, title: "a Viewer: Reveal is disabled and says it needs an Editor", why: MASK_WHY, steps: async ({ page }) => {
    await revealButton(page).first().hover();
    await page.getByRole("tooltip").first().waitFor();
  } }),
  frame("C13", { scenario: "design-masking-viewer", path: FILE, theme: "dark", title: "a Viewer, dark", why: MASK_WHY }),

  // A file with several masks.
  frame("C14", { scenario: "design-masking-multi", path: FILE, before: asEditor, title: "three masks: one can be revealed, one names no setting, one is known by its form", why: MASK_WHY }),
  frame("C15", { scenario: "design-masking-multi", path: FILE, theme: "dark", before: asEditor, title: "three masks, dark", why: MASK_WHY }),
  frame("C16", { scenario: "design-masking-multi", path: FILE, before: asEditor, title: "three masks: one value shown, the others stay hidden", why: SHOWN_WHY, steps: reveal }),

  // A mask the page cannot trust.
  frame("C17", { scenario: "design-masking-reveal", path: FILE, before: then(asEditor, masksChanged((mask) => ({ ...mask, start: Number(mask["start"]) + 1 }))), title: "a mask that is not on a marker: its line is hidden", why: FAIL_CLOSED_WHY }),
  frame("C18", { scenario: "design-masking-reveal", path: FILE, before: then(asEditor, masksChanged((mask) => ({ ...mask, line: 99_999 }))), title: "a mask on a line the text does not have: the text is refused", why: FAIL_CLOSED_WHY }),
  frame("C19", { scenario: "design-masking-reveal", path: FILE, theme: "dark", before: then(asEditor, masksChanged((mask) => ({ ...mask, line: 99_999 }))), title: "the text is refused, dark", why: FAIL_CLOSED_WHY }),

  // A file whose content is withheld (frame 1n).
  frame("C20", { scenario: "design-file-denied", path: "/swimlanes/sitb/files?path=security%2Fkeystore.jks", title: "File 1n: content withheld for a keystore, name, size and hash only", why: WITHHELD_WHY }),
  frame("C21", { scenario: "design-file-denied", path: "/swimlanes/sitb/files?path=security%2Fkeystore.jks", theme: "dark", title: "File 1n, dark", why: WITHHELD_WHY }),
  frame("C22", { scenario: "design-masking-reveal", path: FILE, before: unchecked, title: "content withheld because the hub could not check the file for secrets", why: WITHHELD_WHY }),
];
