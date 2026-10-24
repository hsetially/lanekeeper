import { ApiProblem } from "@/api/problem";
import type { WriteErrorInfo } from "@/components/app/WriteError";

// What a failed reveal says inside the dialog that asked for it (never a toast, never the request). Only the problem's title and its
// request id are the server's words (S21); the rest is this screen's own, chosen by the status.
//
// The hub audits a reveal BEFORE the value leaves, and a second request is a second audited reveal (docs/threat-model.md: "A reveal is
// not replayable, so a response that is lost is audited twice"). So the words differ: a refusal shows nothing and records nothing; an
// answer that never arrived may have recorded a reveal that nobody saw, and trying again records another. The screen says so and
// never retries by itself.

export const REVEALS_PER_MINUTE = 30;

const wait = (seconds: number | undefined): string => (seconds === undefined ? "a moment" : seconds === 1 ? "1 second" : `${seconds} seconds`);

const LOST = "Nothing was shown, but the hub may have recorded a reveal that you did not see. Trying again makes another record.";

function detailFor(error: ApiProblem): string | undefined {
  switch (error.status) {
    case 0:
      return LOST;
    case 400:
      return "The hub did not accept this request. Reload the file and try again.";
    case 403:
      return "Your role does not allow revealing values. It may have changed since this page loaded: reload the page.";
    case 404:
      return "There is nothing to reveal there: the file or the setting may have changed, or its value is no longer hidden. Reload the file. Nothing was shown.";
    case 429:
      return `Reveals are limited to ${REVEALS_PER_MINUTE} a minute. Wait ${wait(error.retryAfterSeconds)}, then try again. Nothing was shown.`;
    case 503:
      return `The audit log could not take the record, so nothing was shown. Try again in ${wait(error.retryAfterSeconds)}.`;
    default:
      return error.status >= 500 ? LOST : undefined;
  }
}

export function revealError(error: unknown): WriteErrorInfo {
  if (!(error instanceof ApiProblem)) return { title: "The value could not be revealed", detail: "Nothing was shown." };
  const detail = detailFor(error);
  return { title: error.title, ...(detail === undefined ? {} : { detail }), ...(error.requestId === undefined ? {} : { requestId: error.requestId }) };
}
