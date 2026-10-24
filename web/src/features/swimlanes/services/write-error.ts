import type { BaselineStatus } from "@/api/generated/model";
import type { ApiProblem } from "@/api/problem";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { lockedExplanation } from "../baseline";

// What a refused restart says inside its dialog: what happened, why, and what to do next, in words the screen chose. The
// server's own title is shown only where it is the best name of what happened; what it wrote in `detail` never is (S21).
// A write that got no answer is not reported as "not sent": nobody knows.

type Context = { swimlane: string; baseline: BaselineStatus | null | undefined };

const withId = (info: Omit<WriteErrorInfo, "requestId">, problem: ApiProblem): WriteErrorInfo => (problem.requestId === undefined ? info : { ...info, requestId: problem.requestId });

export function restartFailure(problem: ApiProblem, { swimlane, baseline }: Context): WriteErrorInfo {
  if (problem.status === 423) {
    const until = problem.body["until"];
    return withId(lockedExplanation(swimlane, baseline, typeof until === "number" && Number.isFinite(until) ? until : null), problem);
  }
  switch (problem.status) {
    case 0:
      return withId({ title: "Couldn't reach the server", detail: "The restart may or may not have been sent. Check the service before you try again." }, problem);
    case 403:
      return withId({ title: "You may not restart services here", detail: "Restarting needs the Operator role. Nothing was restarted." }, problem);
    case 404:
      return withId({ title: `${swimlane} was not found`, detail: "It may have been removed. Nothing was restarted." }, problem);
    case 422:
      return withId({ title: problem.title, detail: "The server did not accept this restart. Nothing was restarted." }, problem);
    case 429:
      return withId(
        {
          title: "Too many requests",
          detail: problem.retryAfterSeconds === undefined ? "Wait a moment, then try again." : `Wait ${problem.retryAfterSeconds} seconds, then try again.`,
        },
        problem,
      );
    default:
      if (problem.status >= 500) return withId({ title: problem.title, detail: "The server could not finish the restart. Check the service before you try again." }, problem);
      return withId({ title: problem.title, detail: "The restart was not confirmed." }, problem);
  }
}
