import { describe, expect, it } from "vitest";
import type { DriftState, FileEntry } from "@/api/generated/model";
import { groupDrift, rowsOf } from "./group";

const entry = (path: string, drift: DriftState): FileEntry => ({
  path,
  is_dir: false,
  kind: "base",
  class: "structured",
  role: "property_source",
  size: 1,
  hash: "a".repeat(64),
  drift,
  format: { eol: "lf", encoding: "utf8", bom: false },
  content_withheld: false,
  attribution: null,
  severity: null,
  pickup_state: null,
});

describe("groupDrift", () => {
  it("groups the files by state, most urgent state first, and by path inside a group", () => {
    const groups = groupDrift([entry("b.yml", "nfs_ahead"), entry("z.yml", "conflict"), entry("a.yml", "nfs_ahead"), entry("m.yml", "git_ahead")]);
    expect(groups.map((g) => [g.state, g.files.map((f) => f.path)])).toEqual([
      ["conflict", ["z.yml"]],
      ["nfs_ahead", ["a.yml", "b.yml"]],
      ["git_ahead", ["m.yml"]],
    ]);
  });

  it("leaves out what is in sync, a folder, and a state that has no files", () => {
    const folder = { ...entry("dir", "nfs_ahead"), is_dir: true };
    expect(groupDrift([entry("a.yml", "in_sync"), folder])).toEqual([]);
  });

  it("keeps untracked, unknown and intentional divergences, after the others", () => {
    const groups = groupDrift([entry("i.yml", "intentional_divergence"), entry("t.yml", "untracked"), entry("u.yml", "unknown"), entry("g.yml", "git_ahead")]);
    expect(groups.map((g) => g.state)).toEqual(["git_ahead", "unknown", "untracked", "intentional_divergence"]);
  });
});

describe("rowsOf", () => {
  const groups = groupDrift([entry("a.yml", "nfs_ahead"), entry("b.yml", "nfs_ahead"), entry("c.yml", "conflict")]);

  it("is a header and its files for each group", () => {
    expect(rowsOf(groups, new Set()).map((r) => (r.kind === "group" ? `# ${r.state} ${r.count}` : r.file.path))).toEqual(["# conflict 1", "c.yml", "# nfs_ahead 2", "a.yml", "b.yml"]);
  });

  it("leaves out the files of a group that is closed, and keeps its header with the count", () => {
    expect(rowsOf(groups, new Set(["nfs_ahead"])).map((r) => (r.kind === "group" ? `# ${r.state} ${r.count} ${r.open}` : r.file.path))).toEqual(["# conflict 1 true", "c.yml", "# nfs_ahead 2 false"]);
  });
});
