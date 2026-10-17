import type { QueryClient } from "@tanstack/react-query";
import { createAccessRequest, createMcpToken, deleteGithubToken, putGithubToken, revokeMcpToken } from "@/api/generated/endpoints/me/me";
import type { AccessRequest, AccessRequestCreate, GithubTokenRequest, McpTokenCreate } from "@/api/generated/model";
import { invalidateAfterWrite } from "@/lib/query/invalidate";
import { writeMutation } from "@/lib/query/write";

// The writes of "me". The two that carry a secret (the GitHub token to store, the MCP token that is returned once) are NOT
// mutation options: a secret must not sit in the MutationCache, so they are plain requests for `useSecretMutation`
// (src/lib/query/secret-mutation.ts: gcTime 0, reset in a `finally`, never retried), and `afterSecretWrite` says what to
// refresh once one has finished. Nothing here is optimistic: whether a token is stored or revoked is the server's to say.

export const accessMutations = {
  /** Ask an admin for a role. */
  requestRole: (client: QueryClient) =>
    writeMutation<"createAccessRequest", AccessRequestCreate, AccessRequest>(client, { operation: "createAccessRequest", run: (body) => createAccessRequest(body), context: () => ({}) }),
  /** Forget the stored GitHub token. It takes no variables: call `mutate(undefined)`. */
  deleteGithubToken: (client: QueryClient) => writeMutation<"deleteGithubToken", undefined, unknown>(client, { operation: "deleteGithubToken", run: () => deleteGithubToken(), context: () => ({}) }),
  /** Revoke a personal MCP token by its id. */
  revokeMcpToken: (client: QueryClient) => writeMutation<"revokeMcpToken", number, unknown>(client, { operation: "revokeMcpToken", run: (id) => revokeMcpToken(id), context: () => ({}) }),
};

/** Store a GitHub token. For `useSecretMutation`. The token is never kept anywhere but the caller's state. */
export const putGithubTokenRequest = (body: GithubTokenRequest) => putGithubToken(body);
/** Make a personal MCP token. The answer holds the token once; it goes to the caller and nowhere else. */
export const createMcpTokenRequest = (body: McpTokenCreate) => createMcpToken(body);

/** What to refresh after `putGithubTokenRequest` or `createMcpTokenRequest` succeeded. Fixed keys only: nothing of the secret. */
export const afterSecretWrite = (client: QueryClient, operation: "putGithubToken" | "createMcpToken"): Promise<void> => invalidateAfterWrite(client, operation, {});
