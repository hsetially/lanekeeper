import { describe, expect, it } from "vitest";
import { codeFilter } from "./filter";

describe("codeFilter (what the address narrows the Findings tab to)", () => {
  it("is nothing when the address says nothing", () => {
    expect(codeFilter({})).toEqual({ kinds: undefined, minSeverity: undefined, label: null });
  });

  it("reads a check by its code, C1 to C12", () => {
    expect(codeFilter({ code: "C8" })).toEqual({ kinds: ["duplicate_keys"], minSeverity: undefined, label: "C8, duplicate keys" });
    expect(codeFilter({ code: "c12" }).kinds).toEqual(["served_mismatch"]);
  });

  it("reads a check by its name", () => {
    expect(codeFilter({ kind: "drift" }).kinds).toEqual(["drift"]);
  });

  it("reads a severity as that severity and above", () => {
    expect(codeFilter({ severity: "high" })).toEqual({ kinds: undefined, minSeverity: "high", label: "high severity and above" });
  });

  it("combines them", () => {
    expect(codeFilter({ code: "C2", severity: "medium" }).label).toBe("C2, redundant tenant file, medium severity and above");
  });

  it("ignores a value it does not know instead of guessing", () => {
    expect(codeFilter({ code: "C99", kind: "nope", severity: "urgent" })).toEqual({ kinds: undefined, minSeverity: undefined, label: null });
    expect(codeFilter({ code: "__proto__" }).kinds).toBeUndefined();
  });
});
