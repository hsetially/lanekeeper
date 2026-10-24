import { act, render, screen, within } from "@testing-library/react";
import { createRef } from "react";
import { describe, expect, it, vi } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { CodeView, splitLines, type CodeViewHandle } from "./CodeView";

describe("splitLines", () => {
  it("splits on every kind of line ending a config file can have", () => {
    expect(splitLines("a\nb\nc")).toEqual(["a", "b", "c"]);
    expect(splitLines("a\r\nb\r\nc")).toEqual(["a", "b", "c"]);
    expect(splitLines("a\rb")).toEqual(["a", "b"]);
    expect(splitLines("a\r\nb\nc\rd")).toEqual(["a", "b", "c", "d"]);
  });

  it("does not make an empty last line out of the final line ending", () => {
    expect(splitLines("a\nb\n")).toEqual(["a", "b"]);
    expect(splitLines("a\r\n")).toEqual(["a"]);
    expect(splitLines("a\n\n")).toEqual(["a", ""]);
  });

  it("is no lines for no text", () => {
    expect(splitLines("")).toEqual([]);
  });

  it("keeps empty lines, and spaces and tabs at the start and the end", () => {
    expect(splitLines("  a\t\n\n b ")).toEqual(["  a\t", "", " b "]);
  });
});

describe("CodeView", () => {
  const text = Array.from({ length: 6_000 }, (_, i) => `key${i}: value${i}`).join("\n");

  it("shows a line number beside each line", () => {
    render(<CodeView label="a.yml" text={"first\nsecond\nthird\n"} />);
    const rows = screen.getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(within(rows[1] as HTMLElement).getAllByRole("cell").map((c) => c.textContent)).toEqual(["2", "second"]);
  });

  it("starts numbering where the screen says, for a slice of a file", () => {
    render(<CodeView label="a.yml" text={"x\ny"} startLine={120} />);
    expect(screen.getAllByRole("row").map((r) => within(r).getAllByRole("cell")[0]?.textContent)).toEqual(["120", "121"]);
  });

  it("draws only the lines in view of a 6,000-line file, and sets the scroll area for all of them", () => {
    const { container } = render(<CodeView label="big.yml" text={text} />);
    expect(screen.getAllByRole("row").length).toBeLessThan(80);
    expect(screen.getByRole("table", { name: "big.yml" })).toHaveAttribute("aria-rowcount", "6000");
    expect(container.querySelector<HTMLElement>("[data-slot='virtual-spacer']")?.style.height).toBe(`${6_000 * 18}px`);
  });

  it("shows a file with Windows line endings without a marker at the end of every line", () => {
    render(<CodeView label="a.yml" text={"a: 1\r\nb: 2\r\n"} />);
    expect(screen.queryByText(/U\+000D/)).not.toBeInTheDocument();
    expect(screen.getAllByRole("row")).toHaveLength(2);
  });

  it("makes hidden characters visible, because a file name or a value can lie (S8)", () => {
    render(<CodeView label="a.yml" text={"name: a‮b\nother: x​y"} />);
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
    expect(screen.getByText("<U+200B>")).toBeInTheDocument();
  });

  it("never turns text into markup", () => {
    render(<CodeView label="a.yml" text={"<img src=x onerror=alert(1)>\n<script>alert(1)</script>"} />);
    expect(document.querySelector("img, script")).toBeNull();
    expect(screen.getByText("<script>alert(1)</script>")).toBeInTheDocument();
  });

  it("marks the line the screen points at, and scrolls to it", () => {
    const ref = createRef<CodeViewHandle>();
    const spy = vi.fn();
    const { container } = render(<CodeView ref={ref} label="a.yml" text={text} highlightLine={5} />);
    const marked = container.querySelectorAll("[data-highlight='true']");
    expect(marked).toHaveLength(1);
    expect(marked[0]).toHaveTextContent("key4: value4");
    const list = screen.getByRole("table", { name: "a.yml" });
    Object.defineProperty(list, "scrollTo", { configurable: true, value: spy });
    Object.defineProperty(list, "scrollHeight", { configurable: true, value: 6_000 * 18 });
    act(() => ref.current?.scrollToLine(3_000));
    expect(spy).toHaveBeenCalled();
  });

  it("keeps the highlighted line rendered when the view is scrolled away from it", () => {
    render(<CodeView label="a.yml" text={text} highlightLine={5} />);
    const list = screen.getByRole("table", { name: "a.yml" });
    Object.defineProperty(list, "scrollTop", { configurable: true, writable: true, value: 18 * 4_000 });
    act(() => {
      list.dispatchEvent(new Event("scroll"));
    });
    expect(screen.getByText("key4: value4")).toBeInTheDocument();
  });

  it("says an empty file is empty", () => {
    render(<CodeView label="a.yml" text="" />);
    expect(screen.getByText("This file is empty.")).toBeInTheDocument();
  });

  it("can be reached from the keyboard, since it scrolls", () => {
    render(<CodeView label="a.yml" text="a" />);
    expect(screen.getByRole("table", { name: "a.yml" })).toHaveAttribute("tabindex", "0");
  });

  it("passes the axe structure checks", async () => {
    const { container } = render(<CodeView label="a.yml" text={"one\ntwo\nthree"} highlightLine={2} />);
    await expectNoAxeViolations(container);
  });
});
