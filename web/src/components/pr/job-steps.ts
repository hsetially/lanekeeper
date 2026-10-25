import type { PrJobState } from "@/api/generated/model";

// What a pull-request job shows of its progress: the state the server reports, as four steps, and nothing else. The job carries
// no step times and no step text (`PrJob` has `state`, `warnings`, `error` and two times), so none is drawn and none is made up.
// A step is done because the state is past it. "Done" is done only for a completed job, so the view cannot say a pull request
// exists before the server did. A queued job has begun nothing, and a failed one does not say how far it got.
//
// Written as functions over the state, not as a table typed with the API's enum: a state the contract adds is a compile error in
// the `switch`, not a blank row on screen.

export type JobPhase = "queued" | "running" | "completed" | "failed";
export type StepStatus = "done" | "active" | "waiting";
export type JobStep = { readonly label: string; readonly status: StepStatus };

export function phaseOf(state: PrJobState): JobPhase {
  switch (state) {
    case "pending":
      return "queued";
    case "preparing":
    case "committing":
    case "opening":
      return "running";
    case "completed":
      return "completed";
    case "failed":
      return "failed";
  }
}

/** The step the state is at (0 to 3), 4 when all are done, -1 when none has begun or the state says nothing about progress. */
function position(state: PrJobState): number {
  switch (state) {
    case "pending":
    case "failed":
      return -1;
    case "preparing":
      return 0;
    case "committing":
      return 1;
    case "opening":
      return 2;
    case "completed":
      return 4;
  }
}

const LABELS = ["Preparing", "Committing", "Opening pull request", "Done"] as const;

export function jobSteps(state: PrJobState): JobStep[] {
  if (state === "failed") return [];
  const at = position(state);
  return LABELS.map((label, i): JobStep => ({ label, status: i < at ? "done" : i === at ? "active" : "waiting" }));
}

/** A job that will not change again: the server is done with it. */
export function isTerminal(state: PrJobState): boolean {
  const phase = phaseOf(state);
  return phase === "completed" || phase === "failed";
}
