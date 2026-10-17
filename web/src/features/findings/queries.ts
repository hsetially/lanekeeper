import { listFindings } from "@/api/generated/endpoints/findings/findings";
import type { ListFindingsParams } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { findingKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";

/** The findings (C1 to C12) for the filters. findings_changed refetches every filter at once; a severity rule write does too. */
export const findingListQuery = (params: Omit<ListFindingsParams, "cursor"> = {}) =>
  pagedQuery({
    queryKey: findingKeys.list(params),
    fetch: (cursor, signal) => listFindings({ ...params, ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });
