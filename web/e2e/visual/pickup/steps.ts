// What the frames of the T6 matrix do to a page that more than one of them does: where the screens are, a service's row, a folder of the
// tree, the open tooltip, and a reading of what a page does with text it must not trust.
import type { Locator, Page } from "@playwright/test";
import { expect } from "../../fixtures/test";

export const SERVICES = "/swimlanes/sitb/services";
export const OVERVIEW = "/swimlanes";
const CORE = "tx-infinity-api%2Ftx-infinity-core-sit7.yml";
/** The File page's core tenant file, in the File page's own scenarios. */
export const FILE = `/swimlanes/sitb/files?path=${CORE}`;
/** The folder of that file, which the tree read for the head of the file asks for. */
export const TREE_PREFIX = "tx-infinity-api";

/** The row of a service on the Services tab, by the name in its first cell. */
export const rowOf = (page: Page, service: string): Locator => page.getByRole("row").filter({ has: page.getByRole("cell", { name: service, exact: true }) });

/** The open tooltip (the one Radix draws twice: the visible content and its hidden twin, so the first). */
export const tooltip = (page: Page): Locator => page.getByRole("tooltip").first();

/** Open a folder of the file tree and wait until its files are in it. */
export async function openFolder(page: Page, folder: string): Promise<void> {
  const button = page.getByRole("button", { name: `Open ${folder}` });
  await button.click();
  await expect(page.getByRole("button", { name: `Close ${folder}` })).toBeVisible();
  await expect(page.getByRole("tree").getByRole("treeitem").nth(2)).toBeVisible();
}

// The characters that make one name look like another: bidirectional controls and zero-width characters (S8).
const CONTROLS = /[\u061C\u200B-\u200F\u202A-\u202E\u2060\u2066-\u2069\uFEFF]/;

/**
 * What the page does with text it must not trust: the controls that are still in the text of the page and in a title attribute as themselves
 * (`raw`, which must be none), and whether the markers that stand for them are there (`marked`: an override and a zero-width space).
 */
export async function hostileMarkers(page: Page): Promise<{ raw: string[]; marked: boolean }> {
  const reading = await page.evaluate(() => {
    const titles = Array.from(document.querySelectorAll("[title]")).map((el) => el.getAttribute("title") ?? "");
    return { text: document.body.innerText, titles };
  });
  const everything = [reading.text, ...reading.titles];
  const raw = everything.flatMap((text) => Array.from(text).filter((ch) => CONTROLS.test(ch)).map((ch) => `U+${ch.codePointAt(0)?.toString(16).toUpperCase().padStart(4, "0") ?? "?"}`));
  const joined = everything.join("\n");
  return { raw, marked: joined.includes("<U+202E>") && joined.includes("<U+200B>") };
}
