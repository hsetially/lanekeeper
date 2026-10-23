import { useEffect } from "react";
import type { PrLink } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { EmptyState } from "@/components/app/EmptyState";
import { ListSkeleton, QueryBoundaryView } from "@/components/app/QueryBoundary";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { GitPullRequest, RefreshCw } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { useStableList } from "@/live/stable-list";
import { useNow } from "@/live/use-now";
import { AppLink, useReplaceTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import { FILTERS, filterOf, listParams, type PrFilter } from "./filter";
import { PrTable } from "./PrTable";
import { prListQuery } from "./queries";

// The pull requests of a swimlane (route `swimlane-pull-requests`): the page that the links of the MCP tools and the "Find my pull
// request" link of a job point to. The handoff has no frame for it (H2), so it is built from the Approvals page's shape: a title,
// a state filter in the address, a table of 32 px rows, a foot that says how much is shown, live updates held still while read.
//
// Live: `pr_state_changed` names `['prs']` (src/live/invalidation.ts), and the list on screen is read again. The event has no
// swimlane (CG1), so the list of every swimlane that is on screen is read again, and this page shows the answer of its own question.
// A row that changes in place changes in place; a row that comes or goes is counted ("1 update") until the person asks to see it,
// so nothing moves under the pointer.

// The segments of the state filter, drawn as the other filters of the app are (Search, docs sources).
const SEGMENT = "h-[26px] rounded-none border-0 border-l border-input px-2.5 font-normal text-foreground first:border-l-0 data-[state=on]:border-input data-[state=on]:bg-secondary data-[state=on]:font-medium hover:bg-secondary-hover";

const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`;
const keyOfLink = (link: PrLink): string => String(link.id);

const LABEL = (filter: PrFilter): string => (filter === "open" ? "Open" : filter === "merged" ? "Merged" : filter === "closed" ? "Closed" : "All");
/** "open pull requests", "merged pull request", or for 'all' just "pull requests". */
const noun = (filter: PrFilter, n: number): string => `${filter === "all" ? "" : `${filter} `}${n === 1 ? "pull request" : "pull requests"}`;

function Body({ rows, swimlane, filter, hasMore, loadingMore, onLoadMore }: { rows: readonly PrLink[]; swimlane: string; filter: PrFilter; hasMore: boolean; loadingMore: boolean; onLoadMore: () => Promise<void> }) {
  const session = useSession();
  const me = session.status === "ready" ? session.me.id : null;
  const now = useNow(true, 30_000);
  const stable = useStableList(rows, keyOfLink, { appendsApply: true });
  // With nothing on screen there is nothing to hold still: a pull request that arrives is shown at once.
  const { apply } = stable;
  const arrived = stable.shown.length === 0 && stable.pending > 0;
  useEffect(() => {
    if (arrived) apply();
  }, [arrived, apply]);
  // The list is empty only when what is on screen is empty and nothing is waiting: the last row of a list goes away when the person
  // asks to see that it went, like any other.
  if (stable.shown.length === 0 && stable.pending === 0) {
    return (
      <EmptyState icon={GitPullRequest} title={filter === "all" ? "No pull requests yet" : `No ${filter} pull requests`}>
        A pull request raised from a file of {swimlane} appears here as it happens.
      </EmptyState>
    );
  }
  return (
    <>
      {stable.pending > 0 ? (
        <div role="status" className="flex h-8 flex-none items-center gap-2 border-b border-border bg-info-bg px-3 text-xs text-info-fg">
          <span className="tabular-nums">{plural(stable.pending, "update", "updates")}</span>
          <span>The list is held still while you read it.</span>
          <Button variant="secondary" size="sm" onClick={stable.apply} className="ml-auto">
            <RefreshCw data-icon="inline-start" />
            Show
          </Button>
        </div>
      ) : null}
      <PrTable rows={stable.shown} swimlane={swimlane} me={me} now={now} moreRows={hasMore} className="flex min-h-0 flex-1 flex-col" />
      {hasMore ? (
        <div className="flex h-9 flex-none items-center gap-3 border-t border-border px-3 text-xs text-muted-foreground">
          <span className="tabular-nums">Showing {stable.shown.length} so far</span>
          <Button variant="secondary" size="sm" loading={loadingMore} onClick={() => void onLoadMore()}>
            Load more
          </Button>
        </div>
      ) : null}
    </>
  );
}

// The props are those of `PageProps<"swimlane-pull-requests">` (src/routes/props.ts), written out because that route is added to the
// table by the integrator; the shapes are the same, so `pages.ts` accepts the page as it is.
type Props = { readonly params: { readonly id: string }; readonly search: { readonly state?: string } };

export default function PullRequestsPage({ params, search }: Props) {
  const swimlane = params.id;
  const filter = filterOf(search.state);
  const replace = useReplaceTo();
  const pages = useCursorPages({ ...prListQuery(listParams(swimlane, filter)) });
  // No `isEmpty`: an empty list is the Body's to draw, so that the last row leaving is held still like any other.
  const state = useQueryState(cursorPagesSource(pages));

  const choose = (next: string): void => {
    // Choosing the chosen filter again would clear the choice; the filter is never empty.
    const to = FILTERS.find((f) => f === next);
    if (to !== undefined && to !== filter) replace(linkTo("swimlane-pull-requests", { id: swimlane }, to === "open" ? {} : { state: to }));
  };

  const count = pages.status === "success" ? pages.items.length : null;
  const subtitle = pages.status === "pending" ? "Loading…" : pages.status === "error" ? "The list could not be read" : `${count ?? 0}${pages.hasMore ? "+" : ""} ${noun(filter, count ?? 0)}`;

  return (
    <div className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex flex-none flex-wrap items-end justify-between gap-x-4 gap-y-3 pb-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl leading-[26px] font-semibold tracking-[-0.01em]">Pull requests</h1>
          <span className="text-muted-foreground">
            <AppLink route="swimlane" params={{ id: swimlane }} className="font-mono">
              {swimlane}
            </AppLink>{" "}
            · {subtitle}
          </span>
        </div>
      </div>
      <div className="relative mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card" aria-busy={state.state === "loading" || undefined}>
        <div className="flex h-10 flex-none items-center gap-2 border-b border-border bg-muted px-2">
          <ToggleGroup type="single" value={filter} aria-label="State" onValueChange={choose} className="shrink-0 gap-0 overflow-hidden border-input bg-background p-0">
            {FILTERS.map((f) => (
              <ToggleGroupItem key={f} value={f} className={SEGMENT}>
                {LABEL(f)}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </div>
        {pages.truncated ? <TruncationBanner shown={pages.items.length} noun="pull requests" /> : null}
        <QueryBoundaryView
          state={state}
          noun="pull requests"
          source="the pull request list"
          className="flex min-h-0 flex-1 flex-col"
          loading={<ListSkeleton rows={6} columns={5} />}
        >
          {(rows) => <Body key={filter} rows={rows} swimlane={swimlane} filter={filter} hasMore={pages.hasMore} loadingMore={pages.isFetching} onLoadMore={pages.loadMore} />}
        </QueryBoundaryView>
        <div className="flex min-h-7 flex-none flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-muted px-3 text-xs text-muted-foreground">
          <span>One row for each file a pull request carries</span>
          <span className="ml-auto">Enter opens the pull request on GitHub</span>
        </div>
      </div>
    </div>
  );
}
