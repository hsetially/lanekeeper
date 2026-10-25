import type { ServiceRef } from "@/api/generated/model";

// What the hub answered to ONE restart request of a "restart all", for the list that shows them (StepList). "accepted" is not
// "restarted" (the contract has no restarting state), and a service whose request was never sent is "not attempted", never "failed".

export type StepState = "waiting" | "sending" | "accepted" | "sent-for-approval" | "failed" | "not-attempted";

export type Step = { readonly service: ServiceRef; readonly state: StepState; /** The approval request, for a step that was sent for approval. */ readonly request?: number };
