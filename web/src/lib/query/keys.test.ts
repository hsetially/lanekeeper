import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { IMMUTABLE_ROOTS, isImmutableKey, keyRoot, keys } from "./keys";

// The vocabulary of query keys (plan, "Query keys, retry and SSE invalidation"). A key is a pure function of what the
// request depends on, so a cached answer is safe to reuse; the two roots that are functions of a content hash are immutable
// and are never refetched on a resync. Every key must also be what TanStack's partial matching expects, because the live
// events (T3) invalidate by prefix.

const all = (): Array<[string, readonly unknown[]]> => [
  ["me", keys.me()],
  ["swimlanes", keys.swimlanes()],
  ["swimlane", keys.swimlane("sit1")],
  ["pickup", keys.pickup("sit1")],
  ["tree", keys.tree("sit1", "a/b", null)],
  ["file-head", keys.fileHead("sit1", "a/b.yml")],
  ["content", keys.content("sit1", "a/b.yml", "h1")],
  ["blob", keys.blob("h1")],
  ["content-ref", keys.contentRef("sit1", "a/b.yml", "base", "main")],
  ["compare", keys.compare("nfs:sit1", "git:base@main", "a/b.yml", null)],
  ["history", keys.history("sit1", "a/b.yml")],
  ["grid", keys.grid("a/b.yml", ["sit1", "sit2"], false, null)],
  ["compare-tree", keys.compareTree("nfs:sit1", "nfs:sit2", "a")],
  ["git-refs", keys.gitRefs("base", "tag", "v1")],
  ["findings", keys.findings({ limit: 50 })],
  ["proposals", keys.proposals({ limit: 50 })],
  ["proposal", keys.proposal("12")],
  ["prs", keys.prs({ limit: 50 })],
  ["pr-job", keys.prJob("9")],
  ["docs", keys.docs({ limit: 50 })],
  ["doc", keys.doc("d1")],
  ["features", keys.features({ flag: "auto-notify" })],
  ["audit", keys.audit({ limit: 50 })],
  ["activity", keys.activity("sit1")],
  ["me-github-credential", keys.meGithubCredential()],
  ["me-mcp-tokens", keys.meMcpTokens()],
  ["admin", keys.admin("users", { limit: 50 })],
];

describe("keys", () => {
  it("has a root for every cache the plan lists, and the root is the first element", () => {
    const roots = all().map(([name]) => name);
    expect(roots).toEqual([
      "me", "swimlanes", "swimlane", "pickup", "tree", "file-head", "content", "blob", "content-ref", "compare", "history", "grid",
      "compare-tree", "git-refs", "findings", "proposals", "proposal", "prs", "pr-job", "docs", "doc", "features", "audit", "activity",
      "me-github-credential", "me-mcp-tokens", "admin",
    ]);
    for (const [name, key] of all()) {
      expect(key[0], name).toBe(name);
      expect(keyRoot(key), name).toBe(name);
    }
  });

  it("makes the same key for the same input and a different key for a different one", () => {
    expect(keys.tree("sit1", "a", null)).toEqual(keys.tree("sit1", "a", null));
    expect(keys.tree("sit1", "a", null)).not.toEqual(keys.tree("sit1", "a", "git_ahead"));
    expect(keys.tree("sit1", "a", null)).not.toEqual(keys.tree("sit2", "a", null));
    expect(keys.compare("nfs:sit1", "nfs:sit2", "f", null)).not.toEqual(keys.compare("nfs:sit1", "nfs:sit2", "f", "ch1"));
  });

  it("does not let a caller's later change reach a key that was made from it", () => {
    const lanes = ["sit1", "sit2"];
    const key = keys.grid("f", lanes, true, null);
    lanes.push("sit3");
    expect(JSON.stringify(key)).not.toContain("sit3");
  });

  it("puts a file's content under the hash, so a changed file is a different key and an old one is never stale", () => {
    expect(keys.content("sit1", "a.yml", "h1")).not.toEqual(keys.content("sit1", "a.yml", "h2"));
    expect(isImmutableKey(keys.content("sit1", "a.yml", "h1"))).toBe(true);
    expect(isImmutableKey(keys.blob("h1"))).toBe(true);
  });

  it("marks exactly the content and blob roots as immutable", () => {
    expect([...IMMUTABLE_ROOTS].sort()).toEqual(["blob", "content"]);
    for (const [name, key] of all()) expect(isImmutableKey(key), name).toBe(name === "content" || name === "blob");
  });

  it("does NOT mark compare or content-ref as immutable: they are keyed by refs, and a ref moves", () => {
    expect(isImmutableKey(keys.compare("nfs:sit1", "git:base@main", "f", null))).toBe(false);
    expect(isImmutableKey(keys.contentRef("sit1", "f", "base", "main"))).toBe(false);
  });

  it("is what partial matching expects: a prefix finds every key under it (the live events invalidate by prefix)", () => {
    const client = new QueryClient();
    client.setQueryData(keys.tree("sit1", "a", null), 1);
    client.setQueryData(keys.tree("sit1", "a/b", "git_ahead"), 2);
    client.setQueryData(keys.tree("sit2", "a", null), 3);
    client.setQueryData(keys.swimlanes(), 4);
    const under = (key: readonly unknown[]) => client.getQueryCache().findAll({ queryKey: key as unknown[] }).length;
    expect(under(["tree", "sit1"])).toBe(2);
    expect(under(["tree"])).toBe(3);
    expect(under(keys.swimlanes())).toBe(1);
    expect(under(["swimlane"])).toBe(0);
  });

  it("hashes params the same whatever order their properties are in", () => {
    const client = new QueryClient();
    client.setQueryData(keys.findings({ limit: 5, swimlane: "sit1" }), 1);
    expect(client.getQueryData(keys.findings({ swimlane: "sit1", limit: 5 }))).toBe(1);
  });

  it("property: a key is JSON-safe and round-trips through JSON unchanged (10,000 cases)", () => {
    forAll(
      10_000,
      (rng) => ({ id: rng.string("abcdefghijklmnopqrstuvwxyz0123456789-", 12) || "a", path: rng.string("abc/._-é😀", 20) || "p", hash: rng.string("0123456789abcdef", 16) || "0" }),
      ({ id, path, hash }) => {
        for (const key of [keys.content(id, path, hash), keys.fileHead(id, path), keys.tree(id, path, null), keys.history(id, path)]) {
          expect(JSON.parse(JSON.stringify(key))).toEqual(key);
        }
      },
    );
  }, 30_000);
});
