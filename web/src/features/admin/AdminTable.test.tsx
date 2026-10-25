import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AdminTable, tableHeight } from "./AdminTable";

// An Admin list is as tall as its rows, up to a limit, so five rules are not a tall empty frame and hundreds of people scroll in a box.

describe("tableHeight", () => {
  it("is the header and the rows, with a little for the border", () => {
    expect(tableHeight(0, false)).toBe(34);
    expect(tableHeight(5, false)).toBe(32 + 5 * 32 + 2);
  });

  it("adds the footer when there is one", () => {
    expect(tableHeight(5, true)).toBe(32 + 5 * 32 + 36 + 2);
  });

  it("stops growing at 480 px, so a long list scrolls inside its box", () => {
    expect(tableHeight(100, false)).toBe(480);
    expect(tableHeight(100, true)).toBe(480);
  });
});

describe("AdminTable", () => {
  it("draws the grid in a box of that height", () => {
    const { container } = render(<AdminTable label="Things" rows={["a", "b"]} columns={[{ id: "x", header: "X", width: "1fr", cell: (row) => row }]} rowKey={(row) => row} />);
    expect(screen.getByRole("grid", { name: "Things" })).toBeInTheDocument();
    expect((container.firstElementChild as HTMLElement).style.height).toBe(`${tableHeight(2, false)}px`);
  });
});
