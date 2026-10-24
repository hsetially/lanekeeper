import { ApiProblem } from "@/api/problem";
import type { WriteErrorInfo } from "@/components/app/WriteError";

// What a failed admin write says inside the dialog that made it: the problem's title (S21: that is all of the server's words that
// are shown), one sentence of this screen's own for the status, and the request id. The admin PUTs and DELETEs carry no expected
// version in the contract, so there is no "someone changed it first" answer to translate except the 409 of the user and request
// decisions, which each screen words itself through `byStatus`.

const COMMON: Readonly<Partial<Record<number, string>>> = {
  0: "The hub could not be reached. The change may not have been made: reload the list to check before you try again.",
  403: "The hub refused this change. Your role may have changed since this page loaded; reload the page and try again.",
  404: "It no longer exists. Reload the list to see what is there now.",
  422: "The hub did not accept these values. Check them and try again.",
  423: "This is locked right now. Try again in a moment.",
  429: "Too many changes in a short time. Wait a moment and try again.",
};
const SERVER = "The hub could not complete this change. Nothing was shown as done; reload the list to check before you try again.";

export function adminWriteError(error: unknown, fallback: string, byStatus: Readonly<Partial<Record<number, string>>> = {}): WriteErrorInfo {
  if (!(error instanceof ApiProblem)) return { title: fallback };
  const detail = byStatus[error.status] ?? COMMON[error.status] ?? (error.status >= 500 ? SERVER : undefined);
  return { title: error.title, ...(detail === undefined ? {} : { detail }), ...(error.requestId === undefined ? {} : { requestId: error.requestId }) };
}
