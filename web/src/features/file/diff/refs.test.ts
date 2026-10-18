import { describe, expect, it } from "vitest";
import { compareSides, diffParamOf, parseDiffParam, sideLabel, type SideContext } from "./refs";

// Which two sides a file's Diff tab compares. The page's `diff` search value names the LEFT side (the right is this swimlane's
// NFS, as in the design): `git`, another swimlane's id, `tag:<name>` or `branch:<name>`, and `baseline:<swimlane>` for the
// adopted copy. The sides go to the server as typed references (CompareRef); the browser never reads a side's file to diff it.

const tenantFile: SideContext = { lane: "sitb", tenant: "sit7", baseCommit: "a".repeat(40), baseLabel: "v2.3.1 + 4" };
const baseFile: SideContext = { lane: "sitb", tenant: null, baseCommit: "b".repeat(40), baseLabel: "v2.3.1 + 4" };

describe("parseDiffParam", () => {
  it("reads the values the handoff names", () => {
    expect(parseDiffParam("git")).toEqual({ kind: "git" });
    expect(parseDiffParam(undefined)).toEqual({ kind: "git" });
    expect(parseDiffParam("presita")).toEqual({ kind: "lane", id: "presita" });
    expect(parseDiffParam("tag:v2.3.1")).toEqual({ kind: "tag", name: "v2.3.1" });
    expect(parseDiffParam("branch:sit9")).toEqual({ kind: "branch", name: "sit9" });
    expect(parseDiffParam("baseline:sitb")).toEqual({ kind: "baseline", lane: "sitb" });
  });
  it("falls back to Git for a value that is none of them", () => {
    for (const bad of ["", "../x", "tag:", "tag:a b", "branch:a;b", "UPPER", "a/b", "x".repeat(300), "baseline:../x"]) expect(parseDiffParam(bad)).toEqual({ kind: "git" });
  });
});

describe("diffParamOf", () => {
  it("is the value parseDiffParam reads back", () => {
    for (const side of [{ kind: "git" }, { kind: "lane", id: "sitc" }, { kind: "tag", name: "v2.3.0" }, { kind: "branch", name: "main" }, { kind: "baseline", lane: "sitb" }] as const) {
      expect(parseDiffParam(diffParamOf(side))).toEqual(side);
    }
  });
});

describe("compareSides", () => {
  it("compares a tenant file's Git head (its tenant's branch) with this swimlane's NFS", () => {
    expect(compareSides({ kind: "git" }, false, tenantFile)).toEqual({ left: "git:tenant@sit7", right: "nfs:sitb" });
  });
  it("compares a base file with the swimlane's base version", () => {
    expect(compareSides({ kind: "git" }, false, baseFile)).toEqual({ left: `git:base@${"b".repeat(40)}`, right: "nfs:sitb" });
  });
  it("names another swimlane's NFS, the baseline, a tag and a branch", () => {
    expect(compareSides({ kind: "lane", id: "presita" }, false, tenantFile)?.left).toBe("nfs:presita");
    expect(compareSides({ kind: "baseline", lane: "sitb" }, false, tenantFile)?.left).toBe("baseline:sitb");
    expect(compareSides({ kind: "tag", name: "v2.3.1" }, false, tenantFile)?.left).toBe("git:base@v2.3.1");
    expect(compareSides({ kind: "branch", name: "sit9" }, false, tenantFile)?.left).toBe("git:tenant@sit9");
    expect(compareSides({ kind: "branch", name: "main" }, false, baseFile)?.left).toBe("git:base@main");
  });
  it("swaps the sides on request", () => {
    expect(compareSides({ kind: "git" }, true, tenantFile)).toEqual({ left: "nfs:sitb", right: "git:tenant@sit7" });
  });
  it("has no sides to compare when the swimlane has no base version for a base file", () => {
    expect(compareSides({ kind: "git" }, false, { ...baseFile, baseCommit: null })).toBeNull();
  });
  it("never sends a reference that is not one", () => {
    expect(compareSides({ kind: "tag", name: "a".repeat(300) }, false, tenantFile)).toBeNull();
    expect(compareSides({ kind: "lane", id: "Bad Lane" }, false, tenantFile)).toBeNull();
  });
});

describe("sideLabel", () => {
  it("says what each side is, the way the design does", () => {
    expect(sideLabel({ kind: "git" }, tenantFile)).toBe("Git (sit7 head)");
    expect(sideLabel({ kind: "git" }, baseFile)).toBe("Git (v2.3.1 + 4)");
    expect(sideLabel({ kind: "baseline", lane: "sitb" }, tenantFile)).toBe("Baseline");
    expect(sideLabel({ kind: "lane", id: "sitc" }, tenantFile)).toBe("sitc");
    expect(sideLabel({ kind: "tag", name: "v2.3.1" }, tenantFile)).toBe("v2.3.1");
    expect(sideLabel({ kind: "branch", name: "sit9" }, tenantFile)).toBe("sit9");
  });
});
