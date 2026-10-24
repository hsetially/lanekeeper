import { describe, expect, it } from "vitest";
import type { BaselineStatus } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";
import { notifyFailure, offerNeedsOperator, offerReadFailure } from "./write-error";

// What a refused notification says inside its dialog: what happened, why, and what to do next, in words the screen chose. The
// server's own `detail` is never shown (S21), and a write that got no answer is not reported as "not sent".

const problem = (status: number, kind: string, extra: Partial<ConstructorParameters<typeof ApiProblem>[0]> = {}) =>
  new ApiProblem({ status, kind, title: `server says ${kind}`, requestId: "req-9", ...extra });
const confirmed: BaselineStatus = { state: "confirmed", by: null, at: 1 };
const unconfirmed: BaselineStatus = { state: "unconfirmed", by: null, at: null };
const inProgress: BaselineStatus = { state: "in_progress", by: null, at: null };
const ctx = (baseline: BaselineStatus | null | undefined = confirmed) => ({ swimlane: "sitb", baseline });

describe("notifyFailure", () => {
  it("names the baseline as the cause of a 423 when it is not confirmed, and says nothing was sent", () => {
    const info = notifyFailure(problem(423, "locked"), ctx(unconfirmed));
    expect(info.title).toBe("sitb is locked until it is adopted");
    expect(info.detail).toContain("baseline is unconfirmed");
    expect(info.detail).toContain("Nothing was sent");
    expect(info.requestId).toBe("req-9");
  });

  it("names an adoption in progress", () => {
    const info = notifyFailure(problem(423, "locked"), ctx(inProgress));
    expect(info.title).toBe("sitb has an adoption in progress");
    expect(info.detail).toContain("Nothing was sent");
  });

  it("says only that something else holds the swimlane when the baseline is confirmed, and when it ends if the server says", () => {
    const info = notifyFailure(problem(423, "locked", { body: { until: 1_760_000_000_000 } }), ctx());
    expect(info.title).toBe("sitb is locked");
    expect(info.detail).toContain("2025-10-09 08:53 UTC");
  });

  it("ignores an `until` that is not a time", () => {
    expect(notifyFailure(problem(423, "locked", { body: { until: "tomorrow" } }), ctx()).detail).not.toContain("tomorrow");
  });

  it("does not guess a cause when the server did not state the baseline", () => {
    const info = notifyFailure(problem(423, "locked"), ctx(null));
    expect(info.title).toBe("sitb is locked");
    expect(info.detail).not.toContain("baseline");
  });

  it.each([400, 422])("says the server did not accept it for a %i, and that nothing was sent", (status) => {
    expect(notifyFailure(problem(status, "unprocessable"), ctx())).toEqual({ title: "server says unprocessable", detail: "The server did not accept this notification. Nothing was sent.", requestId: "req-9" });
  });

  it("says that someone changed it first for a 409, and that nothing was sent", () => {
    const info = notifyFailure(problem(409, "conflict"), ctx());
    expect(info.detail).toContain("Nothing was sent");
  });

  it("names the role for a 403", () => {
    expect(notifyFailure(problem(403, "forbidden"), ctx()).detail).toContain("Operator");
  });

  it("says the swimlane is gone for a 404", () => {
    expect(notifyFailure(problem(404, "not_found"), ctx()).title).toBe("sitb was not found");
  });

  it("does not claim the notification was not sent when nothing came back", () => {
    const info = notifyFailure(problem(0, "network", { requestId: undefined }), ctx());
    expect(info.title).toBe("Couldn't reach the server");
    expect(info.detail).toContain("may or may not have been sent");
    expect(info).not.toHaveProperty("requestId");
  });

  it("does not claim it either for a server error", () => {
    const info = notifyFailure(problem(503, "unavailable"), ctx());
    expect(info.detail).toContain("may or may not have been sent");
    expect(info.detail).not.toContain("Nothing was sent");
  });

  it("says how long to wait when the server asks, and that nothing was sent", () => {
    const info = notifyFailure(problem(429, "too_many_requests", { retryAfterSeconds: 30 }), ctx());
    expect(info.detail).toContain("30 seconds");
    expect(notifyFailure(problem(429, "too_many_requests"), ctx()).detail).toBe("Wait a moment, then try again.");
  });

  it("never shows what the server wrote in `detail`", () => {
    const info = notifyFailure(problem(422, "unprocessable", { detail: "SELECT * FROM secrets" }), ctx());
    expect(JSON.stringify(info)).not.toContain("SELECT");
  });
});

describe("offerReadFailure", () => {
  const what = "the config-server restart offer";

  it("names what could not be read and, for a 403, the role it needs", () => {
    const info = offerReadFailure(problem(403, "forbidden"), what);
    expect(info.title).toBe("Couldn't read the config-server restart offer");
    expect(info.detail).toBe("Reading it needs the Operator role.");
    expect(info.requestId).toBe("req-9");
  });

  it("says the same without a request when the person is below Operator", () => {
    expect(offerNeedsOperator(what)).toEqual({ title: "Couldn't read the config-server restart offer", detail: "Reading it needs the Operator role." });
  });

  it("says when nothing reached the server, and that a retry may help", () => {
    const info = offerReadFailure(problem(0, "network", { requestId: undefined }), what);
    expect(info.detail).toBe("The server could not be reached. Check the connection and retry.");
    expect(info).not.toHaveProperty("requestId");
  });

  it("says a server error may pass, and a refusal will not", () => {
    expect(offerReadFailure(problem(503, "unavailable"), what).detail).toBe("The server returned an error. Retry, or send the request ID to the platform team.");
    expect(offerReadFailure(problem(400, "bad_request"), what).detail).toBe("The request was refused. Send the request ID to the platform team.");
  });

  it("never shows what the server wrote in `detail`", () => {
    expect(JSON.stringify(offerReadFailure(problem(500, "internal", { detail: "SELECT * FROM secrets" }), what))).not.toContain("SELECT");
  });
});
