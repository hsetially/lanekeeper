import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { GridCell, GridRow } from "@/api/generated/model";
import { PathCell, ValueCell } from "./cells";

const cell = (text: string | null, flag: "yaml_boolean_like" | null = null): GridCell =>
  text === null ? { value: null, location: null } : { value: { text, ty: "string", flag }, location: { path: "a", lines: { start: 3, end: 3 } } };
const row: GridRow = { path: "limits.max", cells: [cell("5000")] };

function show(over: Partial<Parameters<typeof ValueCell>[0]> = {}) {
  const onPick = vi.fn();
  render(<ValueCell row={row} lane={0} cell={cell("5000")} differs={false} muted={false} picked={false} offline={false} onPick={onPick} {...over} />);
  return { onPick };
}

describe("a value cell", () => {
  it("shows the value", () => {
    show();
    expect(screen.getByRole("button", { name: "5000" })).toBeInTheDocument();
  });

  it("is a 24px pointer target (WCAG 2.2 target size): a cell is picked by a click, and 20px with 12px around it is under the minimum", () => {
    show({});
    const button = screen.getByRole("button", { name: "5000" });
    expect(button.className).toMatch(/(^|\s)h-6(\s|$)/);
    expect(button.className).not.toMatch(/(^|\s)h-5(\s|$)/);
  });

  it("says an empty value is empty and an absent key is absent, in words and not only in shade", () => {
    show({ cell: cell("") });
    expect(screen.getByRole("button", { name: /empty/ })).toHaveTextContent('""');
  });

  it("says absent for a key that is not in the file", () => {
    show({ cell: cell(null) });
    expect(screen.getByRole("button", { name: "absent" })).toBeInTheDocument();
  });

  it("shows a hidden value as hidden, with its length, and never as anything that looks like a value", () => {
    show({ cell: cell("[REDACTED: 12 chars, aws_key]") });
    const button = screen.getByRole("button", { name: /Hidden value, 12 characters/ });
    expect(button).toHaveTextContent("Hidden");
    expect(button).toHaveAttribute("data-kind", "hidden");
    expect(button).not.toHaveAttribute("data-differs");
  });

  it("marks a cell that stands apart from the row by a flag and a label, not by colour alone", () => {
    show({ differs: true });
    const button = screen.getByRole("button", { name: /5000/ });
    expect(button).toHaveAttribute("data-differs", "true");
    expect(button).toHaveTextContent("5000");
  });

  it("shows the invisible and right-to-left characters of a value as visible marks", () => {
    show({ cell: cell("a‮b") });
    expect(screen.getByRole("img", { name: /U\+202E/ })).toBeInTheDocument();
  });

  it("says that yes is not true", () => {
    show({ cell: cell("yes", "yaml_boolean_like") });
    expect(screen.getByRole("button", { name: /yes.*not equal to true/i })).toBeInTheDocument();
  });

  it("reports the pick with the row and the column, and says whether it is picked", async () => {
    const user = userEvent.setup();
    const { onPick } = show({ picked: true });
    const button = screen.getByRole("button", { name: /5000/ });
    expect(button).toHaveAttribute("aria-pressed", "true");
    await user.click(button);
    expect(onPick).toHaveBeenCalledWith(row, 0);
  });

  it("is dashed and muted for a swimlane whose agent is offline", () => {
    show({ offline: true });
    expect(screen.getByRole("button", { name: /5000/ })).toHaveAttribute("data-offline", "true");
  });
});

describe("a path cell", () => {
  it("dims the parents and stresses the leaf", () => {
    render(<PathCell path="channels.atm.limits.max" />);
    expect(screen.getByText("channels.atm.limits.")).toBeInTheDocument();
    expect(screen.getByText("max")).toHaveClass("font-medium");
  });

  it("lets both the parents and the leaf end in an ellipsis, so that a leaf longer than the cell is not cut off with no sign (the layout run, hostile text)", () => {
    render(<PathCell path="channels.atm.limits.max" />);
    expect(screen.getByText("channels.atm.limits.").closest("span")?.className).toMatch(/(^|\s)truncate(\s|$)/);
    const leaf = screen.getByText("max");
    expect(leaf.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(leaf.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    expect(leaf.className).toMatch(/(^|\s)shrink-0(\s|$)/);
  });

  it("shows hidden characters of a path as marks", () => {
    render(<PathCell path={"a.b​c"} />);
    expect(screen.getByRole("img", { name: /U\+200B/ })).toBeInTheDocument();
  });
});
