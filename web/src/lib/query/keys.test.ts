import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import {
  ADMIN_SECTIONS,
  adminKeys,
  auditKeys,
  compareKeys,
  docKeys,
  fileKeys,
  findingKeys,
  gitKeys,
  IMMUTABLE_ROOTS,
  isImmutableKey,
  keyRoot,
  matchers,
  meKeys,
  prKeys,
  proposalKeys,
  swimlaneKeys,
} from "./keys";

// The vocabulary of query keys (plan, "Query keys, retry and SSE invalidation"), one factory per domain. A key is a pure
// function of what the request depends on, so a cached answer is safe to reuse; the two roots that are functions of a content
// hash are immutable and are never refetched on a resync. Every key must also be what TanStack's partial matching expects,
// because the live events and the writes invalidate by prefix.

const all = (): Array<[string, readonly unknown[]]> => [
  ["me", meKeys.all()],
  ["swimlanes", swimlaneKeys.all()],
  ["swimlane", swimlaneKeys.detail("sit1")],
  ["pickup", swimlaneKeys.pickup("sit1")],
  ["tree", fileKeys.tree("sit1", "a/b", null)],
  ["file-head", fileKeys.head("sit1", "a/b.yml")],
  ["content", fileKeys.content("sit1", "a/b.yml", "h1")],
  ["blob", fileKeys.blob("h1")],
  ["content-ref", fileKeys.contentRef("sit1", "a/b.yml", "base", "main")],
  ["compare", compareKeys.diff("nfs:sit1", "git:base@main", "a/b.yml", null)],
  ["history", fileKeys.history("sit1", "a/b.yml")],
  ["grid", compareKeys.grid("a/b.yml", ["sit1", "sit2"], false, null)],
  ["compare-tree", compareKeys.tree("nfs:sit1", "nfs:sit2", "a")],
  ["git-refs", gitKeys.refs("base", "tag", "v1")],
  ["findings", findingKeys.list({ limit: 50 })],
  ["proposals", proposalKeys.list({ limit: 50 })],
  ["proposal", proposalKeys.detail(12)],
  ["prs", prKeys.list({ limit: 50 })],
  ["pr-job", prKeys.job(9)],
  ["docs", docKeys.list({ limit: 50 })],
  ["doc", docKeys.detail(3)],
  ["features", docKeys.features({ flag: "auto-notify" })],
  ["audit", auditKeys.list({ limit: 50 })],
  ["activity", swimlaneKeys.activity("sit1")],
  ["me-github-credential", meKeys.githubCredential()],
  ["me-mcp-tokens", meKeys.mcpTokens()],
  ["admin", adminKeys.list("users", { limit: 50 })],
  ["draft", fileKeys.draft(12)],
];

describe("key factories", () => {
  it("has a root for every cache the plan lists, and the root is the first element", () => {
    const roots = all().map(([name]) => name);
    expect(roots).toEqual([
      "me", "swimlanes", "swimlane", "pickup", "tree", "file-head", "content", "blob", "content-ref", "compare", "history", "grid",
      "compare-tree", "git-refs", "findings", "proposals", "proposal", "prs", "pr-job", "docs", "doc", "features", "audit", "activity",
      "me-github-credential", "me-mcp-tokens", "admin", "draft",
    ]);
    for (const [name, key] of all()) {
      expect(key[0], name).toBe(name);
      expect(keyRoot(key), name).toBe(name);
    }
  });

  it("makes the same key for the same input and a different key for a different one", () => {
    expect(fileKeys.tree("sit1", "a", null)).toEqual(fileKeys.tree("sit1", "a", null));
    expect(fileKeys.tree("sit1", "a", null)).not.toEqual(fileKeys.tree("sit1", "a", "git_ahead"));
    expect(fileKeys.tree("sit1", "a", null)).not.toEqual(fileKeys.tree("sit2", "a", null));
    expect(compareKeys.diff("nfs:sit1", "nfs:sit2", "f", null)).not.toEqual(compareKeys.diff("nfs:sit1", "nfs:sit2", "f", "ch1"));
  });

  it("does not let a caller's later change reach a key that was made from it", () => {
    const lanes = ["sit1", "sit2"];
    const key = compareKeys.grid("f", lanes, true, null);
    lanes.push("sit3");
    expect(JSON.stringify(key)).not.toContain("sit3");
  });

  it("names ids as strings whether they come from a route (a string) or from an event (a number)", () => {
    expect(proposalKeys.detail(12)).toEqual(proposalKeys.detail("12"));
    expect(prKeys.job(9)).toEqual(prKeys.job("9"));
    expect(docKeys.detail(3)).toEqual(docKeys.detail("3"));
  });

  it("gives each domain a prefix that finds every key of that cache", () => {
    expect(swimlaneKeys.all()).toEqual(["swimlanes"]);
    expect(swimlaneKeys.details()).toEqual(["swimlane"]);
    expect(fileKeys.trees("sit1")).toEqual(["tree", "sit1"]);
    expect(fileKeys.heads("sit1")).toEqual(["file-head", "sit1"]);
    expect(compareKeys.diffs()).toEqual(["compare"]);
    expect(compareKeys.grids()).toEqual(["grid"]);
    expect(proposalKeys.lists()).toEqual(["proposals"]);
    expect(prKeys.lists()).toEqual(["prs"]);
    expect(docKeys.lists()).toEqual(["docs"]);
    expect(docKeys.featureStatuses()).toEqual(["features"]);
    expect(findingKeys.all()).toEqual(["findings"]);
    expect(auditKeys.all()).toEqual(["audit"]);
    expect(adminKeys.section("users")).toEqual(["admin", "users"]);
    expect(ADMIN_SECTIONS).toContain("access-requests");
    expect(adminKeys.all()).toEqual(["admin"]);
    expect(gitKeys.repo("base")).toEqual(["git-refs", "base"]);
  });

  it("puts a file's content under the hash, so a changed file is a different key and an old one is never stale", () => {
    expect(fileKeys.content("sit1", "a.yml", "h1")).not.toEqual(fileKeys.content("sit1", "a.yml", "h2"));
    expect(isImmutableKey(fileKeys.content("sit1", "a.yml", "h1"))).toBe(true);
    expect(isImmutableKey(fileKeys.blob("h1"))).toBe(true);
  });

  it("marks exactly the content and blob roots as immutable", () => {
    expect([...IMMUTABLE_ROOTS].sort()).toEqual(["blob", "content"]);
    for (const [name, key] of all()) expect(isImmutableKey(key), name).toBe(name === "content" || name === "blob");
  });

  it("does NOT mark compare or content-ref as immutable: they are keyed by refs, and a ref moves", () => {
    expect(isImmutableKey(compareKeys.diff("nfs:sit1", "git:base@main", "f", null))).toBe(false);
    expect(isImmutableKey(fileKeys.contentRef("sit1", "f", "base", "main"))).toBe(false);
  });

  it("is what partial matching expects: a prefix finds every key under it (the events and the writes invalidate by prefix)", () => {
    const client = new QueryClient();
    client.setQueryData(fileKeys.tree("sit1", "a", null), 1);
    client.setQueryData(fileKeys.tree("sit1", "a/b", "git_ahead"), 2);
    client.setQueryData(fileKeys.tree("sit2", "a", null), 3);
    client.setQueryData(swimlaneKeys.all(), 4);
    const under = (key: readonly unknown[]) => client.getQueryCache().findAll({ queryKey: key as unknown[] }).length;
    expect(under(fileKeys.trees("sit1"))).toBe(2);
    expect(under(["tree"])).toBe(3);
    expect(under(swimlaneKeys.all())).toBe(1);
    expect(under(["swimlane"])).toBe(0);
  });

  it("keys the Drift tab under the swimlane's own key, so every event that refreshes the swimlane refreshes it", () => {
    const client = new QueryClient();
    client.setQueryData(swimlaneKeys.drift("sit1", null), 1);
    client.setQueryData(swimlaneKeys.drift("sit1", "git_ahead"), 2);
    client.setQueryData(swimlaneKeys.drift("sit2", null), 3);
    expect(swimlaneKeys.drift("sit1", null)).toEqual(["swimlane", "sit1", "drift", null]);
    expect(swimlaneKeys.drift("sit1", null)).not.toEqual(swimlaneKeys.drift("sit1", "git_ahead"));
    expect(client.getQueryCache().findAll({ queryKey: [...swimlaneKeys.detail("sit1")] })).toHaveLength(2);
  });

  it("keys the docs of one file under docs, so doc_indexed (which invalidates every docs list) reaches them", () => {
    const client = new QueryClient();
    client.setQueryData(docKeys.forFile("a/b.yml", null), 1);
    client.setQueryData(docKeys.forFile("a/b.yml", "server.port"), 2);
    client.setQueryData(docKeys.forFile("a/c.yml", null), 3);
    expect(docKeys.forFile("a/b.yml", null)[0]).toBe("docs");
    expect(docKeys.forFile("a/b.yml", null)).not.toEqual(docKeys.forFile("a/b.yml", "server.port"));
    expect(client.getQueryCache().findAll({ queryKey: [...docKeys.lists()] })).toHaveLength(3);
    // A docs LIST key (the filters object) never collides with a for-file key.
    expect(docKeys.list({ limit: 5 })).not.toEqual(docKeys.forFile("a/b.yml", null));
  });

  it("keys a draft by its id as a string, whether the id came from the route or from a result", () => {
    expect(fileKeys.draft(12)).toEqual(["draft", "12"]);
    expect(fileKeys.draft("12")).toEqual(fileKeys.draft(12));
    expect(isImmutableKey(fileKeys.draft(12))).toBe(false);
  });

  it("keys the baseline copy of a file apart from every git read, and not as immutable (a baseline is rebased)", () => {
    expect(fileKeys.baseline("sit1", "a/b.yml")).toEqual(fileKeys.contentRef("sit1", "a/b.yml", "baseline", "baseline"));
    expect(fileKeys.baseline("sit1", "a/b.yml")).not.toEqual(fileKeys.contentRef("sit1", "a/b.yml", "base", "main"));
    expect(isImmutableKey(fileKeys.baseline("sit1", "a/b.yml"))).toBe(false);
    // No branch event reaches it: "baseline" is not a repository.
    expect(matchers.contentRefOnBranch(fileKeys.baseline("sit1", "a/b.yml"), "base", "main")).toBe(false);
  });

  it("hashes params the same whatever order their properties are in", () => {
    const client = new QueryClient();
    client.setQueryData(findingKeys.list({ limit: 5, swimlane: "sit1" }), 1);
    expect(client.getQueryData(findingKeys.list({ swimlane: "sit1", limit: 5 }))).toBe(1);
  });

  it("property: a key is JSON-safe and round-trips through JSON unchanged (10,000 cases)", () => {
    forAll(
      10_000,
      (rng) => ({ id: rng.string("abcdefghijklmnopqrstuvwxyz0123456789-", 12) || "a", path: rng.string("abc/._-é😀", 20) || "p", hash: rng.string("0123456789abcdef", 16) || "0" }),
      ({ id, path, hash }) => {
        for (const key of [fileKeys.content(id, path, hash), fileKeys.head(id, path), fileKeys.tree(id, path, null), fileKeys.history(id, path)]) {
          expect(JSON.parse(JSON.stringify(key))).toEqual(key);
        }
      },
    );
  }, 30_000);
});

describe("matchers (what a live event or a write needs to find among the keys)", () => {
  it("treeCovers: the root and every folder above a path, in that swimlane only", () => {
    const covers = (prefix: string, swimlane = "sit1") => matchers.treeCovers(fileKeys.tree(swimlane, prefix, null), "sit1", "core/deep/a.yml");
    expect(covers("")).toBe(true);
    expect(covers("core")).toBe(true);
    expect(covers("core/")).toBe(true);
    expect(covers("core/deep")).toBe(true);
    expect(covers("core/other")).toBe(false);
    expect(covers("cor")).toBe(false);
    expect(covers("core", "sit2")).toBe(false);
    expect(matchers.treeCovers(fileKeys.head("sit1", "core"), "sit1", "core/a.yml")).toBe(false);
  });

  it("namesSwimlane: a compare or a compare-tree that has the swimlane on either side, as nfs, baseline or effective", () => {
    expect(matchers.comparesSwimlane(compareKeys.diff("nfs:sit1", "git:base@main", "f", null), "sit1")).toBe(true);
    expect(matchers.comparesSwimlane(compareKeys.diff("git:base@main", "baseline:sit1", "f", null), "sit1")).toBe(true);
    expect(matchers.comparesSwimlane(compareKeys.diff("effective:sit1", "effective:sit2", "f", null), "sit2")).toBe(true);
    expect(matchers.comparesSwimlane(compareKeys.diff("nfs:sit10", "nfs:sit2", "f", null), "sit1")).toBe(false);
    expect(matchers.comparesSwimlane(compareKeys.diff("git:base@sit1", "git:tenant@sit1", "f", null), "sit1")).toBe(false);
    expect(matchers.comparesSwimlane(compareKeys.tree("nfs:sit1", "nfs:sit2", ""), "sit2")).toBe(true);
    expect(matchers.comparesSwimlane(swimlaneKeys.detail("sit1"), "sit1")).toBe(false);
  });

  it("gridHasLane: a grid that has the swimlane among its columns", () => {
    expect(matchers.gridHasLane(compareKeys.grid("f", ["sit1", "sit2"], false, null), "sit2")).toBe(true);
    expect(matchers.gridHasLane(compareKeys.grid("f", ["sit1"], false, null), "sit2")).toBe(false);
    expect(matchers.gridHasLane(fileKeys.tree("sit2", "", null), "sit2")).toBe(false);
  });

  it("contentRefOnBranch: a git read of that repo at that ref", () => {
    expect(matchers.contentRefOnBranch(fileKeys.contentRef("sit1", "f", "base", "main"), "base", "main")).toBe(true);
    expect(matchers.contentRefOnBranch(fileKeys.contentRef("sit1", "f", "tenant", "main"), "base", "main")).toBe(false);
    expect(matchers.contentRefOnBranch(fileKeys.contentRef("sit1", "f", "base", "v1.0"), "base", "main")).toBe(false);
  });

  it("tolerates a key that is not what its root says (never throws, never matches by accident)", () => {
    const odd: readonly unknown[][] = [["tree"], ["tree", "sit1"], ["grid", "f", "not-an-array"], ["compare", 1, 2], ["content-ref", "sit1"], [], [null]];
    for (const key of odd) {
      expect(() => matchers.treeCovers(key, "sit1", "a")).not.toThrow();
      expect(() => matchers.comparesSwimlane(key, "sit1")).not.toThrow();
      expect(() => matchers.gridHasLane(key, "sit1")).not.toThrow();
      expect(() => matchers.contentRefOnBranch(key, "base", "main")).not.toThrow();
    }
    expect(matchers.gridHasLane(["grid", "f", "not-an-array"], "sit1")).toBe(false);
    expect(matchers.comparesSwimlane(["compare", 1, 2], "sit1")).toBe(false);
  });
});
