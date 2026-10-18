import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import { dialogOf, failureOf } from "./failure";

const problem = (status: number, kind: string, body: Record<string, unknown> = {}, extra: { retryAfterSeconds?: number } = {}): ApiProblem =>
  new ApiProblem({ status, kind, title: `title of ${kind}`, requestId: "req-1", detail: "SELECT * FROM secrets", body, ...extra });

describe("a decision the server refused, said inside the drawer", () => {
  it("says an own proposal can not be decided by its author, and that nothing was recorded", () => {
    const info = failureOf(problem(403, "forbidden"), { verb: "approve", own: true });
    expect(info).toEqual({ title: "You can't decide your own proposal", detail: "Another Admin has to. Nothing was recorded.", requestId: "req-1" });
  });

  it("says a 403 for anyone else is a role", () => {
    const info = failureOf(problem(403, "insufficient-role"), { verb: "reject", own: false });
    expect(info?.title).toBe("You may not decide proposals");
    expect(info?.detail).toBe("Deciding needs the Admin role. Nothing was recorded.");
  });

  it("says 404 and 422 not_allowed mean the proposal moved on, and that the decision was not recorded", () => {
    expect(failureOf(problem(404, "not_found"), { verb: "approve", own: false })?.title).toBe("This proposal was not found");
    const gone = failureOf(problem(422, "unprocessable", { reason: "not_allowed" }), { verb: "approve", own: false });
    expect(gone?.title).toBe("This proposal can't be decided now");
    expect(gone?.detail).toBe("It is no longer pending: someone else decided it, or it expired. Your decision was not recorded.");
  });

  it("says why a write was refused with words of its own for each reason, never the server's text", () => {
    const reasons: Array<[string, string]> = [
      ["denied", "The write is not allowed on this path."],
      ["too_large", "The file is too large to write."],
      ["invalid_content", "The proposed content is not valid."],
      ["unknown_swimlane", "The swimlane no longer exists."],
      ["agent_unavailable", "The swimlane's agent can't be reached, so nothing could be written. Try again when it is back."],
      ["secret_detected", "The proposed content holds a secret, so it can't be written."],
      ["parse_error", "The proposed content does not parse."],
    ];
    for (const [reason, text] of reasons) {
      const info = failureOf(problem(422, "unprocessable", { reason }), { verb: "approve", own: false });
      expect(info?.detail, reason).toBe(`${text} Nothing was applied.`);
    }
    const unknown = failureOf(problem(422, "unprocessable", { reason: "something_new" }), { verb: "approve", own: false });
    expect(unknown?.detail).toBe("The server did not accept this decision. Nothing was applied.");
  });

  it("never shows what the server wrote in `detail`", () => {
    for (const status of [0, 400, 403, 404, 422, 429, 500, 503]) {
      const info = failureOf(problem(status, "x"), { verb: "approve", own: false });
      expect(JSON.stringify(info), String(status)).not.toContain("SELECT");
    }
  });

  it("says a decision that got no answer may or may not have been recorded", () => {
    const info = failureOf(problem(0, "network"), { verb: "approve", own: false });
    expect(info?.title).toBe("Couldn't reach the server");
    expect(info?.detail).toBe("The decision may or may not have been recorded. Open the proposal again to see where it stands.");
  });

  it("says how long to wait after 429, and that a server error leaves it unknown", () => {
    expect(failureOf(problem(429, "too_many_requests", {}, { retryAfterSeconds: 12 }), { verb: "approve", own: false })?.detail).toBe("Wait 12 seconds, then try again.");
    expect(failureOf(problem(429, "too_many_requests"), { verb: "approve", own: false })?.detail).toBe("Wait a moment, then try again.");
    expect(failureOf(problem(503, "unavailable"), { verb: "reject", own: false })?.detail).toBe("The server could not finish the decision. Open the proposal again to see where it stands.");
  });

  it("leaves 409 and 423 to their dialogs", () => {
    expect(failureOf(problem(409, "conflict", { current: "a".repeat(64) }), { verb: "approve", own: false })).toBeNull();
    expect(failureOf(problem(423, "locked", { until: 1 }), { verb: "approve", own: false })).toBeNull();
  });
});

describe("a decision that gets a dialog of its own", () => {
  it("is the conflict (the file moved) and the lock", () => {
    expect(dialogOf(problem(409, "conflict"))).toBe("conflict");
    expect(dialogOf(problem(423, "locked"))).toBe("locked");
    expect(dialogOf(problem(422, "unprocessable"))).toBeNull();
    expect(dialogOf(null)).toBeNull();
  });
});
