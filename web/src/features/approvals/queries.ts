import { getProposal, listProposals } from "@/api/generated/endpoints/writes/writes";
import type { ListProposalsParams } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { proposalKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";

// Proposals (the approval queue). proposal_changed names the queue and the proposal, so both are EVENT_FED.

export const proposalListQuery = (params: Omit<ListProposalsParams, "cursor"> = {}) =>
  pagedQuery({
    queryKey: proposalKeys.list(params),
    fetch: (cursor, signal) => listProposals({ ...params, ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });

/** One proposal, by its id (a number from an event, a string from a route: the key is the same). */
export const proposalQuery = (id: number) =>
  apiQuery({
    queryKey: proposalKeys.detail(id),
    queryFn: ({ signal }) => getProposal(id, { signal }),
    staleTime: EVENT_FED,
  });
