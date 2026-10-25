// What is retried, and when (rule 5: bounded, with jitter).
//
// A 4xx is the server's answer and the screens handle it (401 and 403 route to sign-in or the permission screen, 404 and
// 409 and 422 are states, 429 says how long to wait). Only what is probably temporary is retried: a network error, a
// timeout, and a 502, 503 or 504, twice at most. A mutation is never retried here: a retry of a write is the caller's
// decision, and the one automatic exception (a network error, same Idempotency-Key) is in src/api/client.ts.

import { ApiProblem } from "./problem";

export const MAX_QUERY_RETRIES = 2;
const BASE_MS = 500;
const CAP_MS = 8000;

function isTemporary(problem: ApiProblem): boolean {
  if (problem.status === 0) return problem.kind === "network" || problem.kind === "timeout";
  return problem.status === 502 || problem.status === 503 || problem.status === 504;
}

/** `failureCount` is how many times the query has failed so far (TanStack Query's meaning). */
export function shouldRetry(failureCount: number, error: unknown): boolean {
  if (!(error instanceof ApiProblem)) return false;
  return failureCount < MAX_QUERY_RETRIES && isTemporary(error);
}

/** Full jitter: a random delay between zero and the capped exponential, so clients that failed together do not return together. */
export function retryDelayMs(attempt: number, random: () => number = Math.random): number {
  const ceiling = Math.min(CAP_MS, BASE_MS * 2 ** Math.max(0, attempt));
  return Math.floor(random() * ceiling);
}
