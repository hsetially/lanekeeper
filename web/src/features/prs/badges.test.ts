import { describe, expect, it } from "vitest";
import type { PrLink } from "@/api/generated/model";
import { buildPrIndex, prBadgeFor } from "./badges";

// "PR open" and "Fix merged, awaiting sync" are derived from the pull-request links of a swimlane, joined on the path. The label
// "awaiting sync" has no column on the server (crates/hub-writes/src/prs/links.rs): it is a merged link for a file whose NFS
// content still differs from Git, so a merged link on a file that is in sync says nothing.

const HASH = "a".repeat(64);
const OTHER = "b".repeat(64);
const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";

const link = (over: Partial<PrLink> = {}): PrLink => ({
  id: 1,
  swimlane: "sitb",
  path: PATH,
  nfs_hash: HASH,
  state: "open",
  pr_url: "https://github.com/csp/csp-tenant-data/pull/418",
  pr_number: 418,
  author: "t:u",
  created_at: 1_760_000_000_000,
  closed_at: null,
  ...over,
});

const find = (links: PrLink[], facts: Parameters<typeof prBadgeFor>[1]) => prBadgeFor(buildPrIndex("sitb", links), facts);

describe("an open pull request", () => {
  it("makes the PR open badge of its file", () => {
    expect(find([link()], { path: PATH })).toEqual({ kind: "open", link: link(), outdated: false });
  });

  it("is for its own file only", () => {
    expect(find([link()], { path: "application.properties" })).toBeNull();
  });

  it("is for its own swimlane only: a link of another swimlane is ignored", () => {
    expect(find([link({ swimlane: "sitc" })], { path: PATH })).toBeNull();
  });

  it("says it was raised for an earlier version when the file has changed since", () => {
    expect(find([link()], { path: PATH, hash: OTHER })).toMatchObject({ kind: "open", outdated: true });
    expect(find([link()], { path: PATH, hash: HASH })).toMatchObject({ kind: "open", outdated: false });
    expect(find([link()], { path: PATH, hash: null })).toMatchObject({ kind: "open", outdated: false });
  });

  it("prefers the one raised for the content the file has now, then the newest", () => {
    const older = link({ id: 5, nfs_hash: HASH, pr_number: 400 });
    const newer = link({ id: 9, nfs_hash: OTHER, pr_number: 420 });
    expect(find([older, newer], { path: PATH, hash: HASH })).toMatchObject({ link: { pr_number: 400 }, outdated: false });
    expect(find([older, newer], { path: PATH })).toMatchObject({ link: { pr_number: 420 } });
  });
});

describe("a merged pull request", () => {
  const merged = link({ state: "merged", closed_at: 1_760_000_100_000 });

  it("makes the Fix merged, awaiting sync badge of a file that still differs", () => {
    for (const drift of ["nfs_ahead", "git_ahead", "conflict", "unknown"] as const) {
      expect(find([merged], { path: PATH, drift }), drift).toEqual({ kind: "merged", link: merged, outdated: false });
    }
  });

  it("says nothing about a file that is in sync, or intentionally different, or whose state is not known", () => {
    expect(find([merged], { path: PATH, drift: "in_sync" })).toBeNull();
    expect(find([merged], { path: PATH, drift: "intentional_divergence" })).toBeNull();
    expect(find([merged], { path: PATH })).toBeNull();
    expect(find([merged], { path: PATH, drift: null })).toBeNull();
  });

  it("says nothing when the file has changed since the pull request was raised: the merge is not this change", () => {
    expect(find([merged], { path: PATH, drift: "nfs_ahead", hash: OTHER })).toBeNull();
    expect(find([merged], { path: PATH, drift: "nfs_ahead", hash: HASH })).toMatchObject({ kind: "merged" });
  });

  it("is shown for a file the caller knows differs without knowing how (a finding of the drift check)", () => {
    expect(find([merged], { path: PATH, differs: true })).toMatchObject({ kind: "merged" });
    expect(find([merged], { path: PATH, differs: false })).toBeNull();
    // A state, when there is one, is what decides: `differs` only stands in for it.
    expect(find([merged], { path: PATH, drift: "in_sync", differs: true })).toBeNull();
    expect(find([merged], { path: PATH, drift: "nfs_ahead", differs: false })).toMatchObject({ kind: "merged" });
  });

  it("gives way to an open pull request of the same file", () => {
    const open = link({ id: 2, pr_number: 430 });
    expect(find([merged, open], { path: PATH, drift: "nfs_ahead" })).toMatchObject({ kind: "open", link: { pr_number: 430 } });
  });
});

describe("a closed pull request", () => {
  it("makes no badge: nothing carries the change", () => {
    expect(find([link({ state: "closed", closed_at: 5 })], { path: PATH, drift: "nfs_ahead" })).toBeNull();
  });
});

describe("buildPrIndex", () => {
  it("is empty for no links", () => {
    expect(find([], { path: PATH, drift: "nfs_ahead" })).toBeNull();
  });
});
