import type { GithubCredential } from "@/api/generated/model";

// When a GitHub token ends, in words. The hub says `expires_soon` (within seven days, and after it has expired) so that a page can
// ask for a new token before a pull request fails (W32, CG14); the clock decides "expired", because the flag is only as fresh as
// the last fetch.

const DAY = 86_400_000;

/** `in 5 days`, `in 1 day`, `expired`. Whole days, rounded up: the last hours count as a day, so a token never reads as having more time than it has. */
export function expiryWords(expiresAt: number, now: number): string {
  if (expiresAt <= now) return "expired";
  const days = Math.ceil((expiresAt - now) / DAY);
  return days === 1 ? "in 1 day" : `in ${days} days`;
}

export type TokenHealth = "ok" | "soon" | "expired";

export function tokenHealth(credential: Pick<GithubCredential, "expires_at" | "expires_soon">, now: number): TokenHealth {
  const at = credential.expires_at;
  if (at !== null && at !== undefined && at <= now) return "expired";
  return credential.expires_soon === true ? "soon" : "ok";
}
