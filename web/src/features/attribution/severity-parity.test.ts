import { describe, expect, it } from "vitest";
import { Severity, type Finding } from "@/api/generated/model";
import { SEVERITIES, SEVERITY_LABEL, compareSeverity, severityRank, sortBySeverity } from "@/components/attribution/severity";
import { SEVERITIES as FINDINGS_SEVERITIES, SEVERITY_LABEL as FINDINGS_LABEL, severityRank as findingsRank, sortFindings } from "@/features/findings/severity";
import { byUrgency } from "@/features/swimlanes/findings/severity";

// The shared severity helpers carry over what the screens did before there was one: the Findings screen's vocabulary and order
// (the first, the one the design draws) and the swimlane Findings tab's. If the screens re-export the shared ones, these hold
// trivially; if one of them changes its own, they fail. Delete the case of a screen whose local copy has been removed.

const finding = (id: number, severity: Severity): Finding => ({ id, kind: "duplicate_keys", swimlane: "sitb", path: "a.yml", severity, rule_ids: [], message: "m", locations: [], detected_at: 1 });
const ids = (rows: readonly Finding[]) => rows.map((f) => f.id);

describe("the same levels, words and ranks as the Findings screen", () => {
  it("lists the four levels most severe first, with the same words", () => {
    expect(SEVERITIES).toEqual(FINDINGS_SEVERITIES);
    expect(SEVERITY_LABEL).toEqual(FINDINGS_LABEL);
  });

  it("ranks them the same way", () => {
    for (const severity of Object.values(Severity)) expect(severityRank(severity), severity).toBe(findingsRank(severity));
  });
});

describe("the same order of severity as the two screens that sorted findings", () => {
  const rows = [finding(1, "low"), finding(2, "critical"), finding(3, "medium"), finding(4, "high"), finding(5, "critical"), finding(6, "low")];

  it("puts them as the Findings screen's default order does, when only the severity differs (newest first among equals)", () => {
    const mine = sortBySeverity(rows, (f) => f.severity, (a, b) => b.id - a.id);
    expect(ids(mine)).toEqual(ids(sortFindings(rows, null, new Map())));
  });

  it("puts them as the swimlane Findings tab does, when only the severity differs (oldest first among equals)", () => {
    const mine = sortBySeverity(rows, (f) => f.severity, (a, b) => a.id - b.id);
    expect(ids(mine)).toEqual(ids(byUrgency(rows)));
  });

  it("agrees with both for every pair of levels", () => {
    for (const a of Object.values(Severity)) for (const b of Object.values(Severity)) expect(Math.sign(compareSeverity(a, b)), `${a} ${b}`).toBe(Math.sign(findingsRank(a) - findingsRank(b)));
  });
});
