import { describe, expect, it } from "vitest";
import type { PrJob, PrLink } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";
import { defaultBody, defaultTitle, duplicateOf, httpsUrl, jobSteps, prNumberOf, tokenTrouble } from "./model";

// The pure parts of the pull-request dialog: the words it starts a title and a description with, how a job's state becomes the
// four steps of the design, what a GitHub token problem means for the person, and which links the dialog will make a link of.

const problem = (status: number, kind: string, body: Record<string, unknown> = {}) => new ApiProblem({ status, kind, title: kind, requestId: "req-1", body });

describe("defaultTitle", () => {
  it("names the file and the branch for a tenant target, and says where it came from", () => {
    expect(defaultTitle({ target: "tenant", stem: "tx-infinity-core", branch: "sit7", lane: "sitb" })).toBe("Update tx-infinity-core for sit7 (from sitb)");
  });
  it("says promote for the base target", () => {
    expect(defaultTitle({ target: "base", stem: "tx-infinity-core", branch: "sit7", lane: "sitb" })).toBe("Promote tx-infinity-core from sit7 to base");
  });
  it("says update in base for a base file, which has no branch to promote from", () => {
    expect(defaultTitle({ target: "base", stem: "tx-infinity-core", branch: null, lane: "sitb" })).toBe("Update tx-infinity-core in base (from sitb)");
  });
  it("is at most 256 characters, the contract's limit", () => {
    expect(defaultTitle({ target: "tenant", stem: "x".repeat(400), branch: "sit7", lane: "sitb" }).length).toBeLessThanOrEqual(256);
  });
});

describe("defaultBody", () => {
  it("records the swimlane, the file, where it goes and the NFS hash", () => {
    const text = defaultBody({ lane: "sitb", path: "tx-infinity-api/tx-infinity-core-sit7.yml", repo: "csp/csp-tenant-data", base: "sit7", writes: "data/config/tx-infinity-api/tx-infinity-core.yml", hash: "9f3c1a7e42b0".padEnd(64, "0") });
    expect(text).toBe(
      "Raised from Lanekeeper on swimlane sitb.\n\nSwimlane: sitb (csp/csp-tenant-data@sit7)\nFile: tx-infinity-api/tx-infinity-core-sit7.yml\nTarget: csp/csp-tenant-data / data/config/tx-infinity-api/tx-infinity-core.yml\nNFS hash: 9f3c1a7e42b0",
    );
  });
});

describe("jobSteps", () => {
  const states = (state: PrJob["state"]) => jobSteps(state).map((s) => s.status);
  it("is Preparing, Committing, Opening pull request and Done", () => {
    expect(jobSteps("pending").map((s) => s.label)).toEqual(["Preparing", "Committing", "Opening pull request", "Done"]);
  });
  it("waits for the first step while the job is queued", () => {
    expect(states("pending")).toEqual(["active", "pending", "pending", "pending"]);
  });
  it("moves one step at a time, only as far as the server says", () => {
    expect(states("preparing")).toEqual(["active", "pending", "pending", "pending"]);
    expect(states("committing")).toEqual(["done", "active", "pending", "pending"]);
    expect(states("opening")).toEqual(["done", "done", "active", "pending"]);
  });
  it("is all done only when the job is completed", () => {
    expect(states("completed")).toEqual(["done", "done", "done", "done"]);
  });
  it("does not make up a step for a failed job: none is done, none is active", () => {
    expect(states("failed")).toEqual(["pending", "pending", "pending", "pending"]);
  });
});

describe("tokenTrouble", () => {
  it("is an expired token", () => {
    expect(tokenTrouble(problem(401, "token_expired"))).toBe("expired");
  });
  it("is a token that GitHub or the vault did not accept", () => {
    for (const kind of ["token_rejected", "token_unreadable", "classic_token", "not_fine_grained_token", "github_account_mismatch"]) expect(tokenTrouble(problem(422, kind))).toBe("unusable");
  });
  it("is no push access to a repository", () => {
    expect(tokenTrouble(problem(422, "token_missing_access", { base: true, tenant: false }))).toBe("no_access");
  });
  it("is nothing for any other problem, and for something that is not a problem", () => {
    expect(tokenTrouble(problem(500, "internal"))).toBeNull();
    expect(tokenTrouble(new Error("x"))).toBeNull();
  });
});

describe("duplicateOf", () => {
  const link: PrLink = { id: 1, swimlane: "sitb", path: "a.yml", nfs_hash: "a".repeat(64), state: "open", pr_url: "https://github.com/o/r/pull/418", pr_number: 418, author: "u", created_at: 1, closed_at: null };
  it("is the open PR the server named", () => {
    expect(duplicateOf(problem(409, "conflict", { reason: "duplicate_pr", link }))).toEqual(link);
  });
  it("is nothing for another conflict, or a link that is not one", () => {
    expect(duplicateOf(problem(409, "conflict", { current: null }))).toBeNull();
    expect(duplicateOf(problem(409, "conflict", { reason: "duplicate_pr", link: { pr_number: "x" } }))).toBeNull();
    expect(duplicateOf(problem(409, "conflict", { reason: "duplicate_pr" }))).toBeNull();
  });
});

describe("httpsUrl", () => {
  it("accepts an https URL", () => {
    expect(httpsUrl("https://github.com/o/r/pull/1")).toBe("https://github.com/o/r/pull/1");
  });
  it("refuses anything else: javascript:, data:, http:, a relative path, junk", () => {
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "http://github.com/x", "/pull/1", "", "https://", "not a url", "https://a b"]) expect(httpsUrl(bad)).toBeNull();
  });
});

describe("prNumberOf", () => {
  it("reads the number at the end of a pull-request URL", () => {
    expect(prNumberOf("https://github.com/o/r/pull/419")).toBe(419);
    expect(prNumberOf("https://github.com/o/r/pull/419/")).toBe(419);
  });
  it("is nothing for a URL that does not end like one", () => {
    expect(prNumberOf("https://github.com/o/r")).toBeNull();
  });
});
