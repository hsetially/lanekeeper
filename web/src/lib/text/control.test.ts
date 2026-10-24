import { describe, expect, it } from "vitest";
import { productSources, withoutComments } from "@/test/source-files";
import { hasControlChar, hasControlOrBackslash, hasHiddenOrBackslash } from "./control";

// What does not belong in a path, an id or a name (S8, S11): the app asks the question in three strengths, and each is written
// once. These tables are the whole behaviour, so a screen that moves from its own loop to the shared one cannot change what it
// accepts without one of them failing.

const SAMPLES: Record<string, string> = {
  letter: "a",
  space: " ",
  tab: "\t",
  "line feed": "\n",
  NUL: "\u0000",
  "last C0": "\u001f",
  DEL: "\u007f",
  "first C1": "\u0080",
  "last C1": "\u009f",
  "no-break space": " ",
  "soft hyphen": "­",
  backslash: "\\",
  "zero-width space": "​",
  "bidi override": "‮",
  "line separator": " ",
  "paragraph separator": " ",
  emoji: "😀",
  "slash and dot": "/..",
};

const EXPECT: Record<string, { controlChar: boolean; controlOrBackslash: boolean; hiddenOrBackslash: boolean }> = {
  letter: { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: false },
  space: { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: false },
  tab: { controlChar: true, controlOrBackslash: true, hiddenOrBackslash: true },
  "line feed": { controlChar: true, controlOrBackslash: true, hiddenOrBackslash: true },
  NUL: { controlChar: true, controlOrBackslash: true, hiddenOrBackslash: true },
  "last C0": { controlChar: true, controlOrBackslash: true, hiddenOrBackslash: true },
  DEL: { controlChar: true, controlOrBackslash: true, hiddenOrBackslash: true },
  "first C1": { controlChar: false, controlOrBackslash: true, hiddenOrBackslash: true },
  "last C1": { controlChar: false, controlOrBackslash: true, hiddenOrBackslash: true },
  "no-break space": { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: false },
  "soft hyphen": { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: true },
  backslash: { controlChar: false, controlOrBackslash: true, hiddenOrBackslash: true },
  "zero-width space": { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: true },
  "bidi override": { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: true },
  "line separator": { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: true },
  "paragraph separator": { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: true },
  emoji: { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: false },
  "slash and dot": { controlChar: false, controlOrBackslash: false, hiddenOrBackslash: false },
};

describe("the three strengths of 'does not belong in a path'", () => {
  it("has a row for every sample", () => {
    expect(Object.keys(EXPECT).sort()).toEqual(Object.keys(SAMPLES).sort());
  });

  describe.each(Object.keys(SAMPLES))("%s", (name) => {
    const sample = SAMPLES[name] ?? "";
    const expected = EXPECT[name];
    it("is judged by hasControlChar (C0 and DEL)", () => {
      expect(hasControlChar(sample)).toBe(expected?.controlChar);
      expect(hasControlChar(`ok${sample}ok`)).toBe(expected?.controlChar);
    });
    it("is judged by hasControlOrBackslash (C0, DEL, C1 and the backslash)", () => {
      expect(hasControlOrBackslash(sample)).toBe(expected?.controlOrBackslash);
      expect(hasControlOrBackslash(`ok${sample}ok`)).toBe(expected?.controlOrBackslash);
    });
    it("is judged by hasHiddenOrBackslash (controls, format characters, line and paragraph separators and the backslash)", () => {
      expect(hasHiddenOrBackslash(sample)).toBe(expected?.hiddenOrBackslash);
      expect(hasHiddenOrBackslash(`ok${sample}ok`)).toBe(expected?.hiddenOrBackslash);
    });
  });

  it("answers false for the empty text", () => {
    expect(hasControlChar("")).toBe(false);
    expect(hasControlOrBackslash("")).toBe(false);
    expect(hasHiddenOrBackslash("")).toBe(false);
  });
});

describe("one definition", () => {
  it("keeps the code-point loops for C0, DEL and C1 in this module only", () => {
    // A screen with its own `code < 0x20 || code === 0x7f` loop can drift from this one (and did: four copies, three strengths).
    const holders = productSources().filter(({ text }) => /0x7f\b/i.test(withoutComments(text))).map((f) => f.path);
    expect(holders).toEqual(["src/lib/text/control.ts"]);
  });

  it("keeps the class of hidden characters in this module only", () => {
    const holders = productSources().filter(({ text }) => /\\p\{Cf\}/.test(withoutComments(text)) && /\\p\{Zl\}/.test(withoutComments(text))).map((f) => f.path);
    // The display side (safe-text, SafeCell) marks surrogates and more; it is a different question, answered where the text is drawn.
    expect(holders.filter((path) => !/safe-text|SafeCell|support-details/.test(path))).toEqual(["src/lib/text/control.ts"]);
  });
});
