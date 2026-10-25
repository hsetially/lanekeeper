import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import { outcomeFor } from "./token-outcomes";

// The code-to-state table of the GitHub token screens (plan T4, S1). The hub refuses a token with a problem whose `type` is the
// code (hub-writes/src/routes/me.rs); each code is one state of the screen. A code the table does not know is an error
// with the request id, never a guess.

const problem = (status: number, kind: string, body: Record<string, unknown> = {}) => new ApiProblem({ status, kind, title: `title of ${kind}`, requestId: "req-1", body });

describe("outcomeFor", () => {
  it.each([["classic_token"], ["not_fine_grained_token"]])("shows %s as the classic-token state", (kind) => {
    expect(outcomeFor(problem(422, kind)).kind).toBe("classic");
  });

  it("shows token_missing_access as the repository state, with which repositories the token can push to", () => {
    expect(outcomeFor(problem(422, "token_missing_access", { base: true, tenant: false }))).toMatchObject({ kind: "repo", base: true, tenant: false });
    expect(outcomeFor(problem(422, "token_missing_access", { base: false, tenant: true }))).toMatchObject({ kind: "repo", base: false, tenant: true });
    expect(outcomeFor(problem(422, "token_missing_access", { base: false, tenant: false }))).toMatchObject({ kind: "repo", base: false, tenant: false });
  });

  it("does not guess a repository the hub did not name", () => {
    expect(outcomeFor(problem(422, "token_missing_access", { base: "yes" }))).toMatchObject({ kind: "repo", base: undefined, tenant: undefined });
    expect(outcomeFor(problem(422, "token_missing_access"))).toMatchObject({ kind: "repo", base: undefined, tenant: undefined });
  });

  it("shows github_account_mismatch as the mismatch state", () => {
    expect(outcomeFor(problem(422, "github_account_mismatch")).kind).toBe("mismatch");
  });

  it("shows idempotency_conflict as a try again", () => {
    expect(outcomeFor(problem(422, "idempotency_conflict")).kind).toBe("retry");
  });

  it.each([
    ["token_rejected", 422, "rejected"],
    ["unavailable", 503, "unavailable"],
    ["bad_request", 400, "bad_request"],
    ["forbidden", 403, "forbidden"],
    ["too_many_requests", 429, "rate_limited"],
    ["token_expired", 422, "other"],
    ["token_unreadable", 422, "other"],
    ["something_new", 418, "other"],
  ])("shows %s (%i) as an error with the request id (%s)", (kind, status, reason) => {
    const outcome = outcomeFor(problem(status, kind));
    expect(outcome).toMatchObject({ kind: "error", reason });
    expect(outcome.kind === "error" && outcome.problem.requestId).toBe("req-1");
  });

  it("goes by the status when the code is one it has not seen", () => {
    expect(outcomeFor(problem(503, "brand_new_code"))).toMatchObject({ kind: "error", reason: "unavailable" });
    expect(outcomeFor(problem(400, "brand_new_code"))).toMatchObject({ kind: "error", reason: "bad_request" });
  });

  it("shows a request that never got an answer as a network error", () => {
    expect(outcomeFor(new ApiProblem({ status: 0, kind: "network", title: "Could not reach the server" }))).toMatchObject({ kind: "error", reason: "network" });
    expect(outcomeFor(new ApiProblem({ status: 0, kind: "timeout", title: "The server took too long" }))).toMatchObject({ kind: "error", reason: "network" });
  });

  it("makes a problem of anything else that was thrown, without its message (it could hold anything)", () => {
    const outcome = outcomeFor(new Error("secret ghp_ABC in here"));
    expect(outcome.kind).toBe("error");
    expect(JSON.stringify(outcome)).not.toContain("ghp_ABC");
  });
});
