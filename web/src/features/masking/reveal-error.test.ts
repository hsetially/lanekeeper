import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import { revealError } from "./reveal-error";

// What a failed reveal says inside its dialog (S21: the problem's title and the request id are the server's words that are shown). A
// reveal is audited BEFORE the value leaves, and a retry is a second audited reveal, so the words differ by whether a record could
// exist: a refusal (403, 404, 429, 503) shows nothing and records nothing; a lost answer (no response, a timeout, a 500) may have
// recorded a reveal that nobody saw, and trying again records another.

const problem = (status: number, kind: string, extra: { retryAfterSeconds?: number; requestId?: string } = {}) => new ApiProblem({ status, kind, title: `Title ${status}`, ...extra });

describe("revealError", () => {
  it("keeps the problem's title and request id, and nothing else of the server's words", () => {
    const info = revealError(new ApiProblem({ status: 404, kind: "not_found", title: "Not found", requestId: "req-1", detail: "the value was hunter2" }));
    expect(info.title).toBe("Not found");
    expect(info.requestId).toBe("req-1");
    expect(JSON.stringify(info)).not.toContain("hunter2");
  });

  it("says how long to wait after 429, from Retry-After", () => {
    const info = revealError(problem(429, "too_many_requests", { retryAfterSeconds: 37 }));
    expect(info.detail).toMatch(/30 .*a minute/);
    expect(info.detail).toContain("37 seconds");
    expect(info.detail).toMatch(/nothing was shown/i);
  });

  it("says to wait a moment when the server did not say how long", () => {
    expect(revealError(problem(429, "too_many_requests")).detail).toContain("a moment");
    expect(revealError(problem(429, "too_many_requests", { retryAfterSeconds: 1 })).detail).toContain("Wait 1 second,");
  });

  it("says the audit log could not take the record after 503, and that nothing was shown", () => {
    const info = revealError(problem(503, "service_unavailable", { retryAfterSeconds: 5 }));
    expect(info.detail).toMatch(/record/i);
    expect(info.detail).toMatch(/nothing was shown/i);
    expect(info.detail).toContain("5 seconds");
  });

  it("says there is nothing to reveal after 404 (one answer for a missing file, a missing setting and an unflagged value)", () => {
    expect(revealError(problem(404, "not_found")).detail).toMatch(/nothing to reveal/i);
  });

  it("says the role does not allow it after 403", () => {
    expect(revealError(problem(403, "forbidden")).detail).toMatch(/role/i);
  });

  it("warns that a lost answer may have been recorded, and that another try is another record", () => {
    for (const info of [revealError(problem(0, "timeout")), revealError(problem(0, "network")), revealError(problem(500, "internal")), revealError(problem(502, "bad_gateway"))]) {
      expect(info.detail, info.title).toMatch(/may have recorded/i);
      expect(info.detail, info.title).toMatch(/another record|second record/i);
    }
  });

  it("does not warn about a record after a refusal that recorded nothing", () => {
    for (const status of [400, 403, 404, 429, 503]) expect(revealError(problem(status, "x")).detail ?? "", String(status)).not.toMatch(/may have recorded/i);
  });

  it("copes with an error that is not a problem", () => {
    expect(revealError(new Error("boom with hunter2"))).toEqual({ title: "The value could not be revealed", detail: "Nothing was shown." });
    expect(revealError("x")).toEqual({ title: "The value could not be revealed", detail: "Nothing was shown." });
  });
});
