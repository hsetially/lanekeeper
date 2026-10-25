import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SafeText } from "./SafeText";
import { splitUnsafe, visibleText } from "./safe-text";

// S8 / W21: text from names, paths, diff lines and hits is data from a file or a person. Bidi controls can make
// "exe.png" read as "gnp.exe"; zero-width characters make two different names look the same.
const BIDI = ["\u202A", "\u202B", "\u202C", "\u202D", "\u202E", "\u2066", "\u2067", "\u2068", "\u2069"];
const ZERO_WIDTH = ["\u200B", "\u200C", "\u200D", "\u200E", "\u200F", "\uFEFF"];
const OTHER = ["\u061C", "\u00AD", "\u2060", "\u2028", "\u2029", "\u0007", "\u007F", "\u0085", "\u3164", "\u115F", "\u2800"];

const hex = (ch: string): string => `U+${(ch.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}`;

describe("splitUnsafe and visibleText", () => {
  it("leaves ordinary text, tabs, accents, CJK and emoji alone", () => {
    const text = "tx-infinity-core-sit1.yml\tCafé 日本語 \u{1F600}";
    expect(splitUnsafe(text)).toEqual([{ kind: "text", text }]);
    expect(visibleText(text)).toBe(text);
  });

  it.each([...BIDI, ...ZERO_WIDTH, ...OTHER])("replaces %j with a visible marker", (ch) => {
    const segments = splitUnsafe(`a${ch}b`);
    expect(segments).toHaveLength(3);
    expect(segments[1]).toMatchObject({ kind: "mark", codePoint: hex(ch) });
    expect(visibleText(`a${ch}b`)).toBe(`a<${hex(ch)}>b`);
  });

  it("marks a lone surrogate, which is not text", () => {
    expect(visibleText("a\uD800b")).toBe("a<U+D800>b");
  });

  it("shows the real name of the classic spoof", () => {
    expect(visibleText("invoice\u202Egnp.exe")).toBe("invoice<U+202E>gnp.exe");
  });

  it("names the common ones and falls back to a category for the rest", () => {
    const named = splitUnsafe("\u202E").find((s) => s.kind === "mark");
    expect(named).toMatchObject({ name: "right-to-left override" });
    const other = splitUnsafe("\u0007").find((s) => s.kind === "mark");
    expect(other).toMatchObject({ name: "control character" });
  });
});

describe("SafeText", () => {
  it("renders plain text as is", () => {
    render(<SafeText text="sitb" />);
    expect(screen.getByText("sitb")).toBeInTheDocument();
  });

  it.each([...BIDI, ...ZERO_WIDTH])("never puts %j into the DOM", (ch) => {
    const { container } = render(<SafeText text={`a${ch}b`} />);
    expect(container.textContent).not.toContain(ch);
    expect(container.textContent).toContain(`<${hex(ch)}>`);
    expect(screen.getByRole("img", { name: new RegExp(hex(ch).replace("+", "\\+")) })).toBeInTheDocument();
  });

  it("isolates the text so a right-to-left name cannot reorder its neighbours", () => {
    const { container } = render(<SafeText text="שלום" />);
    expect(container.querySelector("bdi")).not.toBeNull();
  });

  it("renders an empty string as nothing visible", () => {
    const { container } = render(<SafeText text="" />);
    expect(container.textContent).toBe("");
  });
});
