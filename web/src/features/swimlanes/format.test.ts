import { describe, expect, it } from "vitest";
import { FindingKind, type DriftState, type VersionLabel } from "@/api/generated/model";
import { codeOf } from "@/features/findings/kinds";
import { driftChips, findingTitle, versionLabel } from "./format";

const commit = "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c";

describe("versionLabel", () => {
  it("writes the tag, and how far past it the baseline is", () => {
    expect(versionLabel({ tag: "v2.3.1", distance: 4, commit })).toBe("v2.3.1 + 4");
    expect(versionLabel({ tag: "v2.3.1", distance: 0, commit })).toBe("v2.3.1");
  });

  it("falls back to the short commit when there is no tag", () => {
    expect(versionLabel({ tag: null, distance: 0, commit } satisfies VersionLabel)).toBe("a3f9c21");
  });

  it("is a dash when there is no base version", () => {
    expect(versionLabel(null)).toBe("—");
    expect(versionLabel(undefined)).toBe("—");
  });
});

describe("driftChips", () => {
  it("lists the states that need a look, most urgent first, and leaves out in sync and zero", () => {
    const counts: [DriftState, number][] = [
      ["in_sync", 400],
      ["git_ahead", 1],
      ["nfs_ahead", 2],
      ["conflict", 1],
      ["unknown", 0],
    ];
    expect(driftChips(counts)).toEqual([
      { state: "conflict", count: 1 },
      { state: "nfs_ahead", count: 2 },
      { state: "git_ahead", count: 1 },
    ]);
  });

  it("adds up a state listed twice and keeps untracked and intentional divergences after the others", () => {
    expect(
      driftChips([
        ["intentional_divergence", 3],
        ["untracked", 2],
        ["unknown", 412],
        ["unknown", 8],
      ]),
    ).toEqual([
      { state: "unknown", count: 420 },
      { state: "untracked", count: 2 },
      { state: "intentional_divergence", count: 3 },
    ]);
  });

  it("is empty for a swimlane that is in sync", () => {
    expect(driftChips([["in_sync", 12]])).toEqual([]);
    expect(driftChips([])).toEqual([]);
  });
});

describe("finding codes", () => {
  it("numbers the twelve kinds C1 to C12 in the contract's order (the Findings screen's table, which the swimlane's Findings tab uses too)", () => {
    expect(Object.values(FindingKind).map((kind) => codeOf(kind))).toEqual(["C1", "C2", "C3", "C4", "C5", "C6", "C7", "C8", "C9", "C10", "C11", "C12"]);
  });

  it("has words for each kind", () => {
    for (const kind of Object.values(FindingKind)) expect(findingTitle(kind).length).toBeGreaterThan(3);
    expect(findingTitle("duplicate_keys")).toBe("Duplicate keys");
  });
});
