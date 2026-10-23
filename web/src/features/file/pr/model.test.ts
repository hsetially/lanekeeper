import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import { defaultBody, defaultTitle, tokenTrouble } from "./model";

// The pure parts of the pull-request dialog: the words it starts a title and a description with, and what a GitHub token problem
// means for the person. (The job's steps, the open pull request a 409 names and a pull request's address are tested with the shared
// pieces: src/components/pr/job-steps.test.ts, pr-url.test.ts and src/features/prs/duplicate.test.ts.)

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
