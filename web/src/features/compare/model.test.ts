import { describe, expect, it } from "vitest";
import type { GridCell, SettingValue } from "@/api/generated/model";
import { MAX_GRID_LANES, cellKind, compareHref, describeRef, hiddenMarker, isChannelName, isCompareRef, isLogicalFile, pagesToKeep, parseLanes, parseView, rowFacts, splitPath } from "./model";

const value = (text: string, flag: SettingValue["flag"] = null): GridCell => ({ value: { text, ty: "string", flag }, location: { path: "a", lines: { start: 1, end: 1 } } });
const absent: GridCell = { value: null, location: null };

describe("the lanes of the address", () => {
  it("keeps valid swimlane ids in the order given, once each", () => {
    expect(parseLanes("sit,sitb,sit,presita")).toEqual(["sit", "sitb", "presita"]);
  });

  it("drops what is not a swimlane id instead of sending it to the server", () => {
    expect(parseLanes("sit,,Sit B,../etc,sitb,-bad,a")).toEqual(["sit", "sitb", "a"]);
    expect(parseLanes(undefined)).toEqual([]);
    expect(parseLanes("")).toEqual([]);
  });

  it("is bounded: a long list is cut, not parsed to the end", () => {
    const many = Array.from({ length: 500 }, (_, i) => `lane-${i}`).join(",");
    expect(parseLanes(many).length).toBeLessThanOrEqual(200);
    expect(parseLanes(many).length).toBeGreaterThan(MAX_GRID_LANES);
  });
});

describe("a logical file and a channel from the address", () => {
  it("accepts a base-relative path", () => {
    expect(isLogicalFile("tx-infinity-api/tx-infinity-core.yml")).toBe(true);
    expect(isLogicalFile("a.yml")).toBe(true);
  });

  it("refuses an absolute path, a parent segment, a backslash, a control character and an empty or oversize path", () => {
    for (const bad of ["", "/etc/passwd", "../x.yml", "a/../b.yml", "a\\b.yml", "a\u0000b", "a\nb", "x".repeat(1025)]) expect(isLogicalFile(bad), JSON.stringify(bad)).toBe(false);
  });

  it("takes a channel name by the contract's pattern", () => {
    expect(isChannelName("remote-itm-teller")).toBe(true);
    expect(isChannelName("atm_iso.v2")).toBe(true);
    for (const bad of ["", "-x", "a b", "a/b", "x".repeat(129)]) expect(isChannelName(bad), bad).toBe(false);
  });
});

describe("the view of the address", () => {
  it("defaults to the grid with only the differing rows", () => {
    expect(parseView({})).toEqual({ mode: "grid", lanes: [], file: null, onlyDiff: true, channel: null, left: null, right: null });
  });

  it("reads lanes, file, only and channel", () => {
    const view = parseView({ lanes: "sit,sitb", file: "a/b.yml", only: "all", channel: "atm" });
    expect(view).toMatchObject({ mode: "grid", lanes: ["sit", "sitb"], file: "a/b.yml", onlyDiff: false, channel: "atm" });
  });

  it("ignores a file or channel that is not valid", () => {
    expect(parseView({ file: "../x", channel: "a b" })).toMatchObject({ file: null, channel: null });
  });

  it("is the whole-swimlane compare when the address names a side, with the references the server's links use", () => {
    const view = parseView({ left: "effective:sitb", right: "git:tenant@sit7", file: "a/b.yml" });
    expect(view).toMatchObject({ mode: "lanes", left: "effective:sitb", right: "git:tenant@sit7", file: "a/b.yml" });
    expect(parseView({ right: "nfs:sitc" })).toMatchObject({ mode: "lanes", left: null, right: "nfs:sitc" });
  });

  it("takes a side that is not a reference for no side, so nothing odd is sent to the server", () => {
    expect(parseView({ left: "Not A Lane", right: "git:base@" })).toMatchObject({ mode: "grid", left: null, right: null });
  });
});

describe("the address of a view", () => {
  it("writes only the keys the route reads, and leaves the defaults out", () => {
    const view = { ...parseView({}), lanes: ["sit", "sitb"], file: "a.yml" };
    expect(compareHref(view)).toBe("/compare?file=a.yml&lanes=sit%2Csitb");
    expect(compareHref({ ...view, onlyDiff: false, channel: "atm" })).toBe("/compare?channel=atm&file=a.yml&lanes=sit%2Csitb&only=all");
  });

  it("never writes a key the route does not read (it would throw)", () => {
    expect(() => compareHref({ ...parseView({}), mode: "lanes", left: "effective:sitb", right: "effective:sitc", file: "a.yml" })).not.toThrow();
  });

  it("writes the sides of a whole-swimlane compare, and not the lanes of the grid", () => {
    const view = { ...parseView({}), lanes: ["sit"], mode: "lanes" as const, left: "effective:sitb", right: "effective:sitc", file: "a.yml" };
    const href = compareHref(view);
    expect(href).not.toContain("lanes=");
    expect(href).toContain("file=a.yml");
  });
});

describe("a side of a comparison", () => {
  it("accepts the five references of the contract", () => {
    for (const ok of ["nfs:sitb", "baseline:sitb", "effective:sitb", "git:base@v2.3.1", "git:tenant@sit7"]) expect(isCompareRef(ok), ok).toBe(true);
    for (const bad of ["", "sitb", "nfs:", "nfs:Sit", "git:other@main", "git:base@", `git:base@${"x".repeat(256)}`, "nfs:sitb\n"]) expect(isCompareRef(bad), bad).toBe(false);
  });

  it("says what a side is, in the words the page uses", () => {
    expect(describeRef("effective:sitb")).toEqual({ kind: "swimlane", name: "sitb", source: "Effective", repo: null });
    expect(describeRef("nfs:sitb")).toEqual({ kind: "swimlane", name: "sitb", source: "NFS", repo: null });
    expect(describeRef("baseline:sitb")).toEqual({ kind: "swimlane", name: "sitb", source: "Baseline", repo: null });
    expect(describeRef("git:tenant@sit7")).toEqual({ kind: "git", name: "sit7", source: "Git", repo: "tenant" });
    expect(describeRef("git:base@v2.3.1")).toEqual({ kind: "git", name: "v2.3.1", source: "Git", repo: "base" });
  });
});

describe("the kind of a cell", () => {
  it("tells a value, an empty value, an absent key and a hidden value apart", () => {
    expect(cellKind(value("5000"))).toBe("value");
    expect(cellKind(value(""))).toBe("empty");
    expect(cellKind(absent)).toBe("absent");
    expect(cellKind(value("[REDACTED: 12 chars, aws_key]"))).toBe("hidden");
  });

  it("takes a value that only looks like a marker for a value", () => {
    expect(cellKind(value("[REDACTED: twelve chars]"))).toBe("value");
    expect(cellKind(value("x [REDACTED: 3 chars, k]"))).toBe("value");
  });

  it("reads the length and the rule from a marker", () => {
    expect(hiddenMarker("[REDACTED: 12 chars, aws_key]")).toEqual({ chars: 12, rule: "aws_key" });
    expect(hiddenMarker("plain")).toBeNull();
  });
});

describe("which cells of a row differ from the rest", () => {
  const differingCells = (cells: readonly GridCell[]) => rowFacts(cells).differing;

  it("marks the cells that are not the most common value", () => {
    const cells = [value("5000"), value("5000"), value("10000"), value("5000")];
    expect([...differingCells(cells)]).toEqual([2]);
  });

  it("counts an absent key and an empty value as values of their own", () => {
    const cells = [value("a"), value("a"), absent, value("")];
    expect([...differingCells(cells)].sort()).toEqual([2, 3]);
  });

  it("marks nothing when every visible cell agrees", () => {
    expect([...differingCells([value("a"), value("a")])]).toEqual([]);
  });

  it("says nothing about a hidden value: it is never marked, and it is left out of the count", () => {
    const hidden = value("[REDACTED: 4 chars, k]");
    expect([...differingCells([value("a"), value("b"), hidden, hidden])].sort()).toEqual([1]);
    expect([...differingCells([hidden, hidden])]).toEqual([]);
    expect([...differingCells([hidden, value("a")])]).toEqual([]);
  });

  it("does not take yes for true", () => {
    expect([...differingCells([value("true"), value("yes", "yaml_boolean_like"), value("true")])]).toEqual([1]);
  });

  it("says whether a row has a hidden value, so that it is never called the same everywhere", () => {
    const hidden = value("[REDACTED: 4 chars, k]");
    expect(rowFacts([value("a"), value("a")]).hidden).toBe(false);
    expect(rowFacts([value("a"), value("a"), hidden]).hidden).toBe(true);
    expect(rowFacts([value("a"), value("a"), hidden]).differing.size).toBe(0);
  });

  it("gives the same answer for the same row each time (it is kept)", () => {
    const cells = [value("a"), value("b"), value("a")];
    expect(differingCells(cells)).toBe(differingCells(cells));
  });
});

describe("the path of a setting", () => {
  it("splits at the last dot, so the leaf can be bold", () => {
    expect(splitPath("channels.atm.limits.withdrawal.max")).toEqual({ parent: "channels.atm.limits.withdrawal.", leaf: "max" });
    expect(splitPath("name")).toEqual({ parent: "", leaf: "name" });
  });

  it("does not split inside an index or a quoted key", () => {
    expect(splitPath("a.b[0]")).toEqual({ parent: "a.", leaf: "b[0]" });
    expect(splitPath('a["x.y"]')).toEqual({ parent: "", leaf: 'a["x.y"]' });
    expect(splitPath('a.b["x.y"]')).toEqual({ parent: "a.", leaf: 'b["x.y"]' });
  });

  it("keeps the document prefix of a multi-document file on the parent", () => {
    expect(splitPath("#2/a.b")).toEqual({ parent: "#2/a.", leaf: "b" });
  });
});

describe("how many pages a refresh has to read", () => {
  const sizes = { first: 50, next: 500 };
  it("reads the first page when the person is at the top and the page covers the window", () => {
    expect(pagesToKeep(0, 11, sizes)).toBe(1);
    expect(pagesToKeep(30, 11, sizes)).toBe(1);
  });

  it("reads as many pages as it takes to cover the last row in view and a margin", () => {
    // 50 + 500 = 550 rows in two pages; the window ends at 700 + a margin.
    expect(pagesToKeep(700, 11, sizes)).toBe(3);
    expect(pagesToKeep(2000, 11, sizes)).toBe(5);
  });

  it("never reads more pages than there are", () => {
    expect(pagesToKeep(100_000, 4, sizes)).toBe(4);
    expect(pagesToKeep(0, 0, sizes)).toBe(1);
  });
});
