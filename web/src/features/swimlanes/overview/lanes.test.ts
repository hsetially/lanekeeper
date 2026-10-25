import { describe, expect, it } from "vitest";
import type { SwimlaneSummary } from "@/api/generated/model";
import { facts, filterLanes, sortLanes, tiersOf, type LaneFilter } from "./lanes";

const COMMIT = "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c";
const lane = (id: string, over: Partial<SwimlaneSummary> = {}): SwimlaneSummary => ({
  id,
  display_name: id,
  tenants: ["sit1"],
  agent: "connected",
  last_scan_at: 1_000,
  base_version: { tag: "v2.3.1", distance: 0, commit: COMMIT },
  drift_counts: [],
  finding_count: 0,
  tier: "SIT",
  project: "ops-sit",
  baseline: { state: "confirmed", by: null, at: 1 },
  ...over,
});

const LANES: SwimlaneSummary[] = [
  lane("sitb", { tenants: ["sit7", "sit8"], drift_counts: [["conflict", 1], ["nfs_ahead", 2]], finding_count: 2, last_scan_at: 5_000 }),
  lane("sit", { last_scan_at: 2_000 }),
  lane("presitb", { tier: "PRESIT", project: "ops-presit", agent: "disconnected", drift_counts: [["nfs_ahead", 3]], finding_count: 2, last_scan_at: 500 }),
  lane("presitc", { tier: "PRESIT", project: "ops-presit", tenants: [], drift_counts: [["unknown", 412]], baseline: { state: "unconfirmed", by: null, at: null }, last_scan_at: null }),
];

/** A swimlane as a server that predates the field sends it: the key is absent. */
function without(base: SwimlaneSummary, key: "tier" | "baseline"): SwimlaneSummary {
  const { [key]: _gone, ...rest } = base;
  return rest;
}

const none: LaneFilter = { q: "", tier: null, hasDrift: false, agent: null };
const ids = (lanes: readonly SwimlaneSummary[]) => lanes.map((l) => l.id);

describe("filterLanes", () => {
  it("keeps every swimlane when nothing is asked", () => {
    expect(ids(filterLanes(LANES, none))).toEqual(["sitb", "sit", "presitb", "presitc"]);
  });

  it("matches the text against the id, the project, the tier and every tenant, in any case, as plain text", () => {
    expect(ids(filterLanes(LANES, { ...none, q: "SIT8" }))).toEqual(["sitb"]);
    expect(ids(filterLanes(LANES, { ...none, q: "ops-presit" }))).toEqual(["presitb", "presitc"]);
    expect(ids(filterLanes(LANES, { ...none, q: "  presit  " }))).toEqual(["presitb", "presitc"]);
  });

  it("takes a pattern character as itself, not as a pattern", () => {
    expect(filterLanes(LANES, { ...none, q: "sit.*" })).toEqual([]);
    expect(filterLanes(LANES, { ...none, q: "(" })).toEqual([]);
  });

  it("filters by tier, whatever the case of the address", () => {
    expect(ids(filterLanes(LANES, { ...none, tier: "presit" }))).toEqual(["presitb", "presitc"]);
    expect(ids(filterLanes(LANES, { ...none, tier: "SIT" }))).toEqual(["sitb", "sit"]);
  });

  it("filters to the swimlanes that have drift that needs a look, not those that are only in sync", () => {
    expect(ids(filterLanes([...LANES, lane("quiet", { drift_counts: [["in_sync", 40]] })], { ...none, hasDrift: true }))).toEqual(["sitb", "presitb", "presitc"]);
  });

  it("filters by the agent: online is connected, offline is every other state", () => {
    expect(ids(filterLanes(LANES, { ...none, agent: "offline" }))).toEqual(["presitb"]);
    expect(ids(filterLanes(LANES, { ...none, agent: "online" }))).toEqual(["sitb", "sit", "presitc"]);
  });

  it("combines the filters", () => {
    expect(ids(filterLanes(LANES, { q: "sit", tier: "presit", hasDrift: true, agent: "online" }))).toEqual(["presitc"]);
  });

  it("does not change the list it was given", () => {
    const before = ids(LANES);
    filterLanes(LANES, { ...none, q: "sitb" });
    expect(ids(LANES)).toEqual(before);
  });
});

describe("sortLanes", () => {
  it("sorts by name, ascending and descending", () => {
    expect(ids(sortLanes(LANES, { column: "swimlane", direction: "asc" }))).toEqual(["presitb", "presitc", "sit", "sitb"]);
    expect(ids(sortLanes(LANES, { column: "swimlane", direction: "desc" }))).toEqual(["sitb", "sit", "presitc", "presitb"]);
  });

  it("sorts by findings, and then by name", () => {
    expect(ids(sortLanes(LANES, { column: "findings", direction: "desc" }))).toEqual(["presitb", "sitb", "presitc", "sit"]);
  });

  it("sorts by the last scan, newest first when descending, and puts a swimlane never scanned last either way", () => {
    expect(ids(sortLanes(LANES, { column: "last", direction: "desc" }))).toEqual(["sitb", "sit", "presitb", "presitc"]);
    expect(ids(sortLanes(LANES, { column: "last", direction: "asc" }))).toEqual(["presitb", "sit", "sitb", "presitc"]);
  });

  it("sorts by how many files need a look", () => {
    expect(ids(sortLanes(LANES, { column: "drift", direction: "desc" }))).toEqual(["presitc", "presitb", "sitb", "sit"]);
  });

  it("keeps the order it was given for a column it does not know", () => {
    expect(ids(sortLanes(LANES, { column: "nope", direction: "asc" }))).toEqual(["sitb", "sit", "presitb", "presitc"]);
  });
});

describe("tiersOf", () => {
  it("lists the tiers that exist, once each, in the order they first appear in upper case", () => {
    expect(tiersOf(LANES)).toEqual(["SIT", "PRESIT"]);
  });

  it("leaves out a swimlane with no tier", () => {
    expect(tiersOf([lane("x", { tier: null }), without(lane("y"), "tier")])).toEqual([]);
  });
});

describe("facts (the line under the title)", () => {
  it("counts the swimlanes, the agents online and the swimlanes whose baseline is not confirmed", () => {
    expect(facts(LANES)).toBe("4 swimlanes · 3 of 4 agents online · 1 needs adoption");
  });

  it("says need for more than one, and counts an adoption in progress as not done", () => {
    const lanes = [lane("a", { baseline: { state: "in_progress", by: null, at: null } }), lane("b", { baseline: { state: "unconfirmed", by: null, at: null } })];
    expect(facts(lanes)).toBe("2 swimlanes · 2 of 2 agents online · 2 need adoption");
  });

  it("does not count a swimlane whose baseline the server did not state", () => {
    expect(facts([without(lane("a"), "baseline")])).toBe("1 swimlane · 1 of 1 agents online · 0 need adoption");
  });
});
