import { describe, expect, it } from "vitest";
import type { Finding, Severity } from "@/api/generated/model";
import { finding } from "@/mocks/design/findings/fixtures";
import { SEVERITIES, SEVERITY_LABEL, compareFindings, severityRank, sortFindings } from "./severity";

// The order of what the server found. The severity, the kind and the swimlane are the server's; this only puts rows in an
// order for the person reading them, within the findings that are loaded.

const LANES = new Map([
  ["sit", 0],
  ["sitb", 1],
  ["sitc", 2],
]);

describe("severity", () => {
  it("runs from critical down to low, and has a label for each", () => {
    expect(SEVERITIES).toEqual(["critical", "high", "medium", "low"]);
    expect(SEVERITIES.map((s) => SEVERITY_LABEL[s])).toEqual(["Critical", "High", "Medium", "Low"]);
    expect(SEVERITIES.map(severityRank)).toEqual([0, 1, 2, 3]);
  });
});

const ids = (rows: readonly Finding[]): number[] => rows.map((f) => f.id);
const sev = (id: number, severity: Severity, over: Partial<Finding> = {}): Finding => finding(id, { severity, ...over });

describe("the default order", () => {
  it("is most severe first, then the swimlane in the order of the swimlane list, then the check and the file, and the newest first", () => {
    const rows = [
      sev(1, "low", { swimlane: "sit" }),
      sev(2, "high", { swimlane: "sitc" }),
      sev(3, "high", { swimlane: "sit", kind: "duplicate_keys" }),
      sev(4, "high", { swimlane: "sit", kind: "missed_base_changes", path: "b.yml" }),
      sev(5, "high", { swimlane: "sit", kind: "missed_base_changes", path: "a.yml" }),
      sev(6, "critical", { swimlane: "sitc" }),
      sev(7, "high", { swimlane: "sit", kind: "missed_base_changes", path: "a.yml" }),
    ];
    expect(ids(sortFindings(rows, null, LANES))).toEqual([6, 7, 5, 4, 3, 2, 1]);
  });

  it("puts a finding with no file after those that have one, and a swimlane the list does not know last", () => {
    const rows = [sev(1, "high", { path: null }), sev(2, "high", { path: "a.yml" }), sev(3, "high", { swimlane: "zzz" }), sev(4, "high", { swimlane: "sit" })];
    expect(ids(sortFindings(rows, null, LANES))).toEqual([4, 2, 1, 3]);
  });

  it("does not change the list it is given", () => {
    const rows = [sev(1, "low"), sev(2, "high")];
    const before = ids(rows);
    sortFindings(rows, null, LANES);
    expect(ids(rows)).toEqual(before);
  });
});

describe("a column the person chose", () => {
  const rows = [
    sev(1, "low", { swimlane: "sitc", kind: "drift", path: "m.yml" }),
    sev(2, "high", { swimlane: "sit", kind: "orphan_file", path: "z.yml" }),
    sev(3, "medium", { swimlane: "sitb", kind: "missed_base_changes", path: "a.yml" }),
  ];

  it("sorts by severity both ways", () => {
    expect(ids(sortFindings(rows, { column: "severity", direction: "desc" }, LANES))).toEqual([2, 3, 1]);
    expect(ids(sortFindings(rows, { column: "severity", direction: "asc" }, LANES))).toEqual([1, 3, 2]);
  });

  it("sorts by the swimlane list's order, not the alphabet", () => {
    expect(ids(sortFindings(rows, { column: "swimlane", direction: "asc" }, LANES))).toEqual([2, 3, 1]);
    expect(ids(sortFindings(rows, { column: "swimlane", direction: "desc" }, LANES))).toEqual([1, 3, 2]);
  });

  it("sorts by the check's number, C1 before C4 before C5", () => {
    expect(ids(sortFindings(rows, { column: "check", direction: "asc" }, LANES))).toEqual([3, 2, 1]);
  });

  it("sorts by the path", () => {
    expect(ids(sortFindings(rows, { column: "file", direction: "asc" }, LANES))).toEqual([3, 1, 2]);
  });

  it("falls back to the default order for a column it does not know", () => {
    expect(ids(sortFindings(rows, { column: "nope", direction: "asc" }, LANES))).toEqual(ids(sortFindings(rows, null, LANES)));
  });

  it("compares rows that are equal in every column by id, so that the order never flickers between two refreshes", () => {
    const a = sev(1, "high");
    const b = sev(2, "high");
    // Newest first, as the server lists them.
    expect(compareFindings(null, LANES)(a, b)).toBeGreaterThan(0);
    expect(compareFindings(null, LANES)(b, a)).toBeLessThan(0);
  });
});
