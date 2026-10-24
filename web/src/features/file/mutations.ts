import type { QueryClient } from "@tanstack/react-query";
import { applyDraft, createDraft, deleteFile, editFile, previewFileEffect, revertFile, uploadFile } from "@/api/generated/endpoints/writes/writes";
import type { ChangePreview, DeleteFileParams, Draft, DraftRequest, EditFileRequest, PreviewFileEffectBody, RevertFileRequest, UploadFileBody, WriteResult } from "@/api/generated/model";
import { outcomeOf, writeMutation } from "@/lib/query/write";

// File writes. Every one carries the hash the caller believes is there (a 409 says someone else changed it, and nothing was
// written), may be locked (423), and may be sent for approval (202). So none is optimistic: the screen shows the server's
// answer, and what to refetch afterwards depends on whether the write was applied or only queued (src/lib/query/invalidate.ts).
// `Idempotency-Key`: the fetch layer makes one per call and reuses it only for its own retry after a network error.

export type EditVariables = { swimlane: string; body: EditFileRequest };
export type UploadVariables = { swimlane: string; body: UploadFileBody };
export type RevertVariables = { swimlane: string; body: RevertFileRequest };
export type DeleteVariables = { swimlane: string; params: DeleteFileParams };
export type PreviewVariables = { swimlane: string; body: PreviewFileEffectBody };
export type ApplyDraftVariables = { draft: string; swimlane: string; path: string };

export const fileMutations = {
  edit: (client: QueryClient) =>
    writeMutation<"editFile", EditVariables, WriteResult>(client, {
      operation: "editFile",
      run: ({ swimlane, body }) => editFile(swimlane, body),
      context: ({ swimlane, body }, result) => ({ swimlane, path: body.path, ...outcomeOf(result) }),
    }),
  upload: (client: QueryClient) =>
    writeMutation<"uploadFile", UploadVariables, WriteResult>(client, {
      operation: "uploadFile",
      run: ({ swimlane, body }) => uploadFile(swimlane, body),
      context: ({ swimlane, body }, result) => ({ swimlane, path: body.path, ...outcomeOf(result) }),
    }),
  revert: (client: QueryClient) =>
    writeMutation<"revertFile", RevertVariables, WriteResult>(client, {
      operation: "revertFile",
      run: ({ swimlane, body }) => revertFile(swimlane, body),
      context: ({ swimlane, body }, result) => ({ swimlane, path: body.path, ...outcomeOf(result) }),
    }),
  delete: (client: QueryClient) =>
    writeMutation<"deleteFile", DeleteVariables, WriteResult>(client, {
      operation: "deleteFile",
      run: ({ swimlane, params }) => deleteFile(swimlane, params),
      context: ({ swimlane, params }, result) => ({ swimlane, path: params.path, ...outcomeOf(result) }),
    }),
  /** What a change would affect, and the `preview_id` a delete needs. Writes nothing. */
  preview: (client: QueryClient) =>
    writeMutation<"previewFileEffect", PreviewVariables, ChangePreview>(client, {
      operation: "previewFileEffect",
      run: ({ swimlane, body }) => previewFileEffect(swimlane, body),
      context: () => ({}),
    }),
  /** Save the text as a draft for approval instead of writing it. */
  createDraft: (client: QueryClient) => writeMutation<"createDraft", DraftRequest, Draft>(client, { operation: "createDraft", run: (body) => createDraft(body), context: () => ({}) }),
  /** Apply a saved draft: a file write, with the draft's swimlane and path. */
  applyDraft: (client: QueryClient) =>
    writeMutation<"applyDraft", ApplyDraftVariables, WriteResult>(client, {
      operation: "applyDraft",
      run: ({ draft }) => applyDraft(draft),
      context: ({ swimlane, path }, result) => ({ swimlane, path, ...outcomeOf(result) }),
    }),
};
