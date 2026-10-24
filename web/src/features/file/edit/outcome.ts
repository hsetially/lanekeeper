import type { ValidationIssue, WriteResult } from "@/api/generated/model";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { ApiProblem } from "@/api/problem";

// What a write answered, in the terms the screen needs. A write is never "done" because the request returned: it was applied,
// or it was only SENT FOR APPROVAL (a 200 whose outcome is `proposal_created`; nothing has changed yet), or it was refused
// because someone else changed the file first (409, nothing was written), because the target is locked (423), because the
// content was not acceptable (422, with the lines and rule names, never the text of a line) or because it was too large (413).
//
// A problem's extension members (`current`, `until`, `reason`, `issues`) are untrusted input: each is checked before it is
// used, and `detail` is never used (S21).

export type WriteOutcome =
  | { kind: "applied"; hash: string | null; warnings: readonly ValidationIssue[] }
  | { kind: "sent"; proposal: number }
  | { kind: "conflict"; current: string | null; requestId: string | undefined }
  | { kind: "locked"; until: number | null; requestId: string | undefined }
  | { kind: "invalid"; reason: string | undefined; issues: readonly ValidationIssue[]; requestId: string | undefined }
  | { kind: "too_large"; requestId: string | undefined }
  | { kind: "failed"; status: number; title: string; requestId: string | undefined };

const HASH = /^[0-9a-f]{64}$/;
const ISSUE_KINDS = new Set<string>(["parse_error", "duplicate_key", "secret_match", "redaction_marker"]);
const RULE = /^.{1,256}$/s;
const MAX_ISSUES = 50;

export function fromResult(result: WriteResult): WriteOutcome {
  if (result.outcome === "proposal_created") return { kind: "sent", proposal: result.id };
  return { kind: "applied", hash: result.hash, warnings: result.warnings ?? [] };
}

function issuesOf(value: unknown): ValidationIssue[] {
  if (!Array.isArray(value)) return [];
  const out: ValidationIssue[] = [];
  for (const raw of value.slice(0, MAX_ISSUES) as unknown[]) {
    if (typeof raw !== "object" || raw === null) continue;
    const item = raw as Record<string, unknown>;
    const kind = item["kind"];
    if (typeof kind !== "string" || !ISSUE_KINDS.has(kind)) continue;
    const line = item["line"];
    const rule = item["rule"];
    out.push({
      kind: kind as ValidationIssue["kind"],
      ...(typeof line === "number" && Number.isInteger(line) && line >= 1 ? { line } : {}),
      ...(typeof rule === "string" && RULE.test(rule) ? { rule } : {}),
    });
  }
  return out;
}

export function fromProblem(problem: ApiProblem): WriteOutcome {
  const requestId = problem.requestId;
  const body = problem.body;
  if (problem.status === 409 && problem.kind === "conflict") {
    const current = body["current"];
    return { kind: "conflict", current: typeof current === "string" && HASH.test(current) ? current : null, requestId };
  }
  if (problem.status === 423) {
    const until = body["until"];
    return { kind: "locked", until: typeof until === "number" && Number.isFinite(until) ? until : null, requestId };
  }
  if (problem.status === 422) {
    const reason = body["reason"];
    return { kind: "invalid", reason: typeof reason === "string" ? reason : undefined, issues: issuesOf(body["issues"]), requestId };
  }
  if (problem.status === 413) return { kind: "too_large", requestId };
  return { kind: "failed", status: problem.status, title: problem.title, requestId };
}

/** Whatever a mutation threw or returned. A thrown value that is not a problem is a failure with no title of the server's. */
export function classify(settled: { result: WriteResult } | { error: unknown }): WriteOutcome {
  if ("result" in settled) return fromResult(settled.result);
  return settled.error instanceof ApiProblem ? fromProblem(settled.error) : { kind: "failed", status: 0, title: "The request failed", requestId: undefined };
}

export type IssueLine = { severity: "error" | "warning"; line: number | null; message: string };

const WHERE = (line: number | undefined): string => (line === undefined ? "The file" : `Line ${line}`);

/** One sentence for one issue. The server sends the kind, the line and the rule name; the words are the app's, so no text of the file can reach them. */
export function issueMessage(issue: ValidationIssue): string {
  switch (issue.kind) {
    case "secret_match":
      return `${WHERE(issue.line)} looks like a secret${issue.rule === undefined ? "" : ` (${issue.rule})`}. Save is blocked.`;
    case "parse_error":
      return `${WHERE(issue.line)} could not be read as this kind of file.`;
    case "duplicate_key":
      return `${WHERE(issue.line)} repeats a key that is set earlier in the file.`;
    case "redaction_marker":
      return `${WHERE(issue.line)} holds a redaction marker. A masked value cannot be saved: remove the marker.`;
  }
}

/** A duplicate key is a warning (the write goes through with it); every other kind blocks the write. */
export function issueLine(issue: ValidationIssue): IssueLine {
  return { severity: issue.kind === "duplicate_key" ? "warning" : "error", line: issue.line ?? null, message: issueMessage(issue) };
}

const REFUSAL: Readonly<Record<string, string>> = { not_allowed: "The server does not allow this change.", too_large: "The file is too large.", denied: "This file cannot be changed.", unknown_swimlane: "The swimlane is not known." };

/**
 * What a write that stays in its dialog says: the outcomes that are not a dialog of their own (the file was refused, too large,
 * or the request failed). It always says that nothing was written when that is known, and never what the server put in `detail`.
 */
export function errorInfo(outcome: Extract<WriteOutcome, { kind: "invalid" | "too_large" | "failed" }>): WriteErrorInfo {
  const requestId = outcome.requestId;
  const withId = requestId === undefined ? {} : { requestId };
  if (outcome.kind === "invalid") {
    const lines = outcome.issues.map(issueMessage);
    const detail = lines.length > 0 ? lines.join(" ") : outcome.reason === undefined ? "The server refused the content." : (REFUSAL[outcome.reason] ?? "The server refused the content.");
    return { title: "Not saved: the content was refused", detail, ...withId };
  }
  if (outcome.kind === "too_large") return { title: "The file is too large", detail: "The limit is 2 MiB. Nothing was written.", ...withId };
  if (outcome.status === 0) return { title: outcome.title, detail: "The server could not be reached, so it is not known whether anything was written. Check the connection, reload the file and try again.", ...withId };
  if (outcome.status === 403) return { title: outcome.title, detail: "Nothing was written. Your role does not allow this.", ...withId };
  return { title: outcome.title, detail: "Nothing was written. Try again, or send the request ID to the platform team.", ...withId };
}
