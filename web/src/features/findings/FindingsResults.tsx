import { useCallback, useMemo, useRef, useState } from "react";
import type { Finding, SwimlaneSummary } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { DataTable, type Column, type SortState } from "@/components/app/virtual/DataTable";
import { SeverityBadge } from "@/components/attribution";
import { Button } from "@/components/ui/button";
import { CircleCheck, RefreshCw, Search } from "@/icons";
import { formatClock } from "@/lib/time";
import type { QueryState } from "@/lib/query/use-query-state";
import { useStableList } from "@/live/stable-list";
import { openInNewTab } from "@/routes/AppLink";
import { CheckCell, LinesCell, PathCell, SwimlaneCell } from "./cells";
import { FindingDrawer } from "./FindingDrawer";
import { isFiltered, type Filters } from "./filters";
import { fileHref } from "./lines";
import { KINDS, codeOf } from "./kinds";
import { latestScan } from "./scan";
import { DEFAULT_SORT, sortFindings } from "./severity";

// The results of the Findings screen: the table, the drawer for the finding that was opened, and the states of the list
// (loading, empty, error, stale). The page remounts this when the filters change or the list goes from nothing to something,
// which is what starts the stable list over (a list held still while it is read must not be the list of another filter).
//
// The table is the shared DataTable: arrows move, Enter opens, a click on a row opens, and a click on a link in a row goes to the
// file. It does not sort by itself; the findings are sorted here, in the findings that are loaded, because the server lists them
// newest first and can be asked for no other order.

const keyOf = (finding: Finding): string => String(finding.id);

export type ResultsProps = {
  query: QueryState<Finding[]>;
  /** How many findings are loaded, before the swimlanes the person chose are picked out of them. */
  loaded: number;
  /** More pages are on their way, or the screen stopped asking for them. */
  incomplete: boolean;
  truncated: boolean;
  /** Several swimlanes were chosen, which the server cannot filter by: they are picked out of what is loaded. */
  lanesHere: boolean;
  lanes: readonly SwimlaneSummary[];
  /** The swimlane list is complete, so its counts and times can be said. */
  lanesKnown: boolean;
  filters: Filters;
  onClearFilters: () => void;
};

function EmptyResults({ filtered, lanes, lanesKnown, onClear }: { filtered: boolean; lanes: readonly SwimlaneSummary[]; lanesKnown: boolean; onClear: () => void }) {
  if (filtered) {
    const total = lanes.reduce((sum, lane) => sum + lane.finding_count, 0);
    return (
      <EmptyState
        icon={Search}
        title="No findings match these filters"
        action={<Button onClick={onClear}>Clear filters</Button>}
        className="flex-1"
      >
        Loosen a filter, or clear them all to see {lanesKnown ? `${total} findings` : "every open finding"}.
      </EmptyState>
    );
  }
  const last = latestScan(lanes);
  return (
    <EmptyState icon={CircleCheck} title="No open findings" className="flex-1">
      {lanesKnown && lanes.length > 0
        ? `${last === null ? "The checks" : `The last scan at ${formatClock(last)}`} ran ${KINDS.length} checks on ${lanes.length} swimlanes and found nothing to fix. New findings appear here as soon as a check raises one.`
        : "No check has found anything to fix. New findings appear here as soon as a check raises one."}
    </EmptyState>
  );
}

export function FindingsResults({ query, loaded, incomplete, truncated, lanesHere, lanes, lanesKnown, filters, onClearFilters }: ResultsProps) {
  const all = useMemo(() => query.data ?? [], [query.data]);
  // The findings load in pages: the rows of the next page come after the ones on screen and are not a live change (see useStableList).
  const stable = useStableList(all, keyOf, { appendsApply: true });
  const [sort, setSort] = useState<SortState | null>(DEFAULT_SORT);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const box = useRef<HTMLDivElement>(null);

  const laneOrder = useMemo(() => new Map(lanes.map((lane, i): [string, number] => [lane.id, i])), [lanes]);
  const offline = useMemo(() => new Set(lanes.filter((lane) => lane.agent === "disconnected").map((lane) => lane.id)), [lanes]);
  const rows = useMemo(() => sortFindings(stable.shown, sort, laneOrder), [stable.shown, sort, laneOrder]);
  const selected = selectedId === null ? null : (rows.find((finding) => finding.id === selectedId) ?? null);

  const columns = useMemo<Column<Finding>[]>(
    () => [
      { id: "severity", header: "Severity", width: "104px", minWidth: 104, sortable: true, cell: (f) => <SeverityBadge severity={f.severity} /> },
      { id: "check", header: "Check", width: "208px", minWidth: 188, sortable: true, cell: (f) => <CheckCell kind={f.kind} code={codeOf(f.kind)} /> },
      { id: "swimlane", header: "Swimlane", width: "96px", minWidth: 88, sortable: true, cell: (f) => <SwimlaneCell id={f.swimlane} offline={offline.has(f.swimlane)} /> },
      { id: "file", header: "File", width: "minmax(0,1fr)", minWidth: 200, sortable: true, cell: (f) => <PathCell path={f.path} /> },
      { id: "wrong", header: "What is wrong", width: "minmax(0,1.3fr)", minWidth: 240, cell: (f) => <SafeText text={f.message} /> },
      { id: "lines", header: "Lines", width: "104px", minWidth: 96, cell: (f) => <LinesCell finding={f} /> },
    ],
    [offline],
  );

  const open = useCallback((finding: Finding) => {
    setSelectedId(finding.id);
  }, []);
  const focusTable = useCallback(() => {
    box.current?.querySelector<HTMLElement>('[role="grid"]')?.focus();
  }, []);

  const selectedKeys = useMemo<ReadonlySet<string>>(() => new Set(selectedId === null ? [] : [String(selectedId)]), [selectedId]);
  const filtered = isFiltered(filters);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {stable.pending > 0 ? (
        <div role="status" className="flex h-8 flex-none items-center gap-2 border-b border-border bg-info-bg px-3 text-xs text-info-fg">
          <span className="tabular-nums">
            {stable.pending} {stable.pending === 1 ? "update" : "updates"}
          </span>
          <span>The list is held still while you read it.</span>
          <Button variant="secondary" size="sm" onClick={stable.apply} className="ml-auto">
            <RefreshCw data-icon="inline-start" />
            Show
          </Button>
        </div>
      ) : null}
      {truncated ? <TruncationBanner shown={loaded} noun="findings" /> : null}
      <div ref={box} className="flex min-h-0 flex-1 flex-col">
        <QueryBoundaryView
          state={query}
          noun="findings"
          source="the findings list"
          className="flex min-h-0 flex-1 flex-col"
          loading={<DataTable label="Findings" state="loading" rows={[]} columns={columns} rowKey={keyOf} />}
          empty={<EmptyResults filtered={filtered} lanes={lanes} lanesKnown={lanesKnown} onClear={onClearFilters} />}
        >
          {() => (
            <DataTable
              label="Findings"
              className="min-h-0 flex-1"
              rows={rows}
              columns={columns}
              rowKey={keyOf}
              sort={sort}
              onSortChange={setSort}
              selectedKeys={selectedKeys}
              onActivate={open}
              onRowClick={open}
              onOpenInNewTab={(finding) => {
                const href = fileHref(finding);
                if (href !== null) openInNewTab(href);
              }}
            />
          )}
        </QueryBoundaryView>
      </div>
      <div className="flex min-h-7 flex-none flex-wrap items-center gap-x-4 gap-y-0.5 border-t border-border bg-muted px-3 text-xs text-muted-foreground">
        <span>Severity is set by the hub's rules</span>
        {incomplete ? <span>{lanesHere ? "Sorted and filtered by swimlane" : "Sorted"} within the {loaded.toLocaleString("en-US")} loaded findings.</span> : null}
        <span className="ml-auto">Click a row for evidence and next steps</span>
      </div>
      {selectedId === null ? null : (
        <FindingDrawer
          finding={selected}
          onClose={() => {
            setSelectedId(null);
          }}
          onCloseFocus={focusTable}
        />
      )}
    </div>
  );
}
