import { describe, expect, it } from "vitest";
import { FindingKind } from "@/api/generated/model";
import { MAX_LANES, NO_FILTERS, clientLanes, filtersFromSearch, hrefOf, isFiltered, searchOf, serverParams, toggled } from "./filters";

// The filters of the Findings screen live in the address (`code`, `kind`, `severity`, `lanes`), so that a link keeps them. What
// the address says is untrusted: a value that is not one of the contract's is dropped, so a typo shows everything, not nothing.

describe("filtersFromSearch", () => {
  it("is no filter for an address with none", () => {
    expect(filtersFromSearch({})).toEqual(NO_FILTERS);
    expect(isFiltered(NO_FILTERS)).toBe(false);
  });

  it("reads checks by code, in either case, in the contract's order and once each", () => {
    expect(filtersFromSearch({ code: "c8,C1,C8" }).kinds).toEqual([FindingKind.missed_base_changes, FindingKind.duplicate_keys]);
  });

  it("reads checks by name too, and joins the two", () => {
    expect(filtersFromSearch({ kind: "drift,nope" }).kinds).toEqual([FindingKind.drift]);
    expect(filtersFromSearch({ code: "C8", kind: "drift" }).kinds).toEqual([FindingKind.drift, FindingKind.duplicate_keys]);
  });

  it("drops a code that is not a check", () => {
    expect(filtersFromSearch({ code: "C0,C13,zz,,C 1" }).kinds).toEqual([]);
  });

  it("reads one severity, as 'that and above', and nothing else", () => {
    expect(filtersFromSearch({ severity: "high" }).minSeverity).toBe("high");
    expect(filtersFromSearch({ severity: "critical" }).minSeverity).toBe("critical");
    for (const bad of ["High", "urgent", "", "high,low"]) expect(filtersFromSearch({ severity: bad }).minSeverity, bad).toBeNull();
  });

  it("reads swimlanes that are valid ids, once each, and drops the rest", () => {
    expect(filtersFromSearch({ lanes: "sitb,sitc,sitb,../x,SIT,,a b,presita" }).lanes).toEqual(["sitb", "sitc", "presita"]);
  });

  it("reads `swimlane`, the key the MCP tools' links use (`/findings?swimlane=sitb`), as one swimlane to filter by", () => {
    expect(filtersFromSearch({ swimlane: "sitb" }).lanes).toEqual(["sitb"]);
    expect(filtersFromSearch({ swimlane: "../x" }).lanes).toEqual([]);
  });

  it("lets `lanes`, the screen's own key, win over `swimlane`", () => {
    expect(filtersFromSearch({ lanes: "sitc,sitd", swimlane: "sitb" }).lanes).toEqual(["sitc", "sitd"]);
  });

  it("keeps at most as many swimlanes as it can send", () => {
    const many = Array.from({ length: MAX_LANES + 20 }, (_, i) => `lane${i}`).join(",");
    expect(filtersFromSearch({ lanes: many }).lanes).toHaveLength(MAX_LANES);
  });
});

describe("the address of a filter", () => {
  const filters = { lanes: ["sitb", "sitc"], kinds: [FindingKind.duplicate_keys, FindingKind.config_server_restart_required], minSeverity: "high" } as const;

  it("writes checks as codes, swimlanes as a list and the severity as it is", () => {
    expect(searchOf(filters)).toEqual({ code: "C8,C11", lanes: "sitb,sitc", severity: "high" });
    expect(hrefOf(filters)).toBe("/findings?code=C8%2CC11&lanes=sitb%2Csitc&severity=high");
  });

  it("is the bare screen when nothing is chosen", () => {
    expect(searchOf(NO_FILTERS)).toEqual({});
    expect(hrefOf(NO_FILTERS)).toBe("/findings");
  });

  it("reads back what it wrote", () => {
    expect(filtersFromSearch(searchOf(filters))).toEqual(filters);
  });
});

describe("what is asked of the server", () => {
  it("is everything when nothing is chosen", () => {
    expect(serverParams(NO_FILTERS)).toEqual({});
    expect(clientLanes(NO_FILTERS)).toBeNull();
  });

  it("is one swimlane by the server's filter", () => {
    const one = { ...NO_FILTERS, lanes: ["sitb"] };
    expect(serverParams(one)).toEqual({ swimlane: "sitb" });
    expect(clientLanes(one)).toBeNull();
  });

  it("is every swimlane, narrowed here, when the person chose several, because the server filters by one", () => {
    const two = { ...NO_FILTERS, lanes: ["sitb", "sitc"] };
    expect(serverParams(two)).toEqual({});
    expect([...(clientLanes(two) ?? [])]).toEqual(["sitb", "sitc"]);
  });

  it("sends the checks and the severity to the server, which filters by them", () => {
    expect(serverParams({ lanes: [], kinds: [FindingKind.drift], minSeverity: "medium" })).toEqual({ kinds: ["drift"], min_severity: "medium" });
  });

  it("never sends a cursor or a page size: those are the list's", () => {
    expect(Object.keys(serverParams({ lanes: ["a"], kinds: [FindingKind.drift], minSeverity: "low" })).sort()).toEqual(["kinds", "min_severity", "swimlane"]);
  });
});

describe("toggled", () => {
  it("adds what is missing and removes what is there, without changing the list it is given", () => {
    const list = ["a", "b"];
    expect(toggled(list, "c")).toEqual(["a", "b", "c"]);
    expect(toggled(list, "a")).toEqual(["b"]);
    expect(list).toEqual(["a", "b"]);
  });
});
