import { describe, expect, it } from "vitest";
import type { DriftState, FileEntry } from "@/api/generated/model";
import { buildTree, MORE_PREFIX, type DataNode, type Folders } from "./build";

const entry = (path: string, over: Partial<FileEntry> = {}): FileEntry => ({
  path,
  is_dir: false,
  kind: "base",
  class: "structured",
  role: "property_source",
  size: 10,
  hash: "a".repeat(64),
  drift: "in_sync",
  format: { eol: "lf", encoding: "utf8", bom: false },
  content_withheld: false,
  attribution: null,
  severity: null,
  pickup_state: null,
  ...over,
});
const dir = (path: string, over: Partial<FileEntry> = {}): FileEntry => entry(path, { is_dir: true, kind: "base", class: "text", format: null, hash: null, ...over });
const folders = (...pairs: Array<[string, FileEntry[], boolean?]>): Folders => new Map(pairs.map(([prefix, entries, more]) => [prefix, { entries, hasMore: more === true }]));
const names = (nodes: readonly DataNode[]): string[] => nodes.map((n) => n.name);
const options = { logical: false, filter: "", fileClass: null } as const;

describe("buildTree", () => {
  it("lists the root with folders first and then files, each by name", () => {
    const tree = buildTree(folders(["", [entry("zeta.yml"), dir("tx-api"), entry("alpha.yml"), dir("security")]]), options);
    expect(names(tree)).toEqual(["security", "tx-api", "alpha.yml", "zeta.yml"]);
  });

  it("marks a folder whose entries are not loaded as lazy, and puts the loaded entries under a folder that is", () => {
    const tree = buildTree(folders(["", [dir("tx-api"), dir("security")]], ["tx-api", [entry("tx-api/core.yml")]]), options);
    const [security, txApi] = tree;
    expect(security).toMatchObject({ name: "security", isDir: true, lazy: true, children: [] });
    expect(txApi).toMatchObject({ name: "tx-api", lazy: false });
    expect(names(txApi?.children ?? [])).toEqual(["core.yml"]);
    expect(txApi?.children[0]).toMatchObject({ id: "tx-api/core.yml", name: "core.yml", isDir: false });
  });

  it("takes a folder path with a trailing slash as the same folder", () => {
    const tree = buildTree(folders(["", [dir("tx-api/")]], ["tx-api", [entry("tx-api/core.yml")]]), options);
    expect(tree[0]).toMatchObject({ id: "tx-api", name: "tx-api" });
    expect(names(tree[0]?.children ?? [])).toEqual(["core.yml"]);
  });

  it("adds a Load more row at the end of a folder that has more pages, with the folder it belongs to", () => {
    const tree = buildTree(folders(["", [entry("a.yml")], true]), options);
    expect(tree.map((n) => n.id)).toEqual(["a.yml", MORE_PREFIX]);
    expect(tree[1]).toMatchObject({ more: "" });
    const inner = buildTree(folders(["", [dir("tx-api")]], ["tx-api", [entry("tx-api/a.yml")], true]), options);
    expect(inner[0]?.children.at(-1)).toMatchObject({ id: `${MORE_PREFIX}tx-api`, more: "tx-api" });
  });

  it("is empty before the root is loaded", () => {
    expect(buildTree(new Map(), options)).toEqual([]);
  });

  it("keeps the drift state of a file", () => {
    const tree = buildTree(folders(["", [entry("a.yml", { drift: "nfs_ahead" as DriftState })]]), options);
    expect(tree[0]?.drift).toBe("nfs_ahead");
  });

  describe("with the logical files shown", () => {
    const entries = [
      entry("tx-api/core.yml", { logical_file: "tx-api/core.yml" }),
      entry("tx-api/core-sit7.yml", { kind: "tenant", tenant: "sit7", logical_file: "tx-api/core.yml", drift: "nfs_ahead" as DriftState }),
      entry("tx-api/gateway.yml", { logical_file: "tx-api/gateway.yml" }),
    ];
    const set = folders(["", [dir("tx-api")]], ["tx-api", entries]);

    it("lists a base file and the tenant file that replaces it as one row, which says it uses the tenant file", () => {
      const tree = buildTree(set, { ...options, logical: true });
      const children = tree[0]?.children ?? [];
      expect(names(children)).toEqual(["core.yml", "gateway.yml"]);
      expect(children[0]).toMatchObject({ id: "tx-api/core.yml", usesTenantFile: ["tx-api/core-sit7.yml"], drift: "nfs_ahead" });
      expect(children[1]?.usesTenantFile).toEqual([]);
    });

    it("lists both files as they are when the physical files are shown", () => {
      const tree = buildTree(set, options);
      expect(names(tree[0]?.children ?? [])).toEqual(["core-sit7.yml", "core.yml", "gateway.yml"]);
    });

    it("takes the most urgent drift state of the group", () => {
      const grouped = folders(
        ["", [
          entry("a.yml", { logical_file: "a.yml", drift: "git_ahead" as DriftState }),
          entry("a-sit1.yml", { kind: "tenant", tenant: "sit1", logical_file: "a.yml", drift: "conflict" as DriftState }),
          entry("a-sit2.yml", { kind: "tenant", tenant: "sit2", logical_file: "a.yml", drift: "in_sync" as DriftState }),
        ]],
      );
      const [row] = buildTree(grouped, { ...options, logical: true });
      expect(row?.drift).toBe("conflict");
      expect(row?.usesTenantFile).toEqual(["a-sit1.yml", "a-sit2.yml"]);
    });

    it("shows a tenant file that has no base file under its logical name", () => {
      const [row] = buildTree(folders(["", [entry("only-sit7.yml", { kind: "tenant", tenant: "sit7", logical_file: "only.yml" })]]), { ...options, logical: true });
      expect(row).toMatchObject({ id: "only-sit7.yml", name: "only.yml" });
    });

    it("uses the path when the server gave no logical file", () => {
      const [row] = buildTree(folders(["", [entry("plain.yml")]]), { ...options, logical: true });
      expect(row?.name).toBe("plain.yml");
    });
  });

  describe("filters", () => {
    const set = folders(
      ["", [dir("tx-api"), dir("security"), entry("readme.txt", { class: "text" })]],
      ["tx-api", [entry("tx-api/core.yml"), entry("tx-api/logo.png", { class: "binary" })]],
    );

    it("by file class, keeping the folders", () => {
      const tree = buildTree(set, { ...options, fileClass: "binary" });
      expect(names(tree)).toEqual(["security", "tx-api"]);
      expect(names(tree[1]?.children ?? [])).toEqual(["logo.png"]);
    });

    it("by name, case-insensitively, keeping a folder that holds a match and dropping the rest", () => {
      expect(names(buildTree(set, { ...options, filter: "CORE" }))).toEqual(["tx-api"]);
      expect(names(buildTree(set, { ...options, filter: "CORE" })[0]?.children ?? [])).toEqual(["core.yml"]);
    });

    it("keeps a folder whose own name matches, with what is loaded of it", () => {
      expect(names(buildTree(set, { ...options, filter: "secur" }))).toEqual(["security"]);
    });

    it("takes a pattern character as itself", () => {
      expect(buildTree(set, { ...options, filter: ".*" })).toEqual([]);
    });
  });
});
