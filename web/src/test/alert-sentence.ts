import { expect } from "vitest";

/**
 * A sentence that holds an element in the middle of it (a swimlane's name in `<code>`, a link) is ONE block inside an `Alert`.
 *
 * `Alert` lays the children of its content out as a column (`flex flex-col`: a title above a description). A sentence passed as bare
 * children, "Other people testing in <SafeText/> will be interrupted.", is then three items, a text, a span and a text, and the browser
 * draws three lines. jsdom does no layout, so a test that reads the text of the sentence passes either way; this reads the structure: the
 * column that holds the sentence holds nothing else beside it. `sentence` is the element a `getByText` found for the sentence's own words.
 */
export function expectOneBlockInAlert(sentence: HTMLElement): void {
  const alert = sentence.closest("[data-slot='alert']");
  expect(alert, "the sentence is in an Alert").not.toBeNull();
  const column = alert?.lastElementChild;
  expect(column, "the Alert's content column").toBeTruthy();
  expect(column?.childNodes.length, "the Alert's column holds the sentence as one block, not as a text, an element and a text side by side").toBe(1);
}
