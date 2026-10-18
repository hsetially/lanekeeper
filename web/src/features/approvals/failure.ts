import type { ApiProblem } from "@/api/problem";
import type { WriteErrorInfo } from "@/components/app/WriteError";

// What a refused decision says. The words are this screen's: the server's own `title` is never the whole story, and what it wrote
// in `detail` or `issues` is never shown (S21). A decision that got no answer is not reported as "not recorded": nobody knows.
// The two answers that change what the person should do next, 409 (the file moved first) and 423 (locked), get a dialog of their
// own; everything else stays inside the drawer, beside the buttons, with the request ID.

export type Verb = "approve" | "reject";

const withId = (info: Omit<WriteErrorInfo, "requestId">, problem: ApiProblem): WriteErrorInfo => (problem.requestId === undefined ? info : { ...info, requestId: problem.requestId });

/** Why the server refused to write what the proposal says, in words of ours. The reasons are the contract's `RejectReason`. */
function refusal(reason: unknown): string {
  switch (reason) {
    case "denied":
      return "The write is not allowed on this path.";
    case "too_large":
      return "The file is too large to write.";
    case "invalid_content":
      return "The proposed content is not valid.";
    case "unknown_swimlane":
      return "The swimlane no longer exists.";
    case "agent_unavailable":
      return "The swimlane's agent can't be reached, so nothing could be written. Try again when it is back.";
    case "secret_detected":
      return "The proposed content holds a secret, so it can't be written.";
    case "parse_error":
      return "The proposed content does not parse.";
    default:
      return "The server did not accept this decision.";
  }
}

/** Which refusals have a dialog. */
export function dialogOf(problem: ApiProblem | null): "conflict" | "locked" | null {
  if (problem === null) return null;
  if (problem.status === 409) return "conflict";
  if (problem.status === 423) return "locked";
  return null;
}

/** The inline message for a refused decision, or null when the answer has a dialog. */
export function failureOf(problem: ApiProblem, { own }: { verb: Verb; own: boolean }): WriteErrorInfo | null {
  if (dialogOf(problem) !== null) return null;
  switch (problem.status) {
    case 0:
      return withId({ title: "Couldn't reach the server", detail: "The decision may or may not have been recorded. Open the proposal again to see where it stands." }, problem);
    case 403:
      return own
        ? withId({ title: "You can't decide your own proposal", detail: "Another Admin has to. Nothing was recorded." }, problem)
        : withId({ title: "You may not decide proposals", detail: "Deciding needs the Admin role. Nothing was recorded." }, problem);
    case 404:
      return withId({ title: "This proposal was not found", detail: "It may have been removed. Nothing was recorded." }, problem);
    case 422:
      return problem.body["reason"] === "not_allowed"
        ? withId({ title: "This proposal can't be decided now", detail: "It is no longer pending: someone else decided it, or it expired. Your decision was not recorded." }, problem)
        : withId({ title: "This proposal can't be applied", detail: `${refusal(problem.body["reason"])} Nothing was applied.` }, problem);
    case 429:
      return withId(
        { title: "Too many requests", detail: problem.retryAfterSeconds === undefined ? "Wait a moment, then try again." : `Wait ${problem.retryAfterSeconds} seconds, then try again.` },
        problem,
      );
    default:
      if (problem.status >= 500) return withId({ title: problem.title, detail: "The server could not finish the decision. Open the proposal again to see where it stands." }, problem);
      return withId({ title: problem.title, detail: "The decision was not confirmed." }, problem);
  }
}
