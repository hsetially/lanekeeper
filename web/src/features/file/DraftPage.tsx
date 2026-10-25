import { useQuery } from "@tanstack/react-query";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { useQueryState } from "@/lib/query/use-query-state";
import type { PageProps } from "@/routes/props";
import { AppLink } from "@/routes/AppLink";
import { Search } from "@/icons";
import { DraftReview } from "./draft/DraftReview";
import { draftQuery } from "./draft/queries";

// The draft a tool saved for the person to read and apply (CG17: MCP draft mode links here). The server shows a draft to its
// author only, so a draft that is someone else's is a 404, the same as one that does not exist: the page says so in those words.

export default function DraftPage({ params }: PageProps<"draft">) {
  const query = useQuery(draftQuery(params.id));
  const state = useQueryState(query, { isEmpty: () => false });
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex flex-none flex-col gap-0.5 px-6 pt-3.5 pb-3">
        <h1 className="m-0 text-xl leading-[26px] font-semibold">Review draft</h1>
        <p className="m-0 text-sm text-muted-foreground">Read what this draft changes before you apply it. Nothing is written until you do.</p>
      </header>
      {state.state === "error" && state.error?.status === 404 ? (
        <EmptyState
          icon={Search}
          title="Draft not found"
          action={
            <AppLink route="swimlanes" className="text-sm underline">
              Go to the swimlanes
            </AppLink>
          }
        >
          There is no draft with this number that you can open. A draft can be opened only by the person who made it, and drafts expire.
        </EmptyState>
      ) : (
        <QueryBoundaryView state={state} noun="draft" staleBanner={false} className="flex min-h-0 flex-1 flex-col">
          {(draft) => <DraftReview draft={draft} onRefresh={() => state.refetch()} />}
        </QueryBoundaryView>
      )}
    </div>
  );
}
