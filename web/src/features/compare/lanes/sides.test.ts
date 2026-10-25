import { describe, expect, it } from "vitest";
import { sidesOf, withLeftLane, withRight, withTarget, swapped } from "./sides";

describe("the two sides of a whole-swimlane comparison", () => {
  it("reads a swimlane against a swimlane, a Git branch of the tenant repository, or a Git tag of the base repository", () => {
    expect(sidesOf({ left: "effective:sitb", right: "effective:sitc" })).toEqual({ leftLane: "sitb", target: "lane", rightName: "sitc" });
    expect(sidesOf({ left: "nfs:sitb", right: "git:tenant@sit7" })).toEqual({ leftLane: "sitb", target: "branch", rightName: "sit7" });
    expect(sidesOf({ left: "nfs:sitb", right: "git:base@v2.3.1" })).toEqual({ leftLane: "sitb", target: "tag", rightName: "v2.3.1" });
  });

  it("reads what a link sends: NFS or baseline sides are swimlanes, and a missing side is none", () => {
    expect(sidesOf({ left: "nfs:sitb", right: "nfs:sitc" })).toEqual({ leftLane: "sitb", target: "lane", rightName: "sitc" });
    expect(sidesOf({ left: "baseline:sitb", right: null })).toEqual({ leftLane: "sitb", target: "lane", rightName: null });
    expect(sidesOf({ left: null, right: null })).toEqual({ leftLane: null, target: "lane", rightName: null });
    expect(sidesOf({ left: "git:base@main", right: "effective:sitc" })).toEqual({ leftLane: null, target: "lane", rightName: "sitc" });
  });

  it("compares the effective files of two swimlanes (which file each reads), and NFS against Git", () => {
    expect(withRight({ left: "effective:sitb", right: null }, "sitc")).toEqual({ left: "effective:sitb", right: "effective:sitc" });
    expect(withTarget({ left: "effective:sitb", right: "effective:sitc" }, "branch")).toEqual({ left: "nfs:sitb", right: null });
    expect(withRight({ left: "nfs:sitb", right: null }, "sit7", "branch")).toEqual({ left: "nfs:sitb", right: "git:tenant@sit7" });
    expect(withRight({ left: "nfs:sitb", right: null }, "v2.3.1", "tag")).toEqual({ left: "nfs:sitb", right: "git:base@v2.3.1" });
  });

  it("keeps the right side's kind when the left swimlane changes", () => {
    expect(withLeftLane({ left: "effective:sitb", right: "effective:sitc" }, "sit")).toEqual({ left: "effective:sit", right: "effective:sitc" });
    expect(withLeftLane({ left: "nfs:sitb", right: "git:tenant@sit7" }, "sit")).toEqual({ left: "nfs:sit", right: "git:tenant@sit7" });
  });

  it("swaps two swimlanes, and does not swap a swimlane with Git", () => {
    expect(swapped({ left: "effective:sitb", right: "effective:sitc" })).toEqual({ left: "effective:sitc", right: "effective:sitb" });
    expect(swapped({ left: "nfs:sitb", right: "git:tenant@sit7" })).toBeNull();
    expect(swapped({ left: "effective:sitb", right: null })).toBeNull();
  });

  it("makes only references the contract allows", () => {
    expect(withRight({ left: "effective:sitb", right: null }, "not a lane")).toEqual({ left: "effective:sitb", right: null });
    expect(withLeftLane({ left: null, right: null }, "Bad Lane")).toEqual({ left: null, right: null });
  });
});
