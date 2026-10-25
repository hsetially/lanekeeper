import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import type { BaselineStatus } from "@/api/generated/model";
import { restartFailure } from "./write-error";

const problem = (status: number, kind: string, extra: Partial<ConstructorParameters<typeof ApiProblem>[0]> = {}) =>
  new ApiProblem({ status, kind, title: `server says ${kind}`, requestId: "req-9", ...extra });
const confirmed: BaselineStatus = { state: "confirmed", by: null, at: 1 };
const unconfirmed: BaselineStatus = { state: "unconfirmed", by: null, at: null };
const ctx = (baseline: BaselineStatus | null | undefined = confirmed) => ({ swimlane: "sitb", baseline });

describe("restartFailure", () => {
  it("names the baseline as the cause of a 423 when it is not confirmed", () => {
    const info = restartFailure(problem(423, "locked"), ctx(unconfirmed));
    expect(info.title).toBe("sitb is locked until it is adopted");
    expect(info.detail).toContain("baseline is unconfirmed");
    expect(info.requestId).toBe("req-9");
  });

  it("says only that something else holds it when the baseline is confirmed, and when it ends if the server says", () => {
    const info = restartFailure(problem(423, "locked", { body: { until: 1_760_000_000_000 } }), ctx());
    expect(info.title).toBe("sitb is locked");
    expect(info.detail).toContain("2025-10-09 08:53 UTC");
  });

  it("ignores an `until` that is not a time", () => {
    expect(restartFailure(problem(423, "locked", { body: { until: "tomorrow" } }), ctx()).detail).not.toContain("tomorrow");
  });

  it("says nothing was restarted for a refusal, and uses the server's title only for what it is", () => {
    expect(restartFailure(problem(422, "unprocessable"), ctx())).toEqual({ title: "server says unprocessable", detail: "The server did not accept this restart. Nothing was restarted.", requestId: "req-9" });
  });

  it("says the swimlane is gone for a 404", () => {
    expect(restartFailure(problem(404, "not_found"), ctx()).title).toBe("sitb was not found");
  });

  it("names the role for a 403", () => {
    expect(restartFailure(problem(403, "forbidden"), ctx()).detail).toContain("Operator");
  });

  it("does not claim the restart was not sent when nothing came back", () => {
    const info = restartFailure(problem(0, "network", { requestId: undefined }), ctx());
    expect(info.title).toBe("Couldn't reach the server");
    expect(info.detail).toContain("may or may not have been sent");
    expect(info).not.toHaveProperty("requestId");
  });

  it("does not claim it either for a server error", () => {
    expect(restartFailure(problem(503, "unavailable"), ctx()).detail).toContain("Check the service");
  });

  it("says how long to wait when the server asks", () => {
    expect(restartFailure(problem(429, "too_many_requests", { retryAfterSeconds: 30 }), ctx()).detail).toContain("30 seconds");
  });

  it("never shows what the server wrote in `detail`", () => {
    const info = restartFailure(problem(422, "unprocessable", { detail: "SELECT * FROM secrets" }), ctx());
    expect(JSON.stringify(info)).not.toContain("SELECT");
  });
});
