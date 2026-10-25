import { getFeatureStatus } from "@/api/generated/endpoints/docs/docs";
import type { GetFeatureStatusParams } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { docKeys } from "@/lib/query/keys";
import { apiQuery } from "@/lib/query/options";

/** Which documented features a file or swimlane uses. doc_indexed names it (the documents it reads), so it is EVENT_FED. */
export const featureStatusQuery = (params: Omit<GetFeatureStatusParams, "cursor"> = {}) =>
  apiQuery({
    queryKey: docKeys.features(params),
    queryFn: ({ signal }) => getFeatureStatus(params, { signal }),
    staleTime: EVENT_FED,
  });
