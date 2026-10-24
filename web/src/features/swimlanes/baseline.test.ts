import { describe, expect, it } from "vitest";
import type { BaselineStatus } from "@/api/generated/model";
import { baselineView, editLock, lockedExplanation } from "./baseline";

const BY = { id: "t:u", display_name: "Priya Shah" };
const confirmed: BaselineStatus = { state: "confirmed", by: BY, at: 1_760_000_000_000 };
const unconfirmed: BaselineStatus = { state: "unconfirmed", by: null, at: null };
const inProgress: BaselineStatus = { state: "in_progress", by: null, at: null };

describe("baselineView", () => {
  it("reads the three states of the contract", () => {
    expect(baselineView(confirmed)).toEqual({ kind: "confirmed", by: "Priya Shah", at: 1_760_000_000_000 });
    expect(baselineView(unconfirmed)).toEqual({ kind: "unconfirmed" });
    expect(baselineView(inProgress)).toEqual({ kind: "in_progress" });
  });

  it("says unknown, and not confirmed, for a server that predates the field", () => {
    expect(baselineView(undefined)).toEqual({ kind: "unknown" });
    expect(baselineView(null)).toEqual({ kind: "unknown" });
  });

  it("names nobody when a confirmed baseline has no person", () => {
    expect(baselineView({ state: "confirmed", by: null, at: 5 })).toEqual({ kind: "confirmed", by: null, at: 5 });
  });
});

describe("editLock", () => {
  it("is locked while the baseline is unconfirmed, and says what to do", () => {
    expect(editLock("presitc", unconfirmed)).toEqual({
      kind: "unconfirmed",
      title: "Editing locked",
      text: "presitc has no confirmed baseline. An admin must finish adoption first.",
    });
  });

  it("is locked while an adoption is open, even on a swimlane that was confirmed before", () => {
    expect(editLock("sitb", inProgress)).toEqual({
      kind: "in_progress",
      title: "Editing locked",
      text: "sitb has an adoption in progress. Editing stays locked until an admin confirms the baseline.",
    });
  });

  it("is not locked when the baseline is confirmed", () => {
    expect(editLock("sitb", confirmed)).toBeNull();
  });

  it("does not claim a lock it cannot see: no baseline field means no banner", () => {
    expect(editLock("sitb", undefined)).toBeNull();
  });
});

describe("lockedExplanation (the 423 of a restart, with no cause field in the contract)", () => {
  it("derives the cause from the baseline when the baseline is not confirmed", () => {
    const text = lockedExplanation("presitc", unconfirmed, null);
    expect(text.title).toBe("presitc is locked until it is adopted");
    expect(text.detail).toContain("baseline is unconfirmed");
  });

  it("derives it from an open adoption", () => {
    expect(lockedExplanation("sitb", inProgress, null).title).toBe("sitb has an adoption in progress");
  });

  it("says only that something else holds it when the baseline is confirmed, and when it ends if the server says", () => {
    const text = lockedExplanation("sitb", confirmed, 1_760_000_000_000);
    expect(text.title).toBe("sitb is locked");
    expect(text.detail).toContain("Another operation is using it");
    expect(text.detail).toContain("2025-10-09 08:53 UTC");
  });

  it("makes no claim about the cause when there is no baseline field", () => {
    expect(lockedExplanation("sitb", undefined, null).detail).not.toContain("baseline");
  });
});
