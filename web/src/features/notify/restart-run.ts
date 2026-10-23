import type { ServiceRef } from "@/api/generated/model";
import type { Step, StepState } from "@/components/notify/steps";

// "Restart all" sends one `restartService` request per service of the hub's offer, one at a time (the hub does it that way: a
// restart is audited and locked per service). A run is a list of steps (components/notify/steps.ts). A step is what the hub
// answered to ONE request and nothing more: "accepted" is not "restarted" (the contract has no restarting state), and a service
// whose request was never sent is "not attempted", never "failed".

export type { Step, StepState };

export const begin = (services: readonly ServiceRef[]): Step[] => services.map((service) => ({ service, state: "waiting" }));

export function mark(steps: readonly Step[], index: number, state: StepState, request?: number): Step[] {
  return steps.map((step, i) => (i === index ? { service: step.service, state, ...(request === undefined ? {} : { request }) } : step));
}

/** Every step from `from` on that has not been sent is not attempted. A step that already answered keeps its answer. */
export function abandonRest(steps: readonly Step[], from: number): Step[] {
  return steps.map((step, i) => (i >= from && step.state === "waiting" ? { service: step.service, state: "not-attempted" } : step));
}

export type RunSummary = {
  readonly total: number;
  readonly accepted: number;
  readonly sentForApproval: number;
  readonly failed: number;
  readonly notAttempted: number;
  /** A request was refused and the ones after it were not sent. */
  readonly stoppedEarly: boolean;
};

export function summarise(steps: readonly Step[]): RunSummary {
  const count = (state: StepState): number => steps.filter((step) => step.state === state).length;
  const notAttempted = count("not-attempted");
  return { total: steps.length, accepted: count("accepted"), sentForApproval: count("sent-for-approval"), failed: count("failed"), notAttempted, stoppedEarly: notAttempted > 0 };
}

const listed = new Intl.ListFormat("en", { style: "long", type: "conjunction" });

/** The sentence that says where a run stopped: which request the hub refused and what came before and after it. */
export function describeStop(summary: RunSummary, failedService: string): string {
  const { accepted, sentForApproval, notAttempted } = summary;
  const parts: string[] = [];
  if (accepted > 0) parts.push(`${accepted} earlier ${accepted === 1 ? "request was" : "requests were"} accepted`);
  if (sentForApproval > 0) parts.push(parts.length === 0 ? `${sentForApproval} earlier ${sentForApproval === 1 ? "request was" : "requests were"} sent for approval` : `${sentForApproval} ${sentForApproval === 1 ? "was" : "were"} sent for approval`);
  if (notAttempted > 0) parts.push(parts.length === 0 ? `${notAttempted} ${notAttempted === 1 ? "request was" : "requests were"} not sent` : `${notAttempted} ${notAttempted === 1 ? "was" : "were"} not sent`);
  const refusal = `The hub refused the request for ${failedService}.`;
  return parts.length === 0 ? refusal : `${refusal} ${listed.format(parts)}.`;
}
