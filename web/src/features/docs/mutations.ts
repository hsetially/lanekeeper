import type { QueryClient } from "@tanstack/react-query";
import { createDoc, deleteDoc, updateDoc } from "@/api/generated/endpoints/docs/docs";
import type { DocCreateRequest, DocSummary, DocUpdateRequest } from "@/api/generated/model";
import { writeMutation } from "@/lib/query/write";

// Documents. An update carries the version it was made against (`expected_version`, a 409 says someone else saved first), so
// none of these is optimistic.

export type UpdateDocVariables = { id: number; body: DocUpdateRequest };

export const docMutations = {
  create: (client: QueryClient) => writeMutation<"createDoc", DocCreateRequest, DocSummary>(client, { operation: "createDoc", run: (body) => createDoc(body), context: () => ({}) }),
  update: (client: QueryClient) =>
    writeMutation<"updateDoc", UpdateDocVariables, DocSummary>(client, { operation: "updateDoc", run: ({ id, body }) => updateDoc(id, body), context: ({ id }) => ({ id }) }),
  delete: (client: QueryClient) => writeMutation<"deleteDoc", number, unknown>(client, { operation: "deleteDoc", run: (id) => deleteDoc(id), context: (id) => ({ id }) }),
};
