import { describe, expect, it } from "vitest";
import type { FlagStatus } from "@/api/generated/model";
import { buildRows, groupFlags } from "./model";

// What the Feature Status screen makes of the server's flags: one row for each documented flag with a cell for each swimlane,
// and a row for each channel under it. A cell DIFFERS when the value is not the template default; the contract has no field for
// it, so this compares the two texts as written (a display comparison, not an engine fact: see the page's note).

const value = (text: string) => ({ text, ty: "bool" as const, flag: null });
const entry = (flag: string, channel: string | null, def: string | null, values: Record<string, string | null>, setting = `${channel ?? "x"}.${flag}`): FlagStatus => ({
  flag,
  file: "api/core.yml",
  channel,
  setting,
  template_default: def === null ? null : value(def),
  values: Object.entries(values).map(([swimlane, text]) => ({ swimlane, value: text === null ? null : value(text) })),
});

const LANES = ["sit", "sitb", "sitc"];

describe("groupFlags", () => {
  it("groups the entries of one flag, in the order the flags first appear", () => {
    const groups = groupFlags([entry("b", "t", "false", {}), entry("a", null, "false", {}), entry("b", "u", "false", {})]);
    expect(groups.map((g) => [g.name, g.entries.length, g.channels.map((e) => e.channel)])).toEqual([
      ["b", 2, ["t", "u"]],
      ["a", 1, []],
    ]);
  });
});

describe("a flag with no channel", () => {
  const flags = [entry("enableX", null, "false", { sit: "true", sitb: "false", sitc: null })];

  it("has one cell for each swimlane: the value, marked when it differs, or absent", () => {
    const { rows } = buildRows(flags, LANES, { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(rows).toHaveLength(1);
    expect(rows[0]?.cells.get("sit")).toEqual({ state: "value", text: "true", differs: true, mixed: false });
    expect(rows[0]?.cells.get("sitb")).toEqual({ state: "value", text: "false", differs: false, mixed: false });
    expect(rows[0]?.cells.get("sitc")).toEqual({ state: "absent" });
    expect(rows[0]?.def).toBe("false");
    expect(rows[0]?.channels).toBe(0);
  });

  it("is absent for a swimlane the answer does not mention", () => {
    const { rows } = buildRows([entry("enableX", null, "false", { sit: "true" })], LANES, { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(rows[0]?.cells.get("sitb")).toEqual({ state: "absent" });
  });

  it("does not call a value a difference when the flag has no template default", () => {
    const { rows } = buildRows([entry("enableX", null, null, { sit: "true" })], LANES, { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(rows[0]?.cells.get("sit")).toEqual({ state: "value", text: "true", differs: false, mixed: false });
    expect(rows[0]?.def).toBeNull();
  });
});

describe("a flag with channels", () => {
  const flags = [
    entry("enableX", "teller", "false", { sit: "true", sitb: "false", sitc: "false" }),
    entry("enableX", "atm", "false", { sit: "false", sitb: "false", sitc: "true" }),
  ];

  it("summarises the channels in the flag's own row: mixed when the channels disagree", () => {
    const { rows } = buildRows(flags, LANES, { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(rows).toHaveLength(1);
    expect(rows[0]?.channels).toBe(2);
    expect(rows[0]?.cells.get("sit")).toEqual({ state: "value", text: "mixed", differs: true, mixed: true });
    expect(rows[0]?.cells.get("sitb")).toEqual({ state: "value", text: "false", differs: false, mixed: false });
    expect(rows[0]?.cells.get("sitc")).toEqual({ state: "value", text: "mixed", differs: true, mixed: true });
  });

  it("shows the value when every channel has the same one", () => {
    const same = [entry("enableX", "teller", "false", { sit: "true" }), entry("enableX", "atm", "false", { sit: "true" })];
    const { rows } = buildRows(same, ["sit"], { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(rows[0]?.cells.get("sit")).toEqual({ state: "value", text: "true", differs: true, mixed: false });
  });

  it("adds a row for each channel when the flag is open, with that channel's own cells", () => {
    const { rows } = buildRows(flags, LANES, { expanded: new Set(["enableX"]), filter: "", onlyDiffering: false });
    expect(rows.map((r) => [r.kind, r.name])).toEqual([
      ["flag", "enableX"],
      ["channel", "teller"],
      ["channel", "atm"],
    ]);
    expect(rows[1]?.cells.get("sit")).toEqual({ state: "value", text: "true", differs: true, mixed: false });
    expect(rows[2]?.cells.get("sitc")).toEqual({ state: "value", text: "true", differs: true, mixed: false });
  });

  it("keeps the ids of the rows apart: a channel row is not its flag", () => {
    const { rows } = buildRows(flags, LANES, { expanded: new Set(["enableX"]), filter: "", onlyDiffering: false });
    expect(new Set(rows.map((r) => r.id)).size).toBe(3);
  });
});

describe("counting and filtering", () => {
  const flags = [
    entry("enableAccountSorting", "teller", "false", { sit: "true", sitb: "false", sitc: "false" }),
    entry("enableAuditTrail", null, "true", { sit: "true", sitb: "true", sitc: "true" }),
    entry("enableRateLimiter", null, "true", { sit: "true", sitb: "false", sitc: "true" }),
  ];

  it("counts the flags, those that differ, and for each swimlane the flags that differ in it", () => {
    const result = buildRows(flags, LANES, { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(result.total).toBe(3);
    expect(result.shown).toBe(3);
    expect(result.differing).toBe(2);
    expect(Object.fromEntries(result.differPerLane)).toEqual({ sit: 1, sitb: 1, sitc: 0 });
  });

  it("shows only the flags that differ when asked, and the counts stay those of the whole list", () => {
    const result = buildRows(flags, LANES, { expanded: new Set(), filter: "", onlyDiffering: true });
    expect(result.rows.map((r) => r.name)).toEqual(["enableAccountSorting", "enableRateLimiter"]);
    expect(result.shown).toBe(2);
    expect(result.total).toBe(3);
  });

  it("filters by the name of the flag or its setting, ignoring case, as plain text", () => {
    expect(buildRows(flags, LANES, { expanded: new Set(), filter: "RATE", onlyDiffering: false }).rows.map((r) => r.name)).toEqual(["enableRateLimiter"]);
    expect(buildRows(flags, LANES, { expanded: new Set(), filter: "teller.enableAccount", onlyDiffering: false }).rows.map((r) => r.name)).toEqual(["enableAccountSorting"]);
    expect(buildRows(flags, LANES, { expanded: new Set(), filter: ".*", onlyDiffering: false }).rows).toEqual([]);
  });

  it("is empty for no flags", () => {
    const result = buildRows([], LANES, { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(result).toMatchObject({ rows: [], total: 0, shown: 0, differing: 0 });
  });
});

describe("the template default of a flag", () => {
  it("is the one default, or `varies` when the channels have different ones", () => {
    const one = buildRows([entry("f", "a", "false", {}), entry("f", "b", "false", {})], LANES, { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(one.rows[0]?.def).toBe("false");
    const many = buildRows([entry("f", "a", "false", {}), entry("f", "b", "true", {})], LANES, { expanded: new Set(), filter: "", onlyDiffering: false });
    expect(many.rows[0]?.def).toBe("varies");
  });
});
