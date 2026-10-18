import { describe, expect, it } from "vitest";
import { DriftState } from "@/api/generated/model";
import { DRIFT_VARIANT, driftSummary } from "./drift";
import { STATUS_VARIANTS } from "./StatusBadge";

// How the API's drift counts of a swimlane read as one badge: a variant of StatusBadge and a count. Used by the swimlane
// switcher in the header and by the swimlane screens.

describe("DRIFT_VARIANT", () => {
  it("maps every drift state to a badge variant that exists", () => {
    for (const state of Object.values(DriftState)) expect(STATUS_VARIANTS[DRIFT_VARIANT[state]], state).toBeDefined();
    expect(DRIFT_VARIANT.nfs_ahead).toBe("nfs");
    expect(DRIFT_VARIANT.git_ahead).toBe("git");
    expect(DRIFT_VARIANT.in_sync).toBe("sync");
  });
});

describe("driftSummary", () => {
  it("is in sync when nothing needs attention, whatever else is counted", () => {
    expect(driftSummary([])).toEqual({ total: 0, variant: "sync", word: "In sync" });
    expect(driftSummary([[DriftState.in_sync, 400], [DriftState.intentional_divergence, 3]])).toEqual({ total: 0, variant: "sync", word: "In sync" });
  });

  it("counts every state that needs attention, and names the most urgent one", () => {
    expect(driftSummary([[DriftState.conflict, 1], [DriftState.nfs_ahead, 2], [DriftState.git_ahead, 1]])).toEqual({ total: 4, variant: "conflict", word: "drift" });
    expect(driftSummary([[DriftState.git_ahead, 2], [DriftState.nfs_ahead, 1]])).toMatchObject({ total: 3, variant: "nfs" });
    expect(driftSummary([[DriftState.git_ahead, 2]])).toMatchObject({ total: 2, variant: "git", word: "drift" });
  });

  it("says unknown, not drift, when the only thing is files whose state is not known (an unadopted swimlane)", () => {
    expect(driftSummary([[DriftState.unknown, 412]])).toEqual({ total: 412, variant: "unknown", word: "unknown" });
    expect(driftSummary([[DriftState.unknown, 412], [DriftState.in_sync, 3]])).toMatchObject({ word: "unknown" });
    expect(driftSummary([[DriftState.unknown, 4], [DriftState.nfs_ahead, 1]])).toMatchObject({ word: "drift", variant: "nfs" });
  });

  it("ignores a count that is not a positive whole number", () => {
    expect(driftSummary([[DriftState.nfs_ahead, 0], [DriftState.git_ahead, -2], [DriftState.conflict, 1.5]])).toMatchObject({ total: 0 });
  });
});
