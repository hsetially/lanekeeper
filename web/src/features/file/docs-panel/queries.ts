import { docsForFile } from "@/api/generated/endpoints/docs/docs";
import { featureStatusQuery } from "@/features/status/queries";
import { EVENT_FED } from "@/lib/query/freshness";
import { docKeys } from "@/lib/query/keys";
import { apiQuery } from "@/lib/query/options";

// What the docs panel reads. Two reads, each its own so that one failing does not hide the other:
//  - the docs that describe the file (`docsForFile`), by the LOGICAL file (the name without the tenant), because a doc is about
//    `tx-infinity-core.yml` and not about `tx-infinity-core-sit7.yml`;
//  - the documented feature flags this file sets, with this swimlane's value (the status screen's own query, so the two share
//    one cache entry for the same file and swimlane).
//
// The key is `docKeys.forFile`, rooted at "docs", which is what the `doc_indexed` event invalidates (`docKeys.lists()`), so a doc
// that is indexed again is read again.

/** How many docs the panel asks for. The panel and the "has docs" check ask for the same, so they share one cache entry. */
const DOCS_LIMIT = 10;

export const docsForFileQuery = (file: string) =>
  apiQuery({
    queryKey: docKeys.forFile(file, null),
    queryFn: ({ signal }) => docsForFile({ file, limit: DOCS_LIMIT }, { signal }),
    staleTime: EVENT_FED,
  });

/** The documented flags of `file` and the value each has in swimlane `lane`. */
export const fileFlagsQuery = (lane: string, file: string) => featureStatusQuery({ file, swimlanes: [lane] });
