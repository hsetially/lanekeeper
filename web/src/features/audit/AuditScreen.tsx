import { useMemo, useRef, useState } from "react";
import type { AuditEvent } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { usePeople } from "@/features/approvals/people";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { History, RefreshCw, Search } from "@/icons";
import { useShortcut } from "@/keyboard";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { formatClock } from "@/lib/time";
import { cn } from "@/lib/utils";
import { linkTo } from "@/routes/links";
import type { SearchOf } from "@/routes/props";
import { useReplaceTo } from "@/routes/AppLink";
import { AuditTable, ROW_HEIGHT } from "./AuditTable";
import { EventDrawer } from "./EventDrawer";
import { FilterBar } from "./FilterBar";
import { IntegrityPanel } from "./IntegrityPanel";
import { auditParams, applyQuick, chainOf, filtersFromSearch, newestFirst, quickCounts, searchOf, type Filters, type Quick } from "./model";
import { useNewerEvents } from "./newer";
import { auditListQuery } from "./queries";

// The audit log (Admin only, W11): every audited event the hub keeps, newest first, with who made it, how it arrived, what it was and
// how sure the attribution is. The hub's `adminListAudit` takes a user, an action, a swimlane and a range of days, and pages by a
// cursor; those are the filters. The quick filters (Cursor, unknown author) work on the events that are loaded and say so.
//
// Nothing names the audit trail in the live stream, so the log is not live: it says when it was read, and counts the events that
// may have added to it ("n updates") and reads it again on Refresh. Nothing polls.

const QUICK: ReadonlyArray<{ id: Quick; label: string }> = [
  { id: "all", label: "All events" },
  { id: "cursor", label: "Via Cursor" },
  { id: "unknown", label: "Unknown author" },
];

function LogSkeleton() {
  return (
    <div data-slot="log-skeleton" className="flex flex-col">
      {Array.from({ length: 8 }, (_, row) => (
        <div key={row} className="flex items-center gap-3 border-b border-border px-3" style={{ height: ROW_HEIGHT }}>
          <Skeleton className="h-3 w-16" />
          <Skeleton className="h-3 w-24" />
          <Skeleton className="h-3 w-14" />
          <Skeleton className="h-3 w-14" />
          <Skeleton className="h-3 w-10" />
          <Skeleton className="h-3 w-40" />
          <Skeleton className="h-3 w-24" />
        </div>
      ))}
    </div>
  );
}

const plural = (n: number, one: string, many: string): string => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

export function AuditScreen({ search }: { search: SearchOf<"audit"> }) {
  const people = usePeople();
  const replace = useReplaceTo();

  // The filters are the address's. Their object is made again only when a value changes, so the key of the query stays the same.
  const { actor, action, swimlane, from, to } = search;
  const filters = useMemo<Filters>(() => filtersFromSearch({ actor, action, swimlane, from, to }), [actor, action, swimlane, from, to]);
  const filtered = Object.keys(filters).length > 0;
  const change = (next: Filters): void => {
    replace(linkTo("audit", undefined, searchOf(next)));
  };

  const pages = useCursorPages({ ...auditListQuery(auditParams(filters)) });
  const query = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const lanes = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const ordered = useMemo(() => newestFirst(pages.items), [pages.items]);
  const chain = useMemo(() => chainOf(ordered), [ordered]);
  const [quick, setQuick] = useState<Quick>("all");
  const counts = useMemo(() => quickCounts(ordered), [ordered]);
  const rows = useMemo(() => applyQuick(ordered, quick), [ordered, quick]);
  const newer = useNewerEvents(pages.dataUpdatedAt);

  const [selectedId, setSelectedId] = useState<number | null>(null);
  const selected = selectedId === null ? undefined : ordered.find((e) => e.id === selectedId);
  const previous = useMemo(() => (selected === undefined ? undefined : ordered.find((e) => e.seq === selected.seq - 1)), [ordered, selected]);
  // The table is where the drawer was opened from: focus goes back to it, so the person does not start again from the top of the
  // screen (the drawer's title had focus, and it leaves with the drawer).
  const box = useRef<HTMLDivElement>(null);
  const close = (): void => {
    box.current?.querySelector<HTMLElement>('[role="grid"]')?.focus({ preventScroll: true });
    setSelectedId(null);
  };
  useShortcut("global.close-dialog-menu-or-palette", close, { enabled: selectedId !== null });

  const loadedText = pages.status === "success" ? `Showing ${rows.length.toLocaleString("en-US")} of ${ordered.length.toLocaleString("en-US")} loaded${pages.hasMore ? " so far" : ""}` : pages.status === "pending" ? "Loading…" : "";
  const read = pages.dataUpdatedAt > 0 ? `Read at ${formatClock(pages.dataUpdatedAt)}` : null;
  const clear = (): void => {
    setQuick("all");
    change({});
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex flex-none flex-wrap items-end justify-between gap-x-4 gap-y-3 pb-3">
        <div className="flex max-w-3xl flex-col gap-1">
          <h1 className="text-xl leading-[26px] font-semibold tracking-[-0.01em]">Audit log</h1>
          <span className="text-muted-foreground">Every audited change: who made it, how it arrived, and what was recorded. Append-only and hash-chained.</span>
        </div>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          {read === null ? null : <span>{read}</span>}
          <Button variant="secondary" size="sm" loading={pages.isFetching && pages.status === "success"} onClick={() => void pages.refetch()}>
            <RefreshCw data-icon="inline-start" />
            Refresh
          </Button>
        </div>
      </div>
      <IntegrityPanel
        chain={chain}
        onShow={(id) => {
          setQuick("all");
          setSelectedId(id);
        }}
      />
      <div ref={box} className="relative mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card" aria-busy={query.state === "loading" || undefined}>
        <div className="relative z-[3] flex h-10 flex-none items-center gap-2 border-b border-border bg-muted px-2">
          <div role="group" aria-label="Quick filter" className="inline-flex overflow-hidden rounded-sm border border-input bg-background">
            {QUICK.map(({ id, label }) => (
              <button
                key={id}
                type="button"
                aria-pressed={quick === id}
                onClick={() => {
                  setQuick(id);
                }}
                className={cn("inline-flex h-[26px] items-center gap-1.5 border-0 border-l border-input px-2.5 text-sm font-medium first:border-l-0 hover:bg-accent", quick === id && "bg-secondary")}
              >
                {label}
                {pages.status === "success" ? (
                  <>
                    {" "}
                    <span className="font-normal text-muted-foreground tabular-nums">{counts[id]}</span>
                  </>
                ) : null}
              </button>
            ))}
          </div>
          {newer > 0 ? (
            <span role="status" className="inline-flex items-center gap-2 text-xs text-info-fg">
              <span className="tabular-nums">{plural(newer, "update", "updates")}</span>
              <span className="text-muted-foreground">Events may have been added since the log was read.</span>
            </span>
          ) : null}
          <span className="ml-auto text-xs text-muted-foreground tabular-nums">{loadedText}</span>
        </div>
        <FilterBar filters={filters} users={people.all} swimlanes={lanes.items} onChange={change} />
        {pages.truncated ? <TruncationBanner shown={ordered.length} noun="events" /> : null}
        <QueryBoundaryView
          state={query}
          noun="audit events"
          source="the audit log"
          className="flex min-h-0 flex-1 flex-col"
          loading={<LogSkeleton />}
          empty={
            filtered ? (
              <EmptyState icon={Search} title="No audit events match" action={<Button variant="secondary" onClick={clear}>Clear filters</Button>}>
                Widen the date range or clear the filters.
              </EmptyState>
            ) : (
              <EmptyState icon={History} title="No audit events yet">
                Nothing has been audited yet.
              </EmptyState>
            )
          }
        >
          {() =>
            rows.length === 0 ? (
              <EmptyState icon={Search} title="No events match these filters" action={<Button variant="secondary" onClick={clear}>Clear filters</Button>}>
                Loosen a filter, or clear them all to see {ordered.length.toLocaleString("en-US")} events.
              </EmptyState>
            ) : (
              <AuditTable
                rows={rows}
                selectedId={selectedId}
                breaks={chain.breaks}
                people={people}
                onOpen={(event: AuditEvent) => {
                  setSelectedId(event.id);
                }}
                moreRows={pages.hasMore}
                className="flex-1"
              />
            )
          }
        </QueryBoundaryView>
        {pages.hasMore ? (
          <div className="flex h-9 flex-none items-center gap-3 border-t border-border px-3 text-xs text-muted-foreground">
            <span className="tabular-nums">{loadedText}</span>
            <Button variant="secondary" size="sm" loading={pages.isFetching} onClick={() => void pages.loadMore()}>
              Load more
            </Button>
          </div>
        ) : null}
        <div className="flex min-h-7 flex-none flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-muted px-3 text-xs text-muted-foreground">
          <span className="tabular-nums">{loadedText}</span>
          <span>Attribution says how Lanekeeper identified the author, and how sure it is</span>
          <span className="ml-auto">Click a row for the diff, evidence and chain hash</span>
        </div>
        {selected === undefined ? null : (
          <EventDrawer
            key={selected.id}
            event={selected}
            previous={previous}
            broken={chain.breaks.get(selected.id)}
            people={people}
            onClose={close}
          />
        )}
      </div>
    </div>
  );
}
