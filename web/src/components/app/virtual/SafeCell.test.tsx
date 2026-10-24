import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { splitUnsafe } from "@/components/app/safe-text";
import { forAll } from "@/test/prng";
import { SafeCell } from "./SafeCell";

// SafeCell must mark exactly what SafeText marks. It is a faster way to draw a text with nothing to mark, never a way to
// skip marking, so the property test compares it with the function SafeText itself uses.

const CHARS = ["a", "b", " ", "\t", "é", "漢", "😀", "​", "‮", "⁦", "﻿", "\u0000", "­", "ㅤ", "⠀", "͏", " ", "\ud800", "-", "_"];

describe("SafeCell", () => {
  it("draws a plain text as a text node, with no element around it", () => {
    const { container } = render(<div data-testid="cell">
      <SafeCell text="spring.datasource.url" />
    </div>);
    expect(container.querySelector("bdi")).toBeNull();
    expect(container.querySelector("[data-testid='cell']")?.childNodes).toHaveLength(1);
    expect(container.textContent).toBe("spring.datasource.url");
  });

  it("marks hidden and bidi characters, as SafeText does", () => {
    const { getByText } = render(<SafeCell text={"a‮b"} />);
    expect(getByText("<U+202E>")).toBeInTheDocument();
  });

  it("keeps a tab as a tab", () => {
    const { container } = render(<SafeCell text={"a\tb"} />);
    expect(container.textContent).toBe("a\tb");
    expect(container.querySelector("[role='img']")).toBeNull();
  });

  it("marks exactly what splitUnsafe marks, for any mix of characters (10,000 cases)", () => {
    forAll(
      10_000,
      (rng) => Array.from({ length: rng.int(10) }, () => rng.pick(CHARS)).join(""),
      (text) => {
        const { container, unmount } = render(<SafeCell text={text} />);
        const marked = splitUnsafe(text).filter((s) => s.kind === "mark").length;
        expect(container.querySelectorAll("[role='img']").length).toBe(marked);
        unmount();
        // RTL leaves the container in the body after unmount; 10,000 of them make the afterEach cleanup slower than its timeout.
        container.remove();
      },
    );
  }, 60_000);
});
