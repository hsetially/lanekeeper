import type { QueryClient } from "@tanstack/react-query";
import { notifyServices, restartService } from "@/api/generated/endpoints/writes/writes";
import type { NotifyRequest, NotifyResult, ServiceRef, WriteProposalCreated, WriteResult } from "@/api/generated/model";
import { outcomeOf, writeMutation } from "@/lib/query/write";

// Writes on a swimlane's services. Each can be sent for approval (a 202, `outcome: "proposal_created"`) and can be locked
// (423), so neither is ever optimistic: the screen shows what the server said, and after a 202 only the approval queue is
// refreshed, because nothing was restarted.

export type RestartVariables = { swimlane: string; service: ServiceRef };
export type NotifyVariables = { swimlane: string; body: NotifyRequest };

export const swimlaneMutations = {
  restart: (client: QueryClient) =>
    writeMutation<"restartService", RestartVariables, WriteResult>(client, {
      operation: "restartService",
      run: ({ swimlane, service }) => restartService(swimlane, service),
      context: ({ swimlane }, result) => ({ swimlane, ...outcomeOf(result) }),
    }),
  notify: (client: QueryClient) =>
    writeMutation<"notifyServices", NotifyVariables, NotifyResult | WriteProposalCreated>(client, {
      operation: "notifyServices",
      run: ({ swimlane, body }) => notifyServices(swimlane, body),
      context: ({ swimlane }, result) => ({ swimlane, ...outcomeOf(result) }),
    }),
};
