import { ApiProblem } from "@/api/problem";
import type { WriteErrorInfo } from "@/components/app/WriteError";

/**
 * What a failed write says inside the dialog that made it: the problem's title, a sentence of this screen's own for a status it
 * knows, and the request id. Never the `detail` the server wrote (S21), and never the message of an error that is not a problem.
 */
export function writeErrorInfo(error: unknown, fallback: string, byStatus: Readonly<Partial<Record<number, string>>> = {}): WriteErrorInfo {
  if (!(error instanceof ApiProblem)) return { title: fallback };
  const detail = byStatus[error.status];
  return { title: error.title, ...(detail === undefined ? {} : { detail }), ...(error.requestId === undefined ? {} : { requestId: error.requestId }) };
}
