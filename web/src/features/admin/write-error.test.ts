import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import { adminWriteError } from "./write-error";

// What a failed admin write says inside the dialog that made it: the problem's title, a sentence of this screen's own for the
// status, and the request id. Never what the server wrote in `detail` (S21).

const problem = (status: number, extra: { detail?: string; requestId?: string; kind?: string } = {}) =>
  new ApiProblem({ status, kind: extra.kind ?? "unknown", title: `Title ${status}`, requestId: extra.requestId, detail: extra.detail });

describe("adminWriteError", () => {
  it("keeps the problem's title and request id, and says what to do for the status", () => {
    const info = adminWriteError(problem(403, { requestId: "req-9" }), "Could not save");
    expect(info.title).toBe("Title 403");
    expect(info.requestId).toBe("req-9");
    expect(info.detail).toMatch(/role may have changed/i);
  });

  it("never shows the detail the server wrote", () => {
    const info = adminWriteError(problem(422, { detail: "column webhook_url violates a check on table notifications" }), "Could not save");
    expect(JSON.stringify(info)).not.toContain("violates");
    expect(info.detail).toMatch(/did not accept/i);
  });

  it("uses the screen's own sentence for a status when it has one", () => {
    expect(adminWriteError(problem(409), "x", { 409: "This request was already decided." }).detail).toBe("This request was already decided.");
  });

  it("says nothing was changed when the hub could not be reached, and that the change may have been made", () => {
    const info = adminWriteError(new ApiProblem({ status: 0, kind: "network", title: "Network error" }), "Could not save");
    expect(info.detail).toMatch(/could not be reached/i);
    expect(info.detail).toMatch(/reload/i);
  });

  it("has a sentence for 404, 423, 429 and the 5xx family", () => {
    expect(adminWriteError(problem(404), "x").detail).toMatch(/no longer exists/i);
    expect(adminWriteError(problem(423), "x").detail).toMatch(/locked/i);
    expect(adminWriteError(problem(429), "x").detail).toMatch(/wait/i);
    expect(adminWriteError(problem(503), "x").detail).toMatch(/could not complete/i);
  });

  it("shows only the fallback for an error that is not a problem (its message could hold anything)", () => {
    const info = adminWriteError(new Error("secret token lkjt_abc in a message"), "Could not save");
    expect(info).toEqual({ title: "Could not save" });
  });
});
