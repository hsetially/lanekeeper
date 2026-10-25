import { useMemo } from "react";
import { Role, type Finding, type SwimlaneSummary } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { roleAtLeast } from "@/auth/roles";
import { Clock } from "@/icons";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { formatClock } from "@/lib/time";
import { useNow } from "@/live/use-now";
import { linkTo } from "@/routes/links";
import type { PageProps } from "@/routes/props";
import { FilterBar, type LaneChoice } from "./FilterBar";
import { FindingsResults } from "./FindingsResults";
import { clientLanes, filtersFromSearch, hrefOf, serverParams, type Filters } from "./filters";
import { KINDS } from "./kinds";
import { useReplaceTo } from "@/routes/AppLink";
import { OfflineBanner } from "./OfflineBanner";
import { latestScan } from "./scan";
import { FINDING_MAX_PAGES, findingListQuery } from "./queries";

// The Findings screen: what the checks (C1 to C12) found, across the swimlanes, with the filters of the design, an order, and a
// drawer with the evidence and the next steps. Everything it shows is what the server said: the registry finds, kinds and rates
// the findings (the engine's checks, the admin's severity rules), and nothing here computes any of it. What the screen does
// itself is put the loaded findings in an order, and pick out several swimlanes when the person chose several (the server's filter
// takes one); it says so where it matters.
//
// The design's "Intentional" tab, "Mark intentional" and "Restore lost version" are not here: the contract has no operation for
// them yet (CG6). The adoption wizard is a screen of its own (S6b).

/** The findings the person chose several swimlanes of, picked out of what the server sent. The same array when there is nothing to pick. */
function narrowed(items: Finding[], filters: Filters): Finding[] {
  const keep = clientLanes(filters);
  return keep === null ? items : items.filter((finding) => keep.has(finding.swimlane));
}

/** What the Swimlane menu offers: the swimlane list, or, when that could not be read, the swimlanes the findings and the filter name. */
function laneChoices(lanes: readonly SwimlaneSummary[], findings: readonly Finding[], chosen: readonly string[]): LaneChoice[] {
  if (lanes.length > 0) return lanes.map((lane) => ({ id: lane.id, tier: lane.tier ?? null, count: lane.finding_count }));
  const ids = [...new Set([...findings.map((f) => f.swimlane), ...chosen])].sort();
  return ids.map((id) => ({ id, tier: null, count: null }));
}

const plural = (n: number, one: string, many: string): string => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

export default function FindingsPage({ search }: PageProps<"findings">) {
  const session = useSession();
  const isAdmin = session.status === "ready" && roleAtLeast(session.role, Role.admin);
  const replace = useReplaceTo();
  const now = useNow(true, 30_000);

  const filters = useMemo(() => filtersFromSearch(search), [search]);
  const params = useMemo(() => serverParams(filters), [filters]);
  const lanesPages = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const pages = useCursorPages({ ...findingListQuery(params), autoLoad: true, maxPages: FINDING_MAX_PAGES });

  const lanes = lanesPages.items;
  const lanesKnown = lanesPages.status === "success" && !lanesPages.hasMore;
  const shown = useMemo(() => narrowed(pages.items, filters), [pages.items, filters]);
  const query = useQueryState({ ...cursorPagesSource(pages), data: pages.status === "success" ? shown : undefined }, { isEmpty: (items) => items.length === 0 });

  const choices = useMemo(() => laneChoices(lanes, pages.items, filters.lanes), [lanes, pages.items, filters.lanes]);
  const change = (next: Filters): void => {
    replace(hrefOf(next));
  };

  const total = lanes.reduce((sum, lane) => sum + lane.finding_count, 0);
  const withFindings = lanes.filter((lane) => lane.finding_count > 0).length;
  const lastScan = latestScan(lanes);
  const subtitle =
    query.state === "loading"
      ? "Loading…"
      : query.state === "error"
        ? "Error"
        : lanesKnown
          ? total === 0
            ? "Nothing to fix"
            : `${total.toLocaleString("en-US")} open across ${plural(withFindings, "swimlane", "swimlanes")}`
          : plural(shown.length, "finding", "findings");
  const summary = query.state === "loading" ? "Loading…" : query.state === "error" ? "" : shown.length === 0 ? "0 findings" : `Showing ${plural(shown.length, "finding", "findings")}`;

  // The list restarts (a new stable list, a closed drawer) when the filters change, and when it goes from nothing to something.
  const phase = query.data === undefined ? "pending" : query.data.length === 0 ? "empty" : "list";

  return (
    <div className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex flex-none flex-wrap items-end justify-between gap-x-4 gap-y-2 pb-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl leading-[26px] font-semibold tracking-[-0.01em]">Findings</h1>
          <span className="text-muted-foreground">{subtitle}</span>
        </div>
        {lanesKnown && lanes.length > 0 ? (
          <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <Clock aria-hidden="true" className="size-3" />
            {lastScan === null ? "No scan yet" : `Last scan ${formatClock(lastScan)}`} · {KINDS.length} checks across {lanes.length} swimlanes
          </span>
        ) : null}
      </div>
      <div className="relative mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card" aria-busy={query.state === "loading" || undefined}>
        <FilterBar filters={filters} onChange={change} lanes={choices} summary={summary} />
        <OfflineBanner lanes={lanes} now={now} canCheck={isAdmin} />
        <FindingsResults
          key={`${hrefOf(filters)}|${phase}`}
          query={query}
          loaded={pages.items.length}
          incomplete={pages.hasMore || pages.truncated}
          truncated={pages.truncated}
          lanesHere={clientLanes(filters) !== null}
          lanes={lanes}
          lanesKnown={lanesKnown}
          filters={filters}
          onClearFilters={() => {
            replace(linkTo("findings"));
          }}
        />
      </div>
    </div>
  );
}
