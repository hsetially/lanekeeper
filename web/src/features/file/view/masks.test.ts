import { describe, expect, it } from "vitest";
import type { Mask } from "@/api/generated/model";
import { isMarker, lineMasks, splitLfLines } from "./masks";

// The server masks a flagged value in the text and says where: UTF-8 byte offsets into the whole `text`, half-open, with the
// 1-based line that holds the marker (lines end at a line feed; a carriage return before it belongs to the line). The web
// converts them to places in each line's own string. A range that is not exactly a marker is dropped: it is untrusted input.

const MARK = "[REDACTED: 12 chars, aws_key]";

function maskOf(text: string, line: number, marker = MARK): Mask {
  const bytes = new TextEncoder();
  const before = text.slice(0, text.indexOf(marker));
  const start = bytes.encode(before).length;
  return { line, start, end: start + bytes.encode(marker).length };
}

describe("splitLfLines", () => {
  it("ends a line at a line feed and drops the carriage return before it", () => {
    expect(splitLfLines("a: 1\r\nb: 2\r\n")).toEqual(["a: 1", "b: 2"]);
    expect(splitLfLines("a\nb")).toEqual(["a", "b"]);
  });
  it("has no empty last line for the final line ending, and no lines for no text", () => {
    expect(splitLfLines("a\n")).toEqual(["a"]);
    expect(splitLfLines("")).toEqual([]);
  });
  it("does not split on a lone carriage return (the contract numbers lines by line feed)", () => {
    expect(splitLfLines("a\rb\nc")).toEqual(["a\rb", "c"]);
  });
});

describe("isMarker", () => {
  it("is the form [REDACTED: N chars, rule] and nothing else", () => {
    expect(isMarker(MARK)).toBe(true);
    expect(isMarker("[REDACTED: 0 chars, x]")).toBe(true);
    expect(isMarker("[REDACTED: 12 chars, AWS]")).toBe(false);
    expect(isMarker(`x${MARK}`)).toBe(false);
    expect(isMarker("secret")).toBe(false);
  });
});

describe("lineMasks", () => {
  it("places a marker in the line that holds it", () => {
    const text = `a: 1\nclientSecret: ${MARK}\nb: 2\n`;
    const found = lineMasks(text, [maskOf(text, 2)]);
    expect(found.get(2)).toEqual([{ start: 14, end: 14 + MARK.length, mask: maskOf(text, 2) }]);
    expect(found.size).toBe(1);
  });

  it("counts bytes, not characters, before the marker (a wide character in an earlier line and in this one)", () => {
    const text = `# ünï ✓\nname: é€ ${MARK} tail\n`;
    const mask = maskOf(text, 2);
    const found = lineMasks(text, [mask]);
    const line = splitLfLines(text)[1] ?? "";
    const place = found.get(2)?.[0];
    expect(place).toBeDefined();
    expect(line.slice(place?.start, place?.end)).toBe(MARK);
  });

  it("reads a CRLF file by the same rule", () => {
    const text = `a: 1\r\nk: ${MARK}\r\n`;
    const found = lineMasks(text, [maskOf(text, 2)]);
    const place = found.get(2)?.[0];
    expect(splitLfLines(text)[1]?.slice(place?.start, place?.end)).toBe(MARK);
  });

  it("keeps several markers in one line in order", () => {
    const second = "[REDACTED: 3 chars, token]";
    const text = `x: ${MARK} y: ${second}\n`;
    const found = lineMasks(text, [maskOf(text, 1, second), maskOf(text, 1)]);
    expect(found.get(1)?.map((p) => (splitLfLines(text)[0] ?? "").slice(p.start, p.end))).toEqual([MARK, second]);
  });

  it("drops a range that is not exactly a marker, and one that names a line the text does not have", () => {
    const text = `a: 1\nk: ${MARK}\n`;
    const real = maskOf(text, 2);
    const shifted: Mask = { ...real, start: real.start + 1, end: real.end + 1 };
    const wrongLine: Mask = { ...real, line: 9 };
    const backwards: Mask = { line: 2, start: 20, end: 10 };
    const fractional: Mask = { line: 2, start: 3.5, end: 8 };
    expect(lineMasks(text, [shifted, wrongLine, backwards, fractional]).size).toBe(0);
    expect(lineMasks(text, [real, shifted]).get(2)).toHaveLength(1);
  });

  it("is bounded: at most 1000 masks are read", () => {
    const encoder = new TextEncoder();
    const masks: Mask[] = [];
    let offset = 0;
    const lines = Array.from({ length: 1200 }, (_, i) => {
      const prefix = `k${i}: `;
      masks.push({ line: i + 1, start: offset + encoder.encode(prefix).length, end: offset + encoder.encode(prefix).length + MARK.length });
      offset += encoder.encode(`${prefix}${MARK}\n`).length;
      return `${prefix}${MARK}`;
    });
    const found = lineMasks(`${lines.join("\n")}\n`, masks);
    expect(found.size).toBe(1000);
  });
});
