import { listDocs } from "@/api/generated/endpoints/docs/docs";
import type { ListDocsParams } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { docKeys } from "@/lib/query/keys";
import { pagedQuery } from "@/lib/query/paged";

// The documents. doc_indexed names the list, so it is EVENT_FED. (The reader's own operations take a path, not an id, and
// have no key of the vocabulary: they are added with the reader.)

export const docListQuery = (params: Omit<ListDocsParams, "cursor"> = {}) =>
  pagedQuery({
    queryKey: docKeys.list(params),
    fetch: (cursor, signal) => listDocs({ ...params, ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });
