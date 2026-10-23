import { revealValue } from "@/api/generated/endpoints/swimlanes/swimlanes";
import type { RevealRequest } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";

// The request of `revealValue`, for `useSecretMutation` (src/lib/query/secret-mutation.ts). It is not a mutation option: the answer is
// a secret, and a mutation option would put it in the MutationCache. It is audited server-side, 30 a minute, Editor and above, and
// the fetch layer never retries it and never keeps its answer (src/api/client.ts: NEVER_AUTO_RETRY_PATHS, NEVER_CACHE_PATHS), because
// a retry is a second audited reveal. Each call gets its own Idempotency-Key from the fetch layer.
//
// The answer goes to the caller as a bare string. Nothing here logs it, keys by it or holds it.

/** Longer than this and the hub is not going to answer: the screen says that the reveal may have been recorded. */
export const REVEAL_TIMEOUT_MS = 15_000;

export type RevealVariables = { swimlane: string; body: RevealRequest; signal: AbortSignal };

export async function revealValueRequest({ swimlane, body, signal }: RevealVariables): Promise<string> {
  const answer = await revealValue(swimlane, body, { signal, timeoutMs: REVEAL_TIMEOUT_MS });
  const value: unknown = answer.value;
  if (typeof value !== "string") throw new ApiProblem({ status: 200, kind: "bad_response", title: "The server's answer could not be read" });
  return value;
}
