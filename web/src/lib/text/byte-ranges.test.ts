import { describe, expect, it } from "vitest";
import { forAll, type Rng } from "@/test/prng";
import { byteRangesToUtf16, utf8Length } from "./byte-ranges";

// The API sends a changed span as half-open UTF-8 byte offsets into the text of the same object (F10). A JavaScript string
// is UTF-16, so the offsets are converted before they index it. The property tests check the converter against a reference
// that does not share its code: it encodes the text with TextEncoder and decodes the slice.

const enc = new TextEncoder();
const dec = new TextDecoder("utf-8", { fatal: true });
const ALPHABET = ["a", "b", " ", "_", "-", "é", "ñ", "€", "あ", "漢", "😀", "🧑‍💻", "́", "x"];

const text = (rng: Rng, maxChars: number): string => {
  let out = "";
  const n = rng.int(maxChars + 1);
  for (let i = 0; i < n; i++) out += rng.pick(ALPHABET);
  return out;
};

describe("utf8Length", () => {
  it("matches the encoder for every width", () => {
    for (const s of ["", "abc", "é", "€", "😀", "a😀é€あ", "\u0000", "\u007f", "\u0080", "߿", "ࠀ", "￿"]) {
      expect(utf8Length(s), JSON.stringify(s)).toBe(enc.encode(s).length);
    }
  });

  it("counts a lone surrogate as the replacement character, which is what the encoder writes", () => {
    expect(utf8Length("\ud800")).toBe(enc.encode("\ud800").length);
    expect(utf8Length("a\udc00b")).toBe(enc.encode("a\udc00b").length);
  });
});

describe("byteRangesToUtf16", () => {
  it("is the identity on ASCII", () => {
    expect(byteRangesToUtf16("hello world", [{ start: 6, end: 11 }])).toEqual([{ start: 6, end: 11 }]);
  });

  it("moves a range past multi-byte characters to UTF-16 units", () => {
    // "é" is 2 bytes and 1 unit, "😀" is 4 bytes and 2 units.
    const s = "é😀x";
    expect(byteRangesToUtf16(s, [{ start: 6, end: 7 }])).toEqual([{ start: 3, end: 4 }]);
    expect(byteRangesToUtf16(s, [{ start: 2, end: 6 }])).toEqual([{ start: 1, end: 3 }]);
  });

  it("drops a range that cuts a character in two", () => {
    const s = "a😀b";
    expect(byteRangesToUtf16(s, [{ start: 2, end: 4 }])).toEqual([]);
    expect(byteRangesToUtf16(s, [{ start: 1, end: 3 }])).toEqual([]);
    expect(byteRangesToUtf16(s, [{ start: 1, end: 5 }])).toEqual([{ start: 1, end: 3 }]);
  });

  it("drops a range that is empty, backwards, negative, fractional, not finite or past the end", () => {
    const s = "héllo";
    const bad = [
      { start: 2, end: 2 },
      { start: 4, end: 1 },
      { start: -1, end: 3 },
      { start: 0.5, end: 3 },
      { start: 0, end: 3.5 },
      { start: 0, end: Number.NaN },
      { start: Number.POSITIVE_INFINITY, end: Number.POSITIVE_INFINITY },
      { start: 0, end: 7 },
      { start: 6, end: 99 },
    ];
    expect(utf8Length(s)).toBe(6);
    for (const range of bad) expect(byteRangesToUtf16(s, [range]), JSON.stringify(range)).toEqual([]);
  });

  it("keeps the good ranges when a bad one is among them", () => {
    expect(
      byteRangesToUtf16("abcdef", [
        { start: 1, end: 3 },
        { start: 9, end: 12 },
        { start: 4, end: 6 },
      ]),
    ).toEqual([
      { start: 1, end: 3 },
      { start: 4, end: 6 },
    ]);
  });

  it("sorts, and merges ranges that overlap or touch", () => {
    expect(
      byteRangesToUtf16("abcdefghij", [
        { start: 6, end: 8 },
        { start: 0, end: 2 },
        { start: 1, end: 4 },
        { start: 4, end: 5 },
      ]),
    ).toEqual([
      { start: 0, end: 5 },
      { start: 6, end: 8 },
    ]);
  });

  it("answers nothing for no ranges, and does no work on text with no ranges", () => {
    expect(byteRangesToUtf16("anything é", [])).toEqual([]);
    expect(byteRangesToUtf16("", [{ start: 0, end: 1 }])).toEqual([]);
  });

  it("refuses a line too long to be worth highlighting rather than building a huge table", () => {
    const long = "é".repeat(200_000);
    expect(byteRangesToUtf16(long, [{ start: 0, end: 2 }])).toEqual([]);
  });

  it("does not read ranges beyond a fixed count", () => {
    const many = Array.from({ length: 50_000 }, (_, i) => ({ start: i * 2, end: i * 2 + 1 }));
    const out = byteRangesToUtf16("a".repeat(100_000), many);
    expect(out.length).toBeLessThanOrEqual(10_000);
  });

  it("property: every converted range slices the same characters as the bytes do (10,000 cases)", () => {
    forAll(
      10_000,
      (rng) => {
        const s = text(rng, 24);
        const total = enc.encode(s).length;
        const ranges = Array.from({ length: rng.int(5) }, () => {
          const a = rng.int(total + 2);
          const b = a + rng.int(6);
          return { start: a, end: b };
        });
        return { s, ranges };
      },
      ({ s, ranges }) => {
        const bytes = enc.encode(s);
        const isBoundary = (i: number): boolean => i === 0 || i === bytes.length || ((bytes[i] ?? 0) & 0xc0) !== 0x80;
        const expected = ranges
          .filter((r) => r.start < r.end && r.end <= bytes.length && isBoundary(r.start) && isBoundary(r.end))
          .map((r) => ({ start: r.start, end: r.end }));
        // The reference text of each kept range, by decoding the bytes.
        const wanted = expected.map((r) => dec.decode(bytes.slice(r.start, r.end)));

        const got = byteRangesToUtf16(s, ranges);
        // Every converted range is inside the string, ordered, and apart from its neighbours.
        let previousEnd = -1;
        for (const g of got) {
          expect(g.start).toBeGreaterThanOrEqual(0);
          expect(g.end).toBeLessThanOrEqual(s.length);
          expect(g.start).toBeLessThan(g.end);
          expect(g.start).toBeGreaterThan(previousEnd);
          previousEnd = g.end;
        }
        // Each wanted piece of text lies inside some converted range, and the converted ranges hold nothing else.
        const covered = got.map((g) => s.slice(g.start, g.end));
        for (const piece of wanted) expect(covered.some((c) => c.includes(piece)), piece).toBe(true);
        expect(covered.join("").length).toBeLessThanOrEqual(wanted.reduce((n, w) => n + w.length, 0));
      },
    );
  }, 30_000);

  it("property: a range of whole characters round-trips exactly (10,000 cases)", () => {
    forAll(
      10_000,
      (rng) => {
        const chars = Array.from(text(rng, 20));
        const from = rng.int(chars.length + 1);
        const to = from + rng.int(chars.length - from + 1);
        return { chars, from, to };
      },
      ({ chars, from, to }) => {
        if (from === to) return;
        const s = chars.join("");
        const start = enc.encode(chars.slice(0, from).join("")).length;
        const end = enc.encode(chars.slice(0, to).join("")).length;
        const got = byteRangesToUtf16(s, [{ start, end }]);
        expect(got).toEqual([{ start: chars.slice(0, from).join("").length, end: chars.slice(0, to).join("").length }]);
        expect(s.slice(got[0]?.start, got[0]?.end)).toBe(chars.slice(from, to).join(""));
      },
    );
  }, 30_000);

  it("property: it never throws, whatever the input (10,000 cases)", () => {
    forAll(
      10_000,
      (rng) => {
        const s = text(rng, 16) + (rng.bool() ? "\ud800" : "") + text(rng, 4);
        const ranges = Array.from({ length: rng.int(6) }, () => ({ start: rng.int(80) - 8, end: rng.int(80) - 8 }));
        return { s, ranges };
      },
      ({ s, ranges }) => {
        expect(() => byteRangesToUtf16(s, ranges)).not.toThrow();
      },
    );
  }, 30_000);
});
