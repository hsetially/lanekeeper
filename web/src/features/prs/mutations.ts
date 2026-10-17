import type { QueryClient } from "@tanstack/react-query";
import { createPr, previewPr } from "@/api/generated/endpoints/writes/writes";
import type { CreatePrRequest, PrJob, PrPreview, PrPreviewRequest, WriteProposalCreated } from "@/api/generated/model";
import { outcomeOf, writeMutation } from "@/lib/query/write";

// Opening a pull request. It can be sent for approval (202, a proposal) or start a job; it can answer 409 for a duplicate and
// 423 for a lock, so it is never optimistic. The preview writes nothing.

export const prMutations = {
  create: (client: QueryClient) =>
    writeMutation<"createPr", CreatePrRequest, WriteProposalCreated | PrJob>(client, {
      operation: "createPr",
      run: (body) => createPr(body),
      context: (_body, result) => outcomeOf(result),
    }),
  preview: (client: QueryClient) => writeMutation<"previewPr", PrPreviewRequest, PrPreview>(client, { operation: "previewPr", run: (body) => previewPr(body), context: () => ({}) }),
};
