import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Mask } from "@/api/generated/model";
import { MaskedCodeView } from "./MaskedCodeView";

const MARK = "[REDACTED: 12 chars, aws_key]";
const TEXT = `# synthetic\nclientSecret: ${MARK}\nport: 8080\n`;
const MASK: Mask = { line: 2, start: 12 + 14, end: 12 + 14 + MARK.length, setting: "clientSecret" };

describe("MaskedCodeView", () => {
  it("shows the lines of the masked text with their numbers", () => {
    render(<MaskedCodeView label="a.yml" text={TEXT} masks={[MASK]} />);
    const rows = screen.getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(within(rows[2] as HTMLElement).getAllByRole("cell").map((c) => c.textContent)).toEqual(["3", "port: 8080"]);
  });

  it("marks the marker the server placed, and says what it is", () => {
    render(<MaskedCodeView label="a.yml" text={TEXT} masks={[MASK]} />);
    const mark = screen.getByText(MARK);
    expect(mark.closest("[data-slot='mask']")).not.toBeNull();
    expect(mark.closest("[data-slot='mask']")).toHaveAttribute("title", expect.stringMatching(/masked/i));
    expect(screen.getByText("clientSecret:", { exact: false })).toBeInTheDocument();
  });

  it("shows only what the server sent: nothing in the line is unmasked or made up", () => {
    const { container } = render(<MaskedCodeView label="a.yml" text={TEXT} masks={[MASK]} />);
    expect(container.textContent).toContain(`clientSecret: ${MARK}`);
  });

  it("does not mark text that a bad range names (it is shown as it came)", () => {
    const bad: Mask = { line: 2, start: 0, end: 5 };
    render(<MaskedCodeView label="a.yml" text={TEXT} masks={[bad]} />);
    expect(document.querySelector("[data-slot='mask']")).toBeNull();
    expect(screen.getByText(`clientSecret: ${MARK}`, { exact: false })).toBeInTheDocument();
  });

  it("offers a place beside each marker for an action, with the mask it is for", () => {
    render(<MaskedCodeView label="a.yml" text={TEXT} masks={[MASK]} maskAction={(mask) => <button type="button">Reveal {mask.setting}</button>} />);
    expect(screen.getByRole("button", { name: "Reveal clientSecret" })).toBeInTheDocument();
  });

  it("makes hidden characters in a line visible", () => {
    render(<MaskedCodeView label="a.yml" text={"name: a‮b\n"} masks={[]} />);
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
  });

  it("marks the line the page points at", () => {
    render(<MaskedCodeView label="a.yml" text={TEXT} masks={[MASK]} highlightLine={3} />);
    const rows = screen.getAllByRole("row");
    expect(rows[2]?.querySelector("[data-highlight='true']")).not.toBeNull();
    expect(rows[1]?.querySelector("[data-highlight='true']")).toBeNull();
  });

  it("draws only the lines in view of a long text", () => {
    const long = Array.from({ length: 6000 }, (_, i) => `key${i}: v`).join("\n");
    render(<MaskedCodeView label="big.yml" text={long} masks={[]} />);
    expect(screen.getAllByRole("row").length).toBeLessThan(80);
    expect(screen.getByRole("table", { name: "big.yml" })).toHaveAttribute("aria-rowcount", "6000");
  });
});
