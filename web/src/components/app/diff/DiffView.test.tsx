import { act, render, screen, within } from "@testing-library/react";
import { createRef } from "react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DiffHunk, DiffLine } from "@/api/generated/model";
import { expectNoAxeViolations } from "@/test/axe";
import { DiffView, type DiffViewHandle } from "./DiffView";

const ctx = (text: string): DiffLine => ({ kind: "context", text, inline_changes: [] });
const del = (text: string, ranges: Array<[number, number]> = []): DiffLine => ({ kind: "removed", text, inline_changes: ranges.map(([start, end]) => ({ start, end })) });
const add = (text: string, ranges: Array<[number, number]> = []): DiffLine => ({ kind: "added", text, inline_changes: ranges.map(([start, end]) => ({ start, end })) });
const hunk = (old_start: number, old_lines: number, new_start: number, new_lines: number, lines: DiffLine[]): DiffHunk => ({ old_start, old_lines, new_start, new_lines, lines });

/** The row whose text holds `text`. A changed span splits a line into several elements, so look at the row's text. */
const rowWith = (text: string): HTMLElement => {
  const row = screen.getAllByRole("row").find((r) => (r.textContent ?? "").includes(text));
  if (!row) throw new Error(`no row with ${text}`);
  return row;
};

const small: DiffHunk[] = [hunk(10, 3, 10, 3, [ctx("keep"), del("port: 8080", [[6, 10]]), add("port: 9090", [[6, 10]]), ctx("tail")])];
const twoChanges: DiffHunk[] = [hunk(1, 3, 1, 3, [ctx("a"), del("b"), add("B")]), hunk(40, 3, 40, 3, [ctx("x"), del("y"), add("Y")]), hunk(90, 2, 90, 3, [add("n1"), add("n2"), ctx("z")])];

/** Synthetic hunks for a long file: one change every ten lines, three lines of context, as the engine makes them. */
function longDiff(lines: number): DiffHunk[] {
  const hunks: DiffHunk[] = [];
  for (let at = 5; at < lines; at += 10) {
    const before = Array.from({ length: 3 }, (_, i) => ctx(`line ${at - 3 + i}`));
    const after = Array.from({ length: 3 }, (_, i) => ctx(`line ${at + 1 + i}`));
    hunks.push(hunk(at - 2, 7, at - 2, 7, [...before, del(`line ${at}`), add(`line ${at} changed`), ...after]));
  }
  return hunks;
}

describe("DiffView, unified", () => {
  it("shows each line with its numbers and a marker for added and removed", () => {
    render(<DiffView label="Diff" hunks={small} mode="unified" />);
    const rows = screen.getAllByRole("row");
    // fold (9 lines above), hunk header, 4 lines
    expect(rows).toHaveLength(6);
    const removed = rowWith("port: 8080");
    expect(within(removed).getByLabelText("removed")).toHaveTextContent("-");
    const added = rowWith("port: 9090");
    expect(within(added).getByLabelText("added")).toHaveTextContent("+");
    expect(within(removed).getAllByRole("cell").map((c) => c.textContent)).toEqual(["11", "", "-", "port: 8080"]);
    expect(within(added).getAllByRole("cell").map((c) => c.textContent)).toEqual(["", "11", "+", "port: 9090"]);
  });

  it("shows the hunk header and a fold for the lines above the first hunk", () => {
    render(<DiffView label="Diff" hunks={small} mode="unified" />);
    expect(screen.getByText("@@ -10,3 +10,3 @@")).toBeInTheDocument();
    expect(screen.getByText("9 unchanged lines")).toBeInTheDocument();
  });

  it("says there are no differences for no hunks, or what the screen asks it to say", () => {
    const { rerender } = render(<DiffView label="Diff" hunks={[]} mode="unified" />);
    expect(screen.getByText("No differences")).toBeInTheDocument();
    rerender(<DiffView label="Diff" hunks={[]} mode="unified" empty={<p>Only line endings differ</p>} />);
    expect(screen.getByText("Only line endings differ")).toBeInTheDocument();
  });
});

describe("DiffView, split", () => {
  it("puts the removed line on the left and the added line on the right of one row", () => {
    render(<DiffView label="Diff" hunks={small} mode="split" />);
    const row = rowWith("port: 8080");
    expect(row).toHaveTextContent("port: 9090");
    expect(within(row).getAllByRole("cell").map((c) => c.textContent)).toEqual(["11", "-", "port: 8080", "11", "+", "port: 9090"]);
  });

  it("leaves a side empty, with no number, where nothing corresponds", () => {
    render(<DiffView label="Diff" hunks={[hunk(2, 0, 3, 1, [add("only added")])]} mode="split" />);
    const row = rowWith("only added");
    expect(within(row).getAllByRole("cell").map((c) => c.textContent)).toEqual(["", "", "", "3", "+", "only added"]);
  });

  it("has fewer rows than the unified view when lines pair up", () => {
    const { unmount } = render(<DiffView label="Diff" hunks={small} mode="split" />);
    const split = screen.getAllByRole("row").length;
    unmount();
    render(<DiffView label="Diff" hunks={small} mode="unified" />);
    expect(split).toBeLessThan(screen.getAllByRole("row").length);
  });
});

describe("DiffView, changed words", () => {
  const marks = () => [...document.querySelectorAll("mark")].map((m) => m.textContent);

  it("marks the changed span of a line, on both sides", () => {
    render(<DiffView label="Diff" hunks={small} mode="split" />);
    expect(marks()).toEqual(["8080", "9090"]);
  });

  it("reads the spans as UTF-8 bytes, so a line with accents and emoji marks the right characters", () => {
    // "é😀x": é is 2 bytes, 😀 is 4 bytes, x is 1 byte. Bytes 6 to 7 are the x.
    render(<DiffView label="Diff" hunks={[hunk(1, 1, 1, 1, [del("é😀x", [[6, 7]]), add("é😀y", [[6, 7]])])]} mode="split" />);
    expect(marks()).toEqual(["x", "y"]);
  });

  it("shows the plain line, and does not fail, when a span cannot be right", () => {
    render(<DiffView label="Diff" hunks={[hunk(1, 1, 1, 1, [del("é😀x", [[1, 3], [99, 120], [5, 2]]), add("ok", [[0, 99]])])]} mode="split" />);
    expect(marks()).toEqual([]);
    expect(screen.getByText("é😀x")).toBeInTheDocument();
  });

  it("makes hidden characters visible (a name can lie: S8)", () => {
    render(<DiffView label="Diff" hunks={[hunk(1, 1, 1, 1, [del("a‮b"), add("a​b")])]} mode="split" />);
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
    expect(screen.getByText("<U+200B>")).toBeInTheDocument();
  });

  it("never turns text into markup", () => {
    render(<DiffView label="Diff" hunks={[hunk(1, 1, 1, 1, [del("<script>alert(1)</script>"), add("<img src=x onerror=alert(1)>")])]} mode="split" />);
    expect(document.querySelector("script, img")).toBeNull();
    expect(screen.getByText("<script>alert(1)</script>")).toBeInTheDocument();
  });
});

describe("DiffView, folds", () => {
  const between: DiffHunk[] = [hunk(1, 2, 1, 2, [ctx("a"), del("b")]), hunk(6, 2, 6, 2, [add("y"), ctx("z")])];

  it("shows a fold as text when the screen has not got the file", () => {
    render(<DiffView label="Diff" hunks={between} mode="unified" />);
    expect(screen.getByText("3 unchanged lines")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /unchanged/ })).not.toBeInTheDocument();
  });

  it("opens a fold on a click when it can get the lines, and shows them numbered on both sides", async () => {
    const user = userEvent.setup();
    const getLines = vi.fn((side: string, from: number, count: number) => Array.from({ length: count }, (_, i) => `${side} line ${from + i}`));
    render(<DiffView label="Diff" hunks={between} mode="unified" getLines={getLines} />);
    await user.click(screen.getByRole("button", { name: "3 unchanged lines" }));
    expect(getLines).toHaveBeenCalledWith("old", 3, 3);
    expect(getLines).toHaveBeenCalledWith("new", 3, 3);
    // An unchanged line is the same on both sides; the unified view shows the new side's text, with both numbers.
    expect(screen.getByText("new line 3")).toBeInTheDocument();
    expect(rowWith("new line 3")).toHaveTextContent("33new line 3");
    expect(screen.queryByText("3 unchanged lines")).not.toBeInTheDocument();
  });
});

describe("DiffView, n and p", () => {
  it("n goes to the first change, then the next; p goes back; they stop at the ends", async () => {
    const user = userEvent.setup();
    const onActiveChange = vi.fn();
    render(<DiffView label="Diff" hunks={twoChanges} mode="split" onActiveChange={onActiveChange} />);
    const diff = screen.getByRole("grid", { name: "Diff" });
    await user.click(diff);
    expect(diff).toHaveAttribute("data-active-change", "none");
    await user.keyboard("n");
    expect(diff).toHaveAttribute("data-active-change", "0");
    await user.keyboard("n");
    expect(diff).toHaveAttribute("data-active-change", "1");
    await user.keyboard("nnn");
    expect(diff).toHaveAttribute("data-active-change", "2");
    await user.keyboard("p");
    expect(diff).toHaveAttribute("data-active-change", "1");
    await user.keyboard("ppp");
    expect(diff).toHaveAttribute("data-active-change", "0");
    expect(onActiveChange).toHaveBeenLastCalledWith(0, 3);
  });

  it("points assistive technology at the row of the change", async () => {
    const user = userEvent.setup();
    render(<DiffView label="Diff" hunks={twoChanges} mode="split" />);
    const diff = screen.getByRole("grid", { name: "Diff" });
    await user.click(diff);
    await user.keyboard("n");
    const row = document.getElementById(diff.getAttribute("aria-activedescendant") ?? "");
    expect(row).toHaveTextContent("b");
    expect(row).toHaveTextContent("B");
  });

  it("does not steal the keys from a text field", async () => {
    const user = userEvent.setup();
    render(
      <div>
        <input aria-label="filter" />
        <DiffView label="Diff" hunks={twoChanges} mode="split" />
      </div>,
    );
    await user.type(screen.getByLabelText("filter"), "np");
    expect(screen.getByLabelText("filter")).toHaveValue("np");
    expect(screen.getByRole("grid", { name: "Diff" })).toHaveAttribute("data-active-change", "none");
  });

  it("does nothing, and does not fail, when there is no change to go to", () => {
    const ref = createRef<DiffViewHandle>();
    render(<DiffView ref={ref} label="Diff" hunks={[]} mode="split" />);
    expect(() => act(() => ref.current?.goToChange(1))).not.toThrow();
  });

  it("takes the keyboard on request, so n works as soon as the diff is on screen", () => {
    const ref = createRef<DiffViewHandle>();
    render(<DiffView ref={ref} label="Diff" hunks={twoChanges} mode="split" />);
    act(() => ref.current?.focus());
    expect(document.activeElement).toBe(screen.getByRole("grid", { name: "Diff" }));
  });

  it("can be driven from outside: goToChange moves the same way", () => {
    const ref = createRef<DiffViewHandle>();
    render(<DiffView ref={ref} label="Diff" hunks={twoChanges} mode="split" />);
    const diff = screen.getByRole("grid", { name: "Diff" });
    act(() => ref.current?.goToChange(1));
    expect(diff).toHaveAttribute("data-active-change", "0");
    act(() => ref.current?.goToChange(1));
    expect(diff).toHaveAttribute("data-active-change", "1");
    act(() => ref.current?.goToChange(-1));
    expect(diff).toHaveAttribute("data-active-change", "0");
  });
});

describe("DiffView, a long file", () => {
  it("draws only the rows in view of a 6,000-line file, whichever the view", () => {
    const hunks = longDiff(6_000);
    for (const mode of ["split", "unified"] as const) {
      const { unmount } = render(<DiffView label="Diff" hunks={hunks} mode={mode} />);
      expect(screen.getAllByRole("row").length).toBeLessThan(120);
      unmount();
    }
  });

  it("sizes the scroll area for every row at the height of its kind", () => {
    const { container } = render(<DiffView label="Diff" hunks={small} mode="unified" />);
    // fold 24 + hunk 20 + four lines of 18
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe(`${24 + 20 + 4 * 18}px`);
  });
});

describe("DiffView, accessibility", () => {
  it("passes the axe structure checks in both views", async () => {
    const { container, rerender } = render(<DiffView label="Diff" hunks={twoChanges} mode="split" />);
    await expectNoAxeViolations(container);
    rerender(<DiffView label="Diff" hunks={twoChanges} mode="unified" />);
    await expectNoAxeViolations(container);
  });
});
