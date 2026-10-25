import { describe, expect, it } from "vitest";
import { fallbackName, firstName } from "./people";

describe("what a person is called", () => {
  it("falls back to the start of the object part of their id", () => {
    expect(fallbackName("11111111-1111-4111-8111-111111111111:abcdef12-0000-4000-8000-000000000000")).toBe("user abcdef12");
    expect(fallbackName("no-colon-here")).toBe("user no-colon");
  });

  it("takes the first word of a name, or the name itself when there is none", () => {
    expect(firstName("Priya Shah")).toBe("Priya");
    expect(firstName("  Tomás   Reyes ")).toBe("Tomás");
    expect(firstName("")).toBe("");
  });
});
