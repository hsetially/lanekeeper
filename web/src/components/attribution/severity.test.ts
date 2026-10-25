import { describe, expect, it } from "vitest";
import { Severity } from "@/api/generated/model";
import { MIN_SEVERITY_CHOICES, SEVERITIES, SEVERITY_LABEL, atLeast, compareSeverity, filterAtLeast, loadedScopeNote, minSeverityLabel, minSeverityParam, parseSeverity, severityRank, sortBySeverity } from "./severity";

// Severity as the person reads it: four levels, most severe first, in the words the Findings screen uses. DISPLAY ONLY: a
// severity is the server's (the registry rates a change or a finding by the engine's rules and the admin's, D76, D103), and the
// browser never rates or re-rates one. What is here puts rows in an order, keeps the rows at or above a level, and says plainly
// when it can only do that for the rows that are loaded.

describe("the four levels", () => {
  it("run from critical down to low, with a word each, and cover the contract's levels and no other", () => {
    expect(SEVERITIES).toEqual(["critical", "high", "medium", "low"]);
    expect(SEVERITIES.map((s) => SEVERITY_LABEL[s])).toEqual(["Critical", "High", "Medium", "Low"]);
    expect(SEVERITIES.map(severityRank)).toEqual([0, 1, 2, 3]);
    expect([...SEVERITIES].sort()).toEqual(Object.values(Severity).sort());
  });
});

describe("compareSeverity", () => {
  it("puts the more severe first and a row nobody rated last", () => {
    expect(compareSeverity("critical", "high")).toBeLessThan(0);
    expect(compareSeverity("low", "medium")).toBeGreaterThan(0);
    expect(compareSeverity("high", "high")).toBe(0);
    expect(compareSeverity(null, "low")).toBeGreaterThan(0);
    expect(compareSeverity("low", undefined)).toBeLessThan(0);
    expect(compareSeverity(null, undefined)).toBe(0);
  });
});

type Row = { id: number; severity: Severity | null };
const row = (id: number, severity: Severity | null): Row => ({ id, severity });
const ids = (rows: readonly Row[]) => rows.map((r) => r.id);
const severityOf = (r: Row) => r.severity;

describe("sortBySeverity", () => {
  const rows = [row(1, "low"), row(2, null), row(3, "critical"), row(4, "medium"), row(5, "high"), row(6, "high")];

  it("orders the most severe first and the unrated last, keeping the order of rows that tie", () => {
    expect(ids(sortBySeverity(rows, severityOf))).toEqual([3, 5, 6, 4, 1, 2]);
  });

  it("breaks a tie with the caller's order, so a list keeps one order between two refreshes", () => {
    expect(ids(sortBySeverity(rows, severityOf, (a, b) => b.id - a.id))).toEqual([3, 6, 5, 4, 1, 2]);
  });

  it("returns a new array and leaves the one it was given alone", () => {
    const before = ids(rows);
    const out = sortBySeverity(rows, severityOf);
    expect(out).not.toBe(rows);
    expect(ids(rows)).toEqual(before);
  });

  it("is a sort of the rows it has: the same rows come back", () => {
    expect(ids(sortBySeverity(rows, severityOf)).sort()).toEqual(ids(rows).sort());
  });
});

describe("atLeast and filterAtLeast: that level and above", () => {
  const rows = [row(1, "low"), row(2, null), row(3, "critical"), row(4, "medium"), row(5, "high")];

  it("keeps the level asked for and everything more severe", () => {
    expect(atLeast("high", "high")).toBe(true);
    expect(atLeast("critical", "high")).toBe(true);
    expect(atLeast("medium", "high")).toBe(false);
    expect(ids(filterAtLeast(rows, severityOf, "medium"))).toEqual([3, 4, 5]);
    expect(ids(filterAtLeast(rows, severityOf, "high"))).toEqual([3, 5]);
    expect(ids(filterAtLeast(rows, severityOf, "critical"))).toEqual([3]);
  });

  it("does not guess a level for a row nobody rated: a row with no severity is not at any level", () => {
    expect(atLeast(null, "low")).toBe(false);
    expect(atLeast(undefined, "low")).toBe(false);
    expect(ids(filterAtLeast(rows, severityOf, "low"))).toEqual([1, 3, 4, 5]);
  });

  it("is no filter at all, and the same rows, when no level is asked for", () => {
    expect(filterAtLeast(rows, severityOf, null)).toBe(rows);
    expect(filterAtLeast(rows, severityOf, undefined)).toBe(rows);
  });
});

describe("the choices of the filter", () => {
  it("are Any, Medium and above, High and above, and Critical only: the Findings screen's menu", () => {
    expect(MIN_SEVERITY_CHOICES).toEqual([
      { value: "any", label: "Any severity" },
      { value: "medium", label: "Medium and above" },
      { value: "high", label: "High and above" },
      { value: "critical", label: "Critical only" },
    ]);
  });

  it("have a label for a level, and for none", () => {
    expect(minSeverityLabel("high")).toBe("High and above");
    expect(minSeverityLabel("critical")).toBe("Critical only");
    expect(minSeverityLabel("low")).toBe("Low and above");
    expect(minSeverityLabel(null)).toBe("Any severity");
  });
});

describe("parseSeverity and minSeverityParam", () => {
  it("reads a level from an address and nothing else", () => {
    expect(parseSeverity("high")).toBe("high");
    expect(parseSeverity("critical")).toBe("critical");
    for (const bad of ["High", "urgent", "", " high", "high ", "constructor", "__proto__", undefined, null, 3, {}, ["high"]]) expect(parseSeverity(bad), typeof bad).toBeUndefined();
  });

  it("asks the server for `min_severity` when there is a level, and for nothing otherwise", () => {
    expect(minSeverityParam("high")).toEqual({ min_severity: "high" });
    expect(minSeverityParam(null)).toEqual({});
    expect(minSeverityParam(undefined)).toEqual({});
  });
});

describe("loadedScopeNote: what a filter or a sort can only do to the rows that are here", () => {
  it("says nothing when every row is loaded: the filter is the whole answer", () => {
    expect(loadedScopeNote({ what: "filter", count: 12, hasMore: false })).toBeNull();
    expect(loadedScopeNote({ what: "sort", count: 12, hasMore: false })).toBeNull();
  });

  it("says that it applies to the loaded rows when there are more on the server", () => {
    expect(loadedScopeNote({ what: "filter", count: 50, hasMore: true })).toBe("Applies to the 50 rows loaded so far. More are not loaded.");
    expect(loadedScopeNote({ what: "sort", count: 50, hasMore: true })).toBe("Sorted within the 50 rows loaded so far. More are not loaded.");
  });

  it("uses the singular for one, and names what the rows are", () => {
    expect(loadedScopeNote({ what: "filter", count: 1, hasMore: true, noun: "file" })).toBe("Applies to the 1 file loaded so far. More are not loaded.");
    expect(loadedScopeNote({ what: "sort", count: 3, hasMore: true, noun: "file" })).toBe("Sorted within the 3 files loaded so far. More are not loaded.");
  });
});
