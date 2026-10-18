import { describe, expect, it } from "vitest";
import type { TreeCompareEntry } from "@/api/generated/model";
import { buildTree, countStates, filterEntries, flattenIds } from "./tree";

const e = (file: string, state: TreeCompareEntry["state"] = "different"): TreeCompareEntry => ({ file, state });

describe("the tree of a whole-swimlane comparison", () => {
  it("groups the files by folder, folders first, each in name order, with the number of differing files below a folder", () => {
    const tree = buildTree([e("tx-infinity-api/tx-infinity-core.yml"), e("tx-infinity-api/tx-infinity-gateway.yml"), e("security/security-roles.yml"), e("readme.txt"), e("ops/batch/retry-policy.yml", "left_only"), e("ops/batch/batch-schedule.yml")]);
    expect(tree.map((n) => `${n.kind}:${n.path}:${n.count}`)).toEqual(["dir:ops:2", "dir:security:1", "dir:tx-infinity-api:2", "file:readme.txt:1"]);
    const ops = tree[0];
    expect(ops?.kind === "dir" && ops.children.map((c) => `${c.kind}:${c.path}:${c.count}`)).toEqual(["dir:ops/batch:2"]);
    const batch = ops?.kind === "dir" ? ops.children[0] : undefined;
    expect(batch?.kind === "dir" && batch.children.map((c) => c.path)).toEqual(["ops/batch/batch-schedule.yml", "ops/batch/retry-policy.yml"]);
  });

  it("leaves out a file that is the same on both sides", () => {
    const tree = buildTree([e("a/b.yml", "same"), e("a/c.yml")]);
    expect(tree).toHaveLength(1);
    expect(flattenIds(tree)).toEqual(["d:a", "f:a/c.yml"]);
  });

  it("keeps the state of each file", () => {
    const tree = buildTree([e("a.yml", "right_only")]);
    expect(tree[0]).toMatchObject({ kind: "file", state: "right_only", name: "a.yml" });
  });

  it("is not recursive and is bounded: a path of 500 folders does not overflow the stack", () => {
    const deep = `${Array.from({ length: 500 }, (_, i) => `d${i}`).join("/")}/f.yml`;
    const tree = buildTree([e(deep)]);
    expect(flattenIds(tree)).toHaveLength(501);
  });

  it("makes ids that tell a folder from a file of the same name", () => {
    expect(flattenIds(buildTree([e("a/b"), e("a/b/c.yml")]))).toEqual(expect.arrayContaining(["f:a/b", "d:a/b", "f:a/b/c.yml"]));
  });
});

describe("what the cards count and filter", () => {
  const entries = [e("a.yml"), e("b.yml", "left_only"), e("c.yml", "right_only"), e("d.yml", "same"), e("e.yml")];

  it("counts the differing, left-only and right-only files, and not the same", () => {
    expect(countStates(entries)).toEqual({ different: 2, left_only: 1, right_only: 1, same: 1 });
  });

  it("filters to one kind, or to all that differ", () => {
    expect(filterEntries(entries, "all").map((x) => x.file)).toEqual(["a.yml", "b.yml", "c.yml", "e.yml"]);
    expect(filterEntries(entries, "different").map((x) => x.file)).toEqual(["a.yml", "e.yml"]);
    expect(filterEntries(entries, "left_only").map((x) => x.file)).toEqual(["b.yml"]);
    expect(filterEntries(entries, "right_only").map((x) => x.file)).toEqual(["c.yml"]);
  });
});
