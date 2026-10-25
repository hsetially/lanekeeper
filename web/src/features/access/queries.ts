import { getGithubCredential, getMe, listMcpTokens } from "@/api/generated/endpoints/me/me";
import { POLLED_MS } from "@/lib/query/freshness";
import { meKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";

// The signed-in user and what belongs to them alone. Nothing here is covered by a live event (an admin changing a role does
// not reach the user's stream), so all of it is POLLED: fresh for a minute, then a screen says so and offers Refresh, and
// mounting it refetches. The writes that change it invalidate it themselves (src/lib/query/invalidate.ts).

/** Who the user is and their role. `enabled` is the caller's: a screen before sign-in must not ask. */
export const meQuery = () =>
  apiQuery({
    queryKey: meKeys.all(),
    queryFn: ({ signal }) => getMe({ signal }),
    staleTime: POLLED_MS,
  });

/** The metadata of the user's GitHub token (never the token): when it was stored, whether it works. */
export const githubCredentialQuery = () =>
  apiQuery({
    queryKey: meKeys.githubCredential(),
    queryFn: ({ signal }) => getGithubCredential({ signal }),
    staleTime: POLLED_MS,
  });

/** The user's personal MCP tokens: labels and dates, never a token. A 404 means the mode is inactive. */
export const mcpTokensQuery = () =>
  pagedQuery({
    queryKey: meKeys.mcpTokens(),
    fetch: (cursor, signal) => listMcpTokens(cursor === undefined ? {} : { cursor }, { signal }),
    staleTime: POLLED_MS,
  });
