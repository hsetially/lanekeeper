// What the tests of the Findings screen read from the table. Test code only: nothing outside the tests imports it.
import { screen, within } from "@testing-library/react";

/** The data rows of the findings table (the header row left out). */
export function tableRows(): HTMLElement[] {
  const grid = screen.getByRole("grid", { name: "Findings" });
  // The skeleton is a grid of rows too: while it is on screen there are no findings.
  if (grid.getAttribute("aria-busy") === "true") return [];
  return within(grid)
    .getAllByRole("row")
    .filter((row) => within(row).queryAllByRole("columnheader").length === 0);
}

/** The cells of a row, trimmed. */
export const cellsOf = (row: HTMLElement): string[] => within(row).getAllByRole("gridcell").map((cell) => (cell.textContent ?? "").trim());
