import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import { errorInfo, fromProblem, fromResult, issueLine, issueMessage, type WriteOutcome } from "./outcome";

// What a write answered, as the screen needs to tell it: applied, sent for approval (the 200 with outcome proposal_created),
// 409 (someone else changed it first), 423 (locked), 422 (the content was refused), too large, or a failure. The problem's
// extension members are untrusted input, so each is checked before it is used; `detail` is never used.

const HASH = "b".repeat(64);

function problem(status: number, kind: string, body: Record<string, unknown> = {}): ApiProblem {
  return new ApiProblem({ status, kind, title: `title of ${kind}`, requestId: "req-1", detail: "SELECT * FROM secrets", body: { status, ...body } });
}

describe("fromResult", () => {
  it("is applied, with the new hash and the warnings that did not stop the write", () => {
    const out = fromResult({ outcome: "applied", hash: HASH, audit_id: 7, warnings: [{ kind: "duplicate_key", line: 90 }] });
    expect(out).toEqual({ kind: "applied", hash: HASH, warnings: [{ kind: "duplicate_key", line: 90 }] });
  });
  it("is applied with no warnings when there are none", () => {
    expect(fromResult({ outcome: "applied", hash: null, audit_id: 7 })).toEqual({ kind: "applied", hash: null, warnings: [] });
  });
  it("is sent for approval when the answer is a proposal: the write has NOT been applied", () => {
    expect(fromResult({ outcome: "proposal_created", id: 2041 })).toEqual({ kind: "sent", proposal: 2041 });
  });
});

describe("fromProblem", () => {
  it("is a conflict for 409, with the hash that is on NFS now", () => {
    const out = fromProblem(problem(409, "conflict", { current: HASH }));
    expect(out).toEqual({ kind: "conflict", current: HASH, requestId: "req-1" });
  });
  it("is a conflict with no current hash when the file is gone", () => {
    expect(fromProblem(problem(409, "conflict", { current: null }))).toMatchObject({ kind: "conflict", current: null });
  });
  it("does not trust a current hash that is not a hash", () => {
    expect(fromProblem(problem(409, "conflict", { current: "<img src=x onerror=1>" }))).toMatchObject({ kind: "conflict", current: null });
  });
  it("is not a conflict when the same request was sent with other content", () => {
    expect(fromProblem(problem(409, "idempotency_conflict"))).toMatchObject({ kind: "failed", status: 409 });
  });
  it("is locked for 423, with when it ends if the server says", () => {
    expect(fromProblem(problem(423, "locked", { until: 1_760_000_000_000 }))).toEqual({ kind: "locked", until: 1_760_000_000_000, requestId: "req-1" });
    expect(fromProblem(problem(423, "locked", { until: null }))).toMatchObject({ kind: "locked", until: null });
    expect(fromProblem(problem(423, "locked", { until: "soon" }))).toMatchObject({ kind: "locked", until: null });
  });
  it("is invalid for 422 with issues, keeping lines and rule names and nothing else", () => {
    const out = fromProblem(problem(422, "unprocessable", { reason: "secret_detected", issues: [{ kind: "secret_match", line: 120, rule: "private_key", text: "-----BEGIN" }, { kind: "bogus", line: 1 }, { kind: "parse_error", line: 88 }] })) as Extract<WriteOutcome, { kind: "invalid" }>;
    expect(out.kind).toBe("invalid");
    expect(out.reason).toBe("secret_detected");
    expect(out.issues).toEqual([{ kind: "secret_match", line: 120, rule: "private_key" }, { kind: "parse_error", line: 88 }]);
    expect(JSON.stringify(out)).not.toContain("BEGIN");
  });
  it("is invalid with no issues when the server gave only a reason", () => {
    expect(fromProblem(problem(422, "unprocessable", { reason: "not_allowed" }))).toMatchObject({ kind: "invalid", reason: "not_allowed", issues: [] });
  });
  it("is too large for 413", () => {
    expect(fromProblem(problem(413, "payload_too_large"))).toEqual({ kind: "too_large", requestId: "req-1" });
  });
  it("is a failure otherwise, with the title and the request id, and never the server's detail", () => {
    const out = fromProblem(problem(500, "internal"));
    expect(out).toEqual({ kind: "failed", status: 500, title: "title of internal", requestId: "req-1" });
    expect(JSON.stringify(out)).not.toContain("SELECT");
  });
  it("is a failure for a request that never arrived", () => {
    const out = fromProblem(new ApiProblem({ status: 0, kind: "network", title: "Network error" }));
    expect(out).toMatchObject({ kind: "failed", status: 0 });
  });
});

describe("issues", () => {
  it("has a canned sentence for each kind, with the line and no text of the line", () => {
    expect(issueMessage({ kind: "secret_match", line: 120, rule: "private_key" })).toBe("Line 120 looks like a secret (private_key). Save is blocked.");
    expect(issueMessage({ kind: "parse_error", line: 88 })).toBe("Line 88 could not be read as this kind of file.");
    expect(issueMessage({ kind: "duplicate_key", line: 90 })).toBe("Line 90 repeats a key that is set earlier in the file.");
    expect(issueMessage({ kind: "redaction_marker", line: 12 })).toBe("Line 12 holds a redaction marker. A masked value cannot be saved: remove the marker.");
    expect(issueMessage({ kind: "parse_error" })).toBe("The file could not be read as this kind of file.");
  });
  it("tells an error from a warning: only a duplicate key lets the write through", () => {
    expect(issueLine({ kind: "duplicate_key", line: 4 }).severity).toBe("warning");
    expect(issueLine({ kind: "secret_match", line: 4 }).severity).toBe("error");
    expect(issueLine({ kind: "parse_error", line: 4 }).severity).toBe("error");
    expect(issueLine({ kind: "redaction_marker", line: 4 }).severity).toBe("error");
  });
});

describe("errorInfo", () => {
  it("says an invalid content was refused, with each issue in the app's own words", () => {
    const info = errorInfo({ kind: "invalid", reason: "secret_detected", issues: [{ kind: "secret_match", line: 120, rule: "private_key" }], requestId: "req-1" });
    expect(info.title).toBe("Not saved: the content was refused");
    expect(info.detail).toBe("Line 120 looks like a secret (private_key). Save is blocked.");
    expect(info.requestId).toBe("req-1");
  });
  it("says what the refusal was when the server gave no lines", () => {
    expect(errorInfo({ kind: "invalid", reason: "not_allowed", issues: [], requestId: undefined }).detail).toBe("The server does not allow this change.");
    expect(errorInfo({ kind: "invalid", reason: undefined, issues: [], requestId: undefined }).detail).toBe("The server refused the content.");
  });
  it("says a file that is too large was not written", () => {
    expect(errorInfo({ kind: "too_large", requestId: "r" })).toEqual({ title: "The file is too large", detail: "The limit is 2 MiB. Nothing was written.", requestId: "r" });
  });
  it("says a failed request wrote nothing, and what to do", () => {
    expect(errorInfo({ kind: "failed", status: 500, title: "Internal error", requestId: "r" })).toEqual({ title: "Internal error", detail: "Nothing was written. Try again, or send the request ID to the platform team.", requestId: "r" });
    expect(errorInfo({ kind: "failed", status: 0, title: "Network error", requestId: undefined }).detail).toBe("The server could not be reached, so it is not known whether anything was written. Check the connection, reload the file and try again.");
    expect(errorInfo({ kind: "failed", status: 403, title: "Forbidden", requestId: "r" }).detail).toBe("Nothing was written. Your role does not allow this.");
  });
});
