import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Mask } from "@/api/generated/model";
import { expectNoAxeViolations } from "@/test/axe";
import { MaskedTextView } from "./MaskedTextView";

// The read-only view of a text the server masked (S14b, M1): the rows of the code view (18 px, a number and the line, drawn through
// the one virtualiser), with each marker drawn as a "Hidden value" token. The web masks nothing: the text already holds the marker.
// The view fails closed (plan.ts): a mask it cannot trust hides its whole line, and one it cannot place refuses the text.

const enc = new TextEncoder();
const bytes = (s: string): number => enc.encode(s).length;
const MARK = "[REDACTED: 12 chars, aws_key]";
const SECRET = "hunter2-hunter2";
const mask = (text: string, line: number, needle = MARK, extra: Partial<Mask> = {}): Mask => {
  const start = bytes(text.slice(0, text.indexOf(needle)));
  return { line, start, end: start + bytes(needle), ...extra };
};
const TEXT = `# synthetic\nclientSecret: ${MARK}\nport: 8080\n`;
const cells = (row: HTMLElement) => within(row).getAllByRole("cell");

// WCAG 2.5.8 (axe `target-size`): the Reveal control is small (18 px) and secrets sit on consecutive lines (an id, a secret, a token), so
// the lines of 18 px put two controls 18 px apart, where an undersized target needs 24 px of clear space. A line that carries a control
// is 24 px high: found by the axe run on a file with three revealable values in a row.
describe("MaskedTextView: the height of a line that carries a control", () => {
  const MARK2 = "[REDACTED: 7 chars, jwt]";
  const text = `a: 1\nsecret: ${MARK}\ntoken: ${MARK2}\nz: 2\n`;
  const masks = [mask(text, 2, MARK, { setting: "secret" }), mask(text, 3, MARK2, { setting: "token" })];
  const heights = (): string[] => screen.getAllByRole("row").map((row) => row.style.height);

  it("is 24 px for a line with a control in the place of a marker that names its setting, and 18 px for the others", () => {
    render(<MaskedTextView label="a.yml" text={text} masks={masks} renderMask={(_mark, token) => <>{token}<button type="button">Reveal</button></>} />);
    expect(heights()).toEqual(["18px", "24px", "24px", "18px"]);
  });

  it("is 24 px for a line with a control beside a marker (maskAction)", () => {
    render(<MaskedTextView label="a.yml" text={text} masks={masks} maskAction={() => <button type="button">Act</button>} />);
    expect(heights()).toEqual(["18px", "24px", "24px", "18px"]);
  });

  it("stays 18 px when the view puts nothing in the marker's place, or the marker names no setting", () => {
    const { unmount } = render(<MaskedTextView label="a.yml" text={text} masks={masks} />);
    expect(heights()).toEqual(["18px", "18px", "18px", "18px"]);
    unmount();
    render(<MaskedTextView label="a.yml" text={text} masks={[mask(text, 2), mask(text, 3, MARK2)]} renderMask={(_mark, token) => token} />);
    expect(heights()).toEqual(["18px", "18px", "18px", "18px"]);
  });
});

// A value that is shown in the marker's place is longer than the marker. The rows are as wide as the widest line of the masked text plus
// what the caller says it will add (`extraWidthCh`), so that a value of a hundred characters and the controls after it are reached by
// scrolling the view sideways, not cut off by the line (found by the layout run: a revealed value of 2,000 characters was clipped).
describe("MaskedTextView: room for what is put in a marker's place", () => {
  const spacerWidth = (): string => (document.querySelector("[data-slot='virtual-spacer']") as HTMLElement).style.minWidth;
  const base = `a: 1\nsecret: ${MARK}\n`;

  it("is as wide as the widest line of the text, and as wide as that plus the extra the caller asks for", () => {
    const masks = [mask(base, 2, MARK, { setting: "secret" })];
    const { unmount } = render(<MaskedTextView label="a.yml" text={base} masks={masks} />);
    const widest = `secret: ${MARK}`.length;
    expect(spacerWidth()).toBe(`calc(${widest}ch + 60px)`);
    unmount();
    render(<MaskedTextView label="a.yml" text={base} masks={masks} extraWidthCh={150} />);
    expect(spacerWidth()).toBe(`calc(${widest + 150}ch + 60px)`);
  });
});

describe("MaskedTextView: a text with masks", () => {
  it("shows the lines with their numbers and draws the marker as a token, not as the marker's words", () => {
    render(<MaskedTextView label="a.yml" text={TEXT} masks={[mask(TEXT, 2, MARK, { setting: "clientSecret" })]} />);
    const rows = screen.getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(cells(rows[1] as HTMLElement)[0]?.textContent).toBe("2");
    const line = cells(rows[1] as HTMLElement)[1] as HTMLElement;
    expect(line.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    expect(line).toHaveTextContent("clientSecret:");
    expect(line.textContent).not.toContain("REDACTED");
    expect(cells(rows[2] as HTMLElement).map((c) => c.textContent)).toEqual(["3", "port: 8080"]);
  });

  it("tells a screen reader which setting is hidden", () => {
    render(<MaskedTextView label="a.yml" text={TEXT} masks={[mask(TEXT, 2, MARK, { setting: "clientSecret" })]} />);
    expect(screen.getByRole("table", { name: "a.yml" })).toHaveTextContent("Hidden value for setting clientSecret");
  });

  it("works for a mask with no setting (plain text, a doc, served text): a token and no reveal slot", () => {
    render(<MaskedTextView label="a.yml" text={TEXT} masks={[mask(TEXT, 2)]} maskAction={(m) => <button type="button">Act {m.line}</button>} />);
    expect(document.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    // The slot is offered for every mask, the caller decides whether a mask without a setting gets a control.
    expect(screen.getByRole("button", { name: "Act 2" })).toBeInTheDocument();
  });

  it("works with multi-byte characters before the marker and a CRLF file", () => {
    const text = `é😀: 1\r\n名前: ${MARK}\r\nz: 2\r\n`;
    render(<MaskedTextView label="w.yml" text={text} masks={[mask(text, 2)]} />);
    const rows = screen.getAllByRole("row");
    expect(cells(rows[1] as HTMLElement)[1]).toHaveTextContent("名前:");
    expect(cells(rows[1] as HTMLElement)[1]?.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    // No carriage return is drawn as a marker at the end of a line.
    expect(screen.queryByText("<U+000D>")).toBeNull();
  });

  it("recognises a marker the list does not name, by its form, with no reveal slot", () => {
    const text = `a: ${MARK}\nb: [REDACTED: 5 chars, jwt]\n`;
    render(<MaskedTextView label="a.yml" text={text} masks={[mask(text, 1)]} maskAction={() => <button type="button">Act</button>} />);
    expect(document.querySelectorAll("[data-slot='hidden-value']")).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: "Act" })).toHaveLength(1);
  });

  it("hands the caller the token and the mark, so a caller can put something else in its place", () => {
    render(
      <MaskedTextView
        label="a.yml"
        text={TEXT}
        masks={[mask(TEXT, 2, MARK, { setting: "clientSecret" })]}
        renderMask={(mark, token) => (
          <span data-testid="slot" data-key={mark.key}>
            {token}
            <button type="button">Reveal {mark.setting}</button>
          </span>
        )}
      />,
    );
    expect(screen.getByTestId("slot")).toHaveAttribute("data-key", "2:26");
    expect(screen.getByRole("button", { name: "Reveal clientSecret" })).toBeInTheDocument();
  });

  it("numbers the lines of a slice of a document from its first line, and keeps the masks in the text's own numbering", () => {
    render(<MaskedTextView label="doc" text={TEXT} masks={[mask(TEXT, 2)]} startLine={40} />);
    const rows = screen.getAllByRole("row");
    expect(cells(rows[0] as HTMLElement)[0]?.textContent).toBe("40");
    expect(cells(rows[1] as HTMLElement)[0]?.textContent).toBe("41");
    expect(cells(rows[1] as HTMLElement)[1]?.querySelector("[data-slot='hidden-value']")).not.toBeNull();
  });

  it("marks the line the page points at", () => {
    render(<MaskedTextView label="a.yml" text={TEXT} masks={[mask(TEXT, 2)]} highlightLine={3} />);
    const rows = screen.getAllByRole("row");
    expect(rows[2]?.querySelector("[data-highlight='true']")).not.toBeNull();
    expect(rows[1]?.querySelector("[data-highlight='true']")).toBeNull();
  });

  it("draws only the lines in view of a long text, and finds a mask far down", () => {
    const lines = Array.from({ length: 6000 }, (_, i) => `key${i}: v`);
    lines[4999] = `key4999: ${MARK}`;
    const text = lines.join("\n");
    render(<MaskedTextView label="big.yml" text={text} masks={[mask(text, 5000)]} />);
    expect(screen.getAllByRole("row").length).toBeLessThan(80);
    expect(screen.getByRole("table", { name: "big.yml" })).toHaveAttribute("aria-rowcount", "6000");
  });

  it("makes hidden characters in a line visible", () => {
    render(<MaskedTextView label="a.yml" text={"name: a\u202eb\n"} masks={[]} />);
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
  });

  it("says a file is empty", () => {
    render(<MaskedTextView label="a.yml" text="" masks={[]} />);
    expect(screen.getByText("This file is empty.")).toBeInTheDocument();
  });

  it("has no axe violations", async () => {
    const { container } = render(<MaskedTextView label="a.yml" text={TEXT} masks={[mask(TEXT, 2, MARK, { setting: "clientSecret" })]} />);
    await expectNoAxeViolations(container);
  });
});

describe("MaskedTextView: absent or empty masks", () => {
  it("draws the text as it came, with a marker recognised by its form", () => {
    const text = `a: 1\nb: ${MARK}\n`;
    for (const masks of [undefined, []] as const) {
      const { unmount } = render(<MaskedTextView label="a.yml" text={text} {...(masks === undefined ? {} : { masks })} />);
      expect(screen.getAllByRole("row")).toHaveLength(2);
      expect(document.querySelectorAll("[data-slot='hidden-value']")).toHaveLength(1);
      unmount();
    }
  });
});

describe("MaskedTextView: it fails closed", () => {
  const text = `a: 1\nclientSecret: ${SECRET}\nb: ${MARK}\n`;
  const start = bytes(`a: 1\nclientSecret: `);

  it("hides the whole line when the span is not a marker: the value is never drawn", () => {
    render(<MaskedTextView label="a.yml" text={text} masks={[{ line: 2, start, end: start + bytes(SECRET) }]} />);
    const rows = screen.getAllByRole("row");
    expect(cells(rows[1] as HTMLElement)[1]?.querySelector("[data-slot='hidden-line']")).not.toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(document.body.innerHTML).not.toContain("clientSecret");
    // The other lines are drawn.
    expect(cells(rows[0] as HTMLElement)[1]).toHaveTextContent("a: 1");
  });

  it("hides the line when a mask covers only part of the value, or the value and more", () => {
    for (const [from, to] of [
      [start, start + 4],
      [start + 4, start + bytes(SECRET)],
    ] as const) {
      const { unmount } = render(<MaskedTextView label="a.yml" text={text} masks={[{ line: 2, start: from, end: to }]} />);
      expect(document.body.innerHTML, `${from}-${to}`).not.toContain(SECRET);
      unmount();
    }
  });

  it("refuses to draw the text at all when a mask names a line it does not have, and says so", () => {
    render(<MaskedTextView label="a.yml" text={text} masks={[{ line: 9, start: 0, end: 5 }]} />);
    expect(screen.queryByRole("table")).toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(screen.getByText("This text can't be shown safely")).toBeInTheDocument();
  });

  it("refuses when a range leaves the line it names, even where it would have been a marker", () => {
    render(<MaskedTextView label="a.yml" text={text} masks={[{ line: 1, start, end: start + bytes(SECRET) }]} />);
    expect(screen.queryByRole("table")).toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
  });

  it("refuses on a malformed mask: fractions, negatives, backwards", () => {
    for (const bad of [{ line: 2, start: 1.5, end: 9 }, { line: 2, start: -4, end: 9 }, { line: 2, start: 9, end: 3 }]) {
      const { unmount } = render(<MaskedTextView label="a.yml" text={text} masks={[bad]} />);
      expect(screen.queryByRole("table"), JSON.stringify(bad)).toBeNull();
      expect(document.body.innerHTML).not.toContain(SECRET);
      unmount();
    }
  });

  it("holds when the text is hostile: bidi, zero-width and a fake marker around a real one", () => {
    const hostile = `\u202ek\u200b: ${MARK} [REDACTED: 1 chars, evil\u202e]\n`;
    render(<MaskedTextView label="h.yml" text={hostile} masks={[mask(hostile, 1)]} />);
    expect(screen.getAllByText(/^<U\+/).length).toBeGreaterThanOrEqual(2);
    expect(document.querySelectorAll("[data-slot='hidden-value']")).toHaveLength(1);
  });
});
