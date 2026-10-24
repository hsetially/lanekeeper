import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { highlight, score } from "./match";

// The palette's own filter (cmdk's is switched off, because some results come from the server and must not be filtered
// again): a prefix beats the start of a word beats a match inside a word, case does not matter, and what matched can be drawn.

describe("score", () => {
  it("is 0 for no match and for an empty haystack", () => {
    expect(score("zzz", "tx-infinity-core")).toBe(0);
    expect(score("core", "")).toBe(0);
  });

  it("ranks a prefix above a word start above a match inside a word", () => {
    const prefix = score("core", "core settings");
    const wordStart = score("core", "tx-infinity-core.yml");
    const inside = score("core", "scorecard");
    expect(prefix).toBeGreaterThan(wordStart);
    expect(wordStart).toBeGreaterThan(inside);
    expect(inside).toBeGreaterThan(0);
  });

  it("ignores case, and treats an empty query as matching everything equally", () => {
    expect(score("CORE", "core")).toBe(score("core", "CORE"));
    expect(score("", "anything")).toBe(1);
  });

  it("prefers a shorter text for the same kind of match", () => {
    expect(score("sit", "sit")).toBeGreaterThan(score("sit", "sit-with-a-very-long-name"));
  });

  it("property: a text always matches itself and any piece of itself", () => {
    forAll(
      300,
      (rng) => {
        const text = rng.string("abcdef-_. 0123", 24) || "a";
        const start = rng.int(text.length);
        return { text, piece: text.slice(start, start + 1 + rng.int(6)) };
      },
      ({ text, piece }) => {
        if (piece.trim() === "") return;
        expect(score(piece, text)).toBeGreaterThan(0);
      },
    );
  });
});

describe("highlight", () => {
  it("splits the text around the first match, keeping the original case", () => {
    expect(highlight("core", "tx-infinity-Core.yml")).toEqual({ before: "tx-infinity-", match: "Core", after: ".yml" });
  });

  it("is the whole text, unmarked, when nothing matches or the query is empty", () => {
    expect(highlight("zzz", "abc")).toEqual({ before: "abc", match: "", after: "" });
    expect(highlight("", "abc")).toEqual({ before: "abc", match: "", after: "" });
  });

  it("property: the three parts always join back to the text", () => {
    forAll(
      300,
      (rng) => ({ q: rng.string("abc", 3), text: rng.string("abcABC -", 20) }),
      ({ q, text }) => {
        const h = highlight(q, text);
        expect(h.before + h.match + h.after).toBe(text);
      },
    );
  });
});
