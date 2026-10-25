import { describe, expect, it } from "vitest";
import type { Finding } from "@/api/generated/model";
import { byUrgency, lineLabel } from "./severity";

const finding = (id: number, over: Partial<Finding> = {}): Finding => ({
  id,
  kind: "duplicate_keys",
  swimlane: "sitb",
  path: "tx-infinity-api/core.yml",
  severity: "medium",
  rule_ids: [],
  message: "m",
  locations: [],
  detected_at: 1,
  ...over,
});

describe("byUrgency", () => {
  it("puts the more severe findings first, then the check, then the file", () => {
    const sorted = byUrgency([
      finding(1, { severity: "low" }),
      finding(2, { severity: "critical", kind: "drift" }),
      finding(3, { severity: "high", kind: "redundant_tenant_file", path: "b.yml" }),
      finding(4, { severity: "high", kind: "redundant_tenant_file", path: "a.yml" }),
      finding(5, { severity: "high", kind: "missed_base_changes" }),
    ]);
    expect(sorted.map((f) => f.id)).toEqual([2, 5, 4, 3, 1]);
  });

  it("does not change the list it was given and puts a finding with no file after those with one", () => {
    const input = [finding(1, { path: null }), finding(2)];
    expect(byUrgency(input).map((f) => f.id)).toEqual([2, 1]);
    expect(input.map((f) => f.id)).toEqual([1, 2]);
  });
});

describe("lineLabel", () => {
  it("writes one line as L212 and a range with a dash", () => {
    expect(lineLabel({ start: 212, end: 212 })).toBe("L212");
    expect(lineLabel({ start: 12, end: 14 })).toBe("L12–14");
  });
});
