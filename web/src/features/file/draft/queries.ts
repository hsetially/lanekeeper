import { getDraft } from "@/api/generated/endpoints/writes/writes";
import { fileKeys } from "@/lib/query/keys";
import { apiQuery } from "@/lib/query/options";

// One draft, as it is now (CG17). The answer is `no-store`: its severity and its effects are the engine's as of the call, and no
// live event names a draft, so it is read again whenever the page opens and is never counted as fresh. The key is
// `fileKeys.draft`, rooted at "draft".

export const draftQuery = (id: string) =>
  apiQuery({
    queryKey: fileKeys.draft(id),
    queryFn: ({ signal }) => getDraft(id, { signal }),
    staleTime: 0,
  });
