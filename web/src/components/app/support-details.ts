import type { ApiProblem } from "@/api/problem";
import { PROBLEM_URN_PREFIX } from "@/api/problem";
import { formatTime } from "@/lib/time";

// What a person pastes into a message to support: the request ID, the problem's title and type, the HTTP status and the time. It
// is built from the ApiProblem alone, which holds the RESPONSE and never the request (api/problem.ts), and it leaves out `detail`
// and the problem's extension members, which are the server's words and may name a value. One line per fact: the title is the
// server's text, so a line break, a control character or a bidi mark in it becomes a space (S8, S21).

const oneLine = (text: string): string => text.replace(/[\p{Cc}\p{Cf}\u2028\u2029]/gu, " ").replace(/\s+/g, " ").trim();

export function supportDetails(problem: ApiProblem, at: number): string {
  return [
    `Request ID: ${problem.requestId === undefined ? "none (the request did not reach the hub)" : oneLine(problem.requestId)}`,
    `Problem: ${oneLine(problem.title)}`,
    `Type: ${PROBLEM_URN_PREFIX}${oneLine(problem.rawKind)}`,
    `Status: ${problem.status}`,
    `Time: ${formatTime(at)}`,
  ].join("\n");
}
