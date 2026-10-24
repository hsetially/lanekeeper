import { describe, expect, it } from "vitest";
import { checkCopyable, isMaskedValue } from "./copy-text";

describe("checkCopyable", () => {
  it("accepts an ordinary value and says nothing is hidden in it", () => {
    expect(checkCopyable("tx-infinity-api/core-sit1.yml")).toEqual({ ok: true, hidden: false });
    expect(checkCopyable("with\ttab and ünïcode")).toEqual({ ok: true, hidden: false });
  });

  it("refuses nothing, a line break, and every control character in a single-line value", () => {
    expect(checkCopyable("").ok).toBe(false);
    for (const bad of ["a\nb", "a\rb", "a\u0000b", "a\u001bb", "a\u007fb", "a\u0085b", "a b", "a b"]) expect(checkCopyable(bad).ok, JSON.stringify(bad)).toBe(false);
  });

  it("allows line breaks, and only line breaks, in text", () => {
    expect(checkCopyable("one\ntwo\r\nthree", { multiline: true })).toEqual({ ok: true, hidden: false });
    expect(checkCopyable("one\u001btwo", { multiline: true }).ok).toBe(false);
    expect(checkCopyable("one two", { multiline: true }).ok).toBe(false);
  });

  it("copies a value with a bidi or zero-width character and says it has one", () => {
    for (const odd of ["a‮b", "a​b", "a⁦b", "a­b", "aㅤb"]) expect(checkCopyable(odd), JSON.stringify(odd)).toEqual({ ok: true, hidden: true });
    expect(checkCopyable("a‮b\nc", { multiline: true })).toEqual({ ok: true, hidden: true });
  });
});

describe("isMaskedValue", () => {
  it("is true for the marker and only for the marker", () => {
    expect(isMaskedValue("[REDACTED: 12 chars, github_pat]")).toBe(true);
    expect(isMaskedValue("[REDACTED: 12 chars, github_pat] and more")).toBe(false);
    expect(isMaskedValue("prefix [REDACTED: 12 chars, github_pat]")).toBe(false);
    expect(isMaskedValue("REDACTED")).toBe(false);
    expect(isMaskedValue("")).toBe(false);
  });
});
