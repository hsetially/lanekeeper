import { describe, expect, it } from "vitest";

describe("jsdom test setup", () => {
  it("runs in jsdom with jest-dom matchers", () => {
    const el = document.createElement("div");
    el.textContent = "ok";
    document.body.append(el);
    expect(el).toBeInTheDocument();
  });

  it("gives elements a fixed box for virtualisers, overridable by data attributes", () => {
    const plain = document.createElement("div");
    expect(plain.getBoundingClientRect().height).toBe(768);
    const small = document.createElement("div");
    small.dataset["testHeight"] = "200";
    small.dataset["testWidth"] = "300";
    expect(small.getBoundingClientRect()).toMatchObject({ width: 300, height: 200 });
    expect(small.offsetHeight).toBe(200);
  });

  it("stubs the browser APIs jsdom lacks", () => {
    expect(typeof ResizeObserver).toBe("function");
    expect(window.matchMedia("(prefers-reduced-motion: reduce)").matches).toBe(false);
    expect(() => document.body.scrollIntoView()).not.toThrow();
    expect(document.body.hasPointerCapture(1)).toBe(false);
  });
});
