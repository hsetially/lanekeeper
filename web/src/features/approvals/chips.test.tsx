import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { TriangleAlert } from "@/icons";
import { Chip } from "./chips";

// A chip is a word in a small box. Where the room is short (a table column, a drawer, labels 40% longer) the word ends in an ellipsis
// inside the box, and the box does not grow past what holds it: the layout run found "File changed since proposed" and "unknown author"
// cut off by their cells.
describe("a chip", () => {
  it("keeps its word in a span that ends in an ellipsis, inside a box that is never wider than its container", () => {
    render(
      <Chip tone="stale" Icon={TriangleAlert}>
        File changed since proposed
      </Chip>,
    );
    const word = screen.getByText("File changed since proposed");
    expect(word.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(word.className).toMatch(/(^|\s)truncate(\s|$)/);
    const box = word.closest("[data-slot='tone-chip']");
    expect(box?.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    expect(box?.getAttribute("data-tone")).toBe("stale");
  });
});
