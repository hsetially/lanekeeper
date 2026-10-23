import type { BaselineStatus } from "@/api/generated/model";
import type { ApiProblem } from "@/api/problem";
import { isRetryableProblem } from "@/lib/query/use-query-state";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { baselineView } from "@/features/swimlanes/baseline";
import { formatTime } from "@/lib/time";

// What a refused notification says inside its dialog: what happened, why, and what to do next, in words the screen chose. The
// server's own title is shown only where it is the best name of what happened; what it wrote in `detail` never is (S21). A write
// that got no answer, or a server error, is not reported as "not sent": nobody knows. A refusal (400, 403, 404, 409, 422, 423) is
// reported as "nothing was sent", because the server refused before it did anything.

type Context = { swimlane: string; baseline: BaselineStatus | null | undefined };

const withId = (info: Omit<WriteErrorInfo, "requestId">, problem: ApiProblem): WriteErrorInfo => (problem.requestId === undefined ? info : { ...info, requestId: problem.requestId });

/** The cause of a 423, as far as the screen can know it: the contract has no cause on it (CG4b), so it is the baseline or "something else". */
function locked(swimlane: string, baseline: BaselineStatus | null | undefined, until: number | null): { title: string; detail: string } {
  const view = baselineView(baseline);
  if (view.kind === "unconfirmed") {
    return { title: `${swimlane} is locked until it is adopted`, detail: "Its baseline is unconfirmed, so writes to it are blocked. An admin needs to adopt it first. Nothing was sent." };
  }
  if (view.kind === "in_progress") {
    return { title: `${swimlane} has an adoption in progress`, detail: "Writes to it are blocked until an admin confirms the baseline. Nothing was sent." };
  }
  const ends = until === null ? "" : ` Locked until ${formatTime(until)}.`;
  return { title: `${swimlane} is locked`, detail: `Another operation is using it. Nothing was sent. Try again in a moment.${ends}` };
}

export function notifyFailure(problem: ApiProblem, { swimlane, baseline }: Context): WriteErrorInfo {
  if (problem.status === 423) {
    const until = problem.body["until"];
    return withId(locked(swimlane, baseline, typeof until === "number" && Number.isFinite(until) ? until : null), problem);
  }
  switch (problem.status) {
    case 0:
      return withId({ title: "Couldn't reach the server", detail: "The notification may or may not have been sent. Check the services before you try again." }, problem);
    case 403:
      return withId({ title: "You may not notify services here", detail: "Notifying services needs the Operator role. Nothing was sent." }, problem);
    case 404:
      return withId({ title: `${swimlane} was not found`, detail: "It may have been removed. Nothing was sent." }, problem);
    case 409:
      return withId({ title: problem.title, detail: "Something changed before the notification was handled. Nothing was sent. Reload the swimlane, then try again." }, problem);
    case 400:
    case 422:
      return withId({ title: problem.title, detail: "The server did not accept this notification. Nothing was sent." }, problem);
    case 429:
      return withId(
        {
          title: "Too many requests",
          detail: problem.retryAfterSeconds === undefined ? "Wait a moment, then try again." : `Wait ${problem.retryAfterSeconds} seconds, then try again.`,
        },
        problem,
      );
    default:
      if (problem.status >= 500) return withId({ title: problem.title, detail: "The server could not finish the notification. It may or may not have been sent. Check the services before you try again." }, problem);
      return withId({ title: problem.title, detail: "The notification was not confirmed." }, problem);
  }
}

/**
 * What a restart offer that could not be read says inside its dialog. A read can be tried again (it changes nothing), so a problem
 * that a second try can change says to retry; a refusal says to send the request ID. What the server wrote in `detail` is never shown.
 */
export function offerReadFailure(problem: ApiProblem, what: string): WriteErrorInfo {
  const title = `Couldn't read ${what}`;
  if (problem.status === 403) return withId(offerNeedsOperator(what), problem);
  if (problem.status === 0) return withId({ title, detail: "The server could not be reached. Check the connection and retry." }, problem);
  if (isRetryableProblem(problem) || problem.status >= 500) return withId({ title, detail: "The server returned an error. Retry, or send the request ID to the platform team." }, problem);
  return withId({ title, detail: "The request was refused. Send the request ID to the platform team." }, problem);
}

/** Reading an offer is an Operator's: a person below it is told so without a request being made. */
export function offerNeedsOperator(what: string): WriteErrorInfo {
  return { title: `Couldn't read ${what}`, detail: "Reading it needs the Operator role." };
}
