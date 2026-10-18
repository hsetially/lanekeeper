import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { matchesStep, parseStep, type KeyLike } from "./keys";

const key = (k: string, mods: Partial<Omit<KeyLike, "key">> = {}): KeyLike => ({ key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods });

describe("parseStep", () => {
  it("reads the steps the handoff's keymap uses", () => {
    expect(parseStep("ControlOrMeta+k")).toEqual({ key: "k", ctrlOrMeta: true, shift: false, alt: false });
    expect(parseStep("Shift+Tab")).toEqual({ key: "Tab", ctrlOrMeta: false, shift: true, alt: false });
    expect(parseStep("Escape")).toEqual({ key: "Escape", ctrlOrMeta: false, shift: false, alt: false });
    expect(parseStep("Space")).toEqual({ key: " ", ctrlOrMeta: false, shift: false, alt: false });
    expect(parseStep("ControlOrMeta+Enter")).toMatchObject({ key: "Enter", ctrlOrMeta: true });
    expect(parseStep("/")).toMatchObject({ key: "/" });
    expect(parseStep("?")).toMatchObject({ key: "?" });
  });

  it("refuses a step it cannot read, so a typo in a row cannot become a silent no-op", () => {
    expect(() => parseStep("")).toThrow();
    expect(() => parseStep("Hyper+k")).toThrow();
    expect(() => parseStep("ControlOrMeta+")).toThrow();
  });
});

describe("matchesStep", () => {
  it("matches a letter whatever the Caps Lock state, and never with another modifier", () => {
    const g = parseStep("g");
    expect(matchesStep(g, key("g"))).toBe(true);
    expect(matchesStep(g, key("G"))).toBe(true);
    expect(matchesStep(g, key("g", { ctrlKey: true }))).toBe(false);
    expect(matchesStep(g, key("g", { metaKey: true }))).toBe(false);
    expect(matchesStep(g, key("g", { altKey: true }))).toBe(false);
    expect(matchesStep(g, key("s"))).toBe(false);
  });

  it("does not treat Shift+g as g: a capital letter is another shortcut", () => {
    expect(matchesStep(parseStep("g"), key("G", { shiftKey: true }))).toBe(false);
  });

  it("matches a punctuation key by what it types, with Shift as the layout needs it", () => {
    expect(matchesStep(parseStep("?"), key("?", { shiftKey: true }))).toBe(true);
    expect(matchesStep(parseStep("/"), key("/"))).toBe(true);
    expect(matchesStep(parseStep("/"), key("/", { ctrlKey: true }))).toBe(false);
  });

  it("takes Control or Command for ControlOrMeta, and not Alt", () => {
    const k = parseStep("ControlOrMeta+k");
    expect(matchesStep(k, key("k", { ctrlKey: true }))).toBe(true);
    expect(matchesStep(k, key("k", { metaKey: true }))).toBe(true);
    expect(matchesStep(k, key("K", { metaKey: true }))).toBe(true);
    expect(matchesStep(k, key("k"))).toBe(false);
    expect(matchesStep(k, key("k", { ctrlKey: true, altKey: true }))).toBe(false);
    expect(matchesStep(k, key("k", { ctrlKey: true, shiftKey: true }))).toBe(false);
  });

  it("matches Space by its key value, Shift+Tab with Shift, and a named key exactly", () => {
    expect(matchesStep(parseStep("Space"), key(" "))).toBe(true);
    expect(matchesStep(parseStep("Shift+Tab"), key("Tab", { shiftKey: true }))).toBe(true);
    expect(matchesStep(parseStep("Shift+Tab"), key("Tab"))).toBe(false);
    expect(matchesStep(parseStep("Tab"), key("Tab", { shiftKey: true }))).toBe(false);
    expect(matchesStep(parseStep("Enter"), key("Enter"))).toBe(true);
    expect(matchesStep(parseStep("Escape"), key("Escape"))).toBe(true);
  });

  it("property: a step never matches a key that differs from it in the key itself", () => {
    const names = ["a", "g", "s", "k", "?", "/", "Escape", "Enter", "ArrowUp"] as const;
    forAll(
      500,
      (rng) => ({ a: rng.pick(names), b: rng.pick(names) }),
      ({ a, b }) => {
        if (a === b) return;
        expect(matchesStep(parseStep(a), key(b, { shiftKey: b === "?" }))).toBe(false);
      },
    );
  });
});
