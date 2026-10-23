import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { MAX_MARKERS, MAX_SCAN_CHARS, parseMarker, splitMarkers } from "./marker";

// The marker is the contract's own: `[REDACTED: N chars, rule]` (S14b), `^\[REDACTED: [0-9]+ chars, [a-z0-9_]+\]$` as a whole value.
// The web never makes one; it only recognises what the server sent, and anything that is not exactly that shape is ordinary text.

describe("parseMarker", () => {
  it("reads the length and the rule of a whole marker", () => {
    expect(parseMarker("[REDACTED: 22 chars, generic_secret]")).toEqual({ chars: 22, rule: "generic_secret" });
    expect(parseMarker("[REDACTED: 0 chars, a1]")).toEqual({ chars: 0, rule: "a1" });
  });

  it("refuses anything that is not exactly a marker", () => {
    for (const text of [
      "",
      "[REDACTED: 22 chars, generic_secret] ",
      " [REDACTED: 22 chars, generic_secret]",
      "[REDACTED: 22 chars, generic_secret]\n",
      "[REDACTED: 22 chars, generic_secret]\r",
      "[redacted: 22 chars, generic_secret]",
      "[REDACTED: -1 chars, x]",
      "[REDACTED: 2.5 chars, x]",
      "[REDACTED: 22 chars, Generic]",
      "[REDACTED: 22 chars, a-b]",
      "[REDACTED: 22 chars, ]",
      "[REDACTED: ٢٢ chars, x]",
      "[REDACTED: 22 chars, x]]",
      "x[REDACTED: 22 chars, x]",
    ]) {
      expect(parseMarker(text), JSON.stringify(text)).toBeNull();
    }
  });

  it("does not treat a length it cannot hold as a number", () => {
    expect(parseMarker(`[REDACTED: ${"9".repeat(40)} chars, x]`)).toEqual({ chars: null, rule: "x" });
  });
});

describe("splitMarkers", () => {
  it("returns text with no marker as one part", () => {
    expect(splitMarkers("port: 8080")).toEqual([{ kind: "text", text: "port: 8080" }]);
    expect(splitMarkers("")).toEqual([]);
  });

  it("splits a text around its markers and keeps every character", () => {
    const parts = splitMarkers("a: [REDACTED: 4 chars, k] b: [REDACTED: 9 chars, jwt] c");
    expect(parts.map((p) => p.kind)).toEqual(["text", "marker", "text", "marker", "text"]);
    expect(parts.map((p) => p.text).join("")).toBe("a: [REDACTED: 4 chars, k] b: [REDACTED: 9 chars, jwt] c");
    expect(parts[1]).toMatchObject({ kind: "marker", info: { chars: 4, rule: "k" } });
  });

  it("recognises a marker at the start, at the end and on its own", () => {
    expect(splitMarkers("[REDACTED: 1 chars, a]").map((p) => p.kind)).toEqual(["marker"]);
    expect(splitMarkers("x[REDACTED: 1 chars, a]").map((p) => p.kind)).toEqual(["text", "marker"]);
    expect(splitMarkers("[REDACTED: 1 chars, a]x").map((p) => p.kind)).toEqual(["marker", "text"]);
  });

  it("does not take a near miss for a marker", () => {
    const text = "[REDACTED: 1 chars, A] [REDACTED:1 chars, a] [REDACTED: 1 char, a] [REDACTED: x chars, a]";
    expect(splitMarkers(text)).toEqual([{ kind: "text", text }]);
  });

  it("keeps hostile text around a marker as text", () => {
    const parts = splitMarkers("\u202eevil\u200b[REDACTED: 3 chars, x]\u0000tail");
    expect(parts.map((p) => p.kind)).toEqual(["text", "marker", "text"]);
    expect(parts[0]?.text).toBe("\u202eevil\u200b");
    expect(parts[2]?.text).toBe("\u0000tail");
  });

  it("reads at most MAX_MARKERS markers and leaves the rest as text, so no marker is ever lost from the line", () => {
    const one = "[REDACTED: 1 chars, a]";
    const text = one.repeat(MAX_MARKERS + 5);
    const parts = splitMarkers(text);
    expect(parts.filter((p) => p.kind === "marker")).toHaveLength(MAX_MARKERS);
    expect(parts.map((p) => p.text).join("")).toBe(text);
  });

  it("does not scan a text past MAX_SCAN_CHARS (it comes back whole, as text)", () => {
    const text = `${"a".repeat(MAX_SCAN_CHARS)}[REDACTED: 1 chars, a]`;
    expect(splitMarkers(text)).toEqual([{ kind: "text", text }]);
  });

  it("is linear on text that nearly matches (no backtracking blow-up)", () => {
    const text = "[REDACTED: ".repeat(20_000) + "9".repeat(50_000);
    const started = performance.now();
    splitMarkers(text.slice(0, MAX_SCAN_CHARS));
    expect(performance.now() - started).toBeLessThan(1_000);
  });

  it("property: the parts always add up to the text, and every marker part parses", () => {
    const pieces = ["a", " ", "é", "😀", "[REDACTED: 3 chars, x]", "[REDACTED: ", "]", "chars, ", "\n"];
    forAll(
      300,
      (rng) => Array.from({ length: rng.int(12) }, () => rng.pick(pieces)).join(""),
      (text) => {
        const parts = splitMarkers(text);
        expect(parts.map((p) => p.text).join("")).toBe(text);
        for (const part of parts) if (part.kind === "marker") expect(parseMarker(part.text)).toEqual(part.info);
      },
    );
  });
});
