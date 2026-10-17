import type { QueryClient } from "@tanstack/react-query";
import { approveProposal, rejectProposal } from "@/api/generated/endpoints/writes/writes";
import type { ApproveRequest, Proposal, RejectRequest, WriteResult } from "@/api/generated/model";
import { outcomeOf, writeMutation } from "@/lib/query/write";

// Deciding a proposal. Approving applies the change, which can still be refused (409 the file moved, 423 locked) or sent on
// for another approval, so it is never optimistic; the queue and the proposal are refetched, and the file the proposal
// touched when the caller says which (the proposal it is deciding knows).

export type ApproveVariables = { id: number; body?: ApproveRequest; swimlane?: string; path?: string };
export type RejectVariables = { id: number; body: RejectRequest };

export const approvalMutations = {
  approve: (client: QueryClient) =>
    writeMutation<"approveProposal", ApproveVariables, WriteResult>(client, {
      operation: "approveProposal",
      run: ({ id, body }) => approveProposal(id, body),
      context: ({ id, swimlane, path }, result) => ({ id, ...(swimlane === undefined ? {} : { swimlane }), ...(path === undefined ? {} : { path }), ...outcomeOf(result) }),
    }),
  reject: (client: QueryClient) =>
    writeMutation<"rejectProposal", RejectVariables, Proposal>(client, {
      operation: "rejectProposal",
      run: ({ id, body }) => rejectProposal(id, body),
      context: ({ id }) => ({ id }),
    }),
};
