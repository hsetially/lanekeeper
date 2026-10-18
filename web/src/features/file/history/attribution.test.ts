import { describe, expect, it } from "vitest";
import type { Attribution } from "@/api/generated/model";
import { describeAttribution, sourceLabel } from "./attribution";

const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const OTHER = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000003";

const attribution = (over: Partial<Attribution>): Attribution => ({ source: "tool_write", confidence: "certain", actor: { kind: "user", user: ME }, evidence: {}, ...over });

describe("describeAttribution", () => {
  it("says You for the signed-in person's own write, with the strongest confidence", () => {
    expect(describeAttribution(attribution({}), ME)).toEqual({ who: "You", how: "via Lanekeeper", confidence: "Certain", bars: 4 });
  });

  it("does not name another person: the contract gives an id and no name", () => {
    expect(describeAttribution(attribution({ actor: { kind: "user", user: OTHER } }), ME)).toMatchObject({ who: "Another Lanekeeper user", how: "via Lanekeeper" });
  });

  it("names an OS login", () => {
    expect(describeAttribution(attribution({ source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" } }), ME)).toEqual({ who: "dana.okoye", how: "at an NFS login", confidence: "High", bars: 3 });
  });

  it("names a sync job and its run", () => {
    expect(describeAttribution(attribution({ source: "sync_job", confidence: "high", actor: { kind: "job", job: { name: "csp-tenant-data-sit7", uid: "run-41" } } }), ME).who).toBe("Sync job csp-tenant-data-sit7");
  });

  it("is Unknown when nobody is named, whatever the source says", () => {
    expect(describeAttribution(attribution({ source: "unknown", confidence: "low", actor: null }), ME)).toEqual({ who: "Unknown", how: "found by scan", confidence: "Unknown", bars: 0 });
  });

  it("scales the bars with the confidence", () => {
    const bars = (confidence: Attribution["confidence"]) => describeAttribution(attribution({ confidence }), ME).bars;
    expect([bars("low"), bars("medium"), bars("high"), bars("certain")]).toEqual([1, 2, 3, 4]);
  });

  it("says an NFS client that was seen but not named", () => {
    expect(describeAttribution(attribution({ source: "nfs_client", confidence: "medium", actor: null }), null).how).toBe("from an NFS client");
  });
});

describe("sourceLabel", () => {
  it("says how a version was observed, in the design's words", () => {
    expect(sourceLabel("tool_write")).toBe("Tool write");
    expect(sourceLabel("scan")).toBe("Scan found a change");
    expect(sourceLabel("sync")).toBe("Sync from Git");
  });
});
