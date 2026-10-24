import { getPrJob, listPrs } from "@/api/generated/endpoints/writes/writes";
import type { ListPrsParams } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { prKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";
import { apiQuery } from "@/lib/query/options";

// Pull-request links and the job that opens one. pr_state_changed names the links and pr_job_progress the job, so both are
// EVENT_FED. A job's progress is only ever what the server reports: nothing here makes up a "done".

export const prListQuery = (params: Omit<ListPrsParams, "cursor"> = {}) =>
  pagedQuery({
    queryKey: prKeys.list(params),
    fetch: (cursor, signal) => listPrs({ ...params, ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });

export const prJobQuery = (id: number) =>
  apiQuery({
    queryKey: prKeys.job(id),
    queryFn: ({ signal }) => getPrJob(id, { signal }),
    staleTime: EVENT_FED,
  });
