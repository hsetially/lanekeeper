import { listDocs, renderDoc } from "@/api/generated/endpoints/docs/docs";
import type { ListDocsParams } from "@/api/generated/model";
import { EVENT_FED } from "@/lib/query/freshness";
import { docKeys } from "@/lib/query/keys";
import { apiQuery } from "@/lib/query/options";
import { pagedQuery } from "@/lib/query/paged";

// The documents. doc_indexed names the list and the doc (its id), so both are EVENT_FED.

export const docListQuery = (params: Omit<ListDocsParams, "cursor"> = {}) =>
  pagedQuery({
    queryKey: docKeys.list(params),
    fetch: (cursor, signal) => listDocs({ ...params, ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    staleTime: EVENT_FED,
  });

/**
 * The reader is addressed by path (`renderDoc` takes a path, not an id), and the live events and the writes name a doc by its
 * id (`docKeys.detail(id)`). So the reader first asks which doc has this path, and keys the page by the id it finds: an event
 * that says "doc 102 was indexed" and a replace of doc 102 then reach the page that shows it. The ask is a list by path prefix,
 * which is also what gives the reader the doc's version, source and size for Replace and Delete.
 */
export const docByPathQuery = (path: string) => docListQuery({ path_prefix: path, limit: 20 });

/** The server-rendered page of one doc. `id` is the doc's id from `docByPathQuery`; the page is revalidated by ETag. */
export const renderedDocQuery = (id: number, path: string) =>
  apiQuery({
    queryKey: docKeys.detail(id),
    queryFn: ({ signal }) => renderDoc({ path }, { signal }),
    staleTime: EVENT_FED,
  });
