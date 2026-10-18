import { useCallback, useEffect, useMemo, useRef, useState, type UIEvent } from "react";
import type { GridRow, SwimlaneSummary } from "@/api/generated/model";
import { Banner } from "@/components/app/Banner";
import { visibleText } from "@/components/app/safe-text";
import { EmptyState } from "@/components/app/EmptyState";
import { ErrorState } from "@/components/app/ErrorState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Check, FileCode, Layers, RefreshCw, Search, WifiOff } from "@/icons";
import { useShortcut } from "@/keyboard";
import { cursorPagesSource, useQueryState, type QueryState } from "@/lib/query/use-query-state";
import { formatClock, formatRelative } from "@/lib/time";
import { useNow } from "@/live/use-now";
import { MAX_GRID_LANES, rowFacts, type CompareView } from "../model";
import { GRID_PAGE } from "../queries";
import { CellDrawer } from "./CellDrawer";
import { PathCell, ValueCell } from "./cells";
import { GridToolbar } from "./GridToolbar";
import { LaneHeader } from "./LaneHeader";
import { LanePicker } from "./LanePicker";
import { isOffline, laneColumns, type LaneColumn } from "./lanes";
import { defaultPick, pickCell, type Pick } from "./picks";
import { PickBar } from "./PickBar";
import { TooManyLanes } from "./TooManyLanes";
import { useGridPages } from "./useGridPages";

// "One file across swimlanes": rows are the settings of one logical file, columns are swimlanes (up to 40), and a cell is the
// value that swimlane reads. The rows come from the server in pages (50, then 500) and are drawn by the one virtualised table
// (rows and columns in view only). Everything the page says about a difference is the server's: the rows it lists with "Only
// differing rows", the comparison behind the drawer. The page marks the cells that stand apart from their row by comparing
// the values it was sent, and never looks at a hidden value.

const ROW_HEIGHT = 28;
const HEADER_HEIGHT = 32;

type Props = {
  view: CompareView;
  onView: (next: CompareView) => void;
  swimlanes: QueryState<SwimlaneSummary[]>;
};

export function GridScreen({ view, onView, swimlanes }: Props) {
  const data = swimlanes.data;
  const all = useMemo<readonly SwimlaneSummary[]>(() => data ?? [], [data]);
  const defaultIds = useMemo(() => all.slice(0, MAX_GRID_LANES).map((lane) => lane.id), [all]);
  const ids = view.lanes.length > 0 ? view.lanes : defaultIds;
  const idsKey = ids.join(",");
  const lanes = useMemo(() => laneColumns(ids, all), [ids, all]);
  const [filePickerOpen, setFilePickerOpen] = useState(false);
  const [lanePickerOpen, setLanePickerOpen] = useState(false);
  const over = ids.length > MAX_GRID_LANES;
  const waitingForLanes = view.lanes.length === 0 && swimlanes.state === "loading";

  const toolbar = {
    file: view.file,
    sides: ids.slice(0, 2),
    filePickerOpen,
    onFilePickerOpen: setFilePickerOpen,
    onFile: (file: string) => {
      setFilePickerOpen(false);
      onView({ ...view, file });
    },
    laneCount: ids.length,
    limit: MAX_GRID_LANES,
    onChooseLanes: () => {
      setLanePickerOpen(true);
    },
    onlyDiff: view.onlyDiff,
    onOnlyDiff: (on: boolean) => {
      onView({ ...view, onlyDiff: on });
    },
    channel: view.channel,
    onChannel: (channel: string | null) => {
      onView({ ...view, channel });
    },
  };

  const picker = (
    <LanePicker
      open={lanePickerOpen}
      onOpenChange={setLanePickerOpen}
      all={all}
      selected={ids}
      limit={MAX_GRID_LANES}
      onApply={(chosen) => {
        onView({ ...view, lanes: chosen });
      }}
    />
  );

  let body;
  if (waitingForLanes) {
    body = (
      <>
        <GridToolbar {...toolbar} channels={[]} q="" onQ={() => undefined} searchRef={null} rowsDisabled status={<LoadingStatus text="Reading swimlanes…" />} />
        <div role="status" aria-busy="true" aria-label="Loading swimlanes" className="p-4 text-sm text-muted-foreground">
          Loading swimlanes…
        </div>
      </>
    );
  } else if (view.lanes.length === 0 && swimlanes.state === "error" && swimlanes.error) {
    body = (
      <ErrorState title="Couldn't load comparison" problem={swimlanes.error} {...(swimlanes.retryable ? { onRetry: swimlanes.refetch } : {})}>
        The swimlane list could not be read, so the grid does not know which columns to show. Choose swimlanes in the address, or retry.
      </ErrorState>
    );
  } else if (over) {
    body = (
      <>
        <GridToolbar {...toolbar} channels={[]} q="" onQ={() => undefined} searchRef={null} rowsDisabled status={null} />
        <TooManyLanes
          lanes={ids}
          limit={MAX_GRID_LANES}
          onKeepFirst={() => {
            onView({ ...view, lanes: ids.slice(0, MAX_GRID_LANES) });
          }}
          onChoose={() => {
            setLanePickerOpen(true);
          }}
        />
      </>
    );
  } else if (ids.length === 0) {
    body = (
      <>
        <GridToolbar {...toolbar} channels={[]} q="" onQ={() => undefined} searchRef={null} rowsDisabled status={null} />
        <EmptyState icon={Layers} title="No swimlanes to compare" action={<Button onClick={toolbar.onChooseLanes}>Choose swimlanes</Button>}>
          No swimlane is registered yet, or none is open to you. Choose the swimlanes that are the columns of the grid.
        </EmptyState>
      </>
    );
  } else if (view.file === null) {
    body = (
      <>
        <GridToolbar {...toolbar} channels={[]} q="" onQ={() => undefined} searchRef={null} rowsDisabled status={null} />
        <EmptyState
          icon={FileCode}
          title="Choose a file to compare"
          action={
            <Button
              onClick={() => {
                setFilePickerOpen(true);
              }}
            >
              Choose a file
            </Button>
          }
        >
          Pick one logical file. Its settings are listed with one column for each of the {ids.length} swimlanes.
        </EmptyState>
      </>
    );
  } else {
    body = <GridData key={`${view.file}|${idsKey}|${view.onlyDiff}|${view.channel ?? ""}`} view={view} file={view.file} lanes={lanes} toolbar={toolbar} onView={onView} swimlanes={swimlanes} />;
  }

  return (
    <div className="relative mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card">
      {body}
      {picker}
    </div>
  );
}

function LoadingStatus({ text }: { text: string }) {
  return (
    <span role="status" className="inline-flex items-center gap-1.5 text-foreground">
      <Spinner />
      {text}
    </span>
  );
}

type DataProps = {
  view: CompareView;
  file: string;
  lanes: readonly LaneColumn[];
  toolbar: Omit<Parameters<typeof GridToolbar>[0], "channels" | "q" | "onQ" | "searchRef" | "rowsDisabled" | "status">;
  onView: (next: CompareView) => void;
  swimlanes: QueryState<SwimlaneSummary[]>;
};

/** The rows that contain the text in their path, and where each is among the rows loaded (the pages are counted in loaded rows). */
function filterRows(items: readonly GridRow[], q: string): { rows: readonly GridRow[]; indexes: readonly number[] | null } {
  const needle = q.trim().toLowerCase();
  if (needle === "") return { rows: items, indexes: null };
  const rows: GridRow[] = [];
  const indexes: number[] = [];
  items.forEach((row, index) => {
    if (row.path.toLowerCase().includes(needle)) {
      rows.push(row);
      indexes.push(index);
    }
  });
  return { rows, indexes };
}

function GridData({ view, file, lanes, toolbar, onView, swimlanes }: DataProps) {
  const spec = useMemo(() => ({ file, lanes: lanes.map((lane) => lane.id), onlyDiff: view.onlyDiff, channel: view.channel }), [file, lanes, view.onlyDiff, view.channel]);
  const pages = useGridPages(spec);
  const query = useQueryState(cursorPagesSource(pages));
  const [q, setQ] = useState("");
  const [pick, setPick] = useState<Pick | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const clock = useNow(true, 30_000);

  useShortcut("local.filter.focus", () => {
    searchRef.current?.focus();
    searchRef.current?.select();
  });

  const { rows, indexes } = useMemo(() => filterRows(pages.items, q), [pages.items, q]);

  // The channels the loaded paths name. A grid read for one channel holds only that one, so the names seen are kept for the visit.
  const [seen, setSeen] = useState<readonly string[]>([]);
  useEffect(() => {
    const found = new Set(seen);
    for (const row of pages.items) {
      if (!row.path.startsWith("channels.")) continue;
      const rest = row.path.slice("channels.".length);
      const end = rest.search(/[.[]/);
      const name = end < 0 ? rest : rest.slice(0, end);
      if (name !== "") found.add(name);
    }
    if (found.size !== seen.length) setSeen([...found].sort());
  }, [pages.items, seen]);

  // The cells pick through one function that never changes (a cell is memoised on it), so it reads the pick from a ref.
  const pickRef = useRef(pick);
  useEffect(() => {
    pickRef.current = pick;
  });
  const onPick = useCallback((row: GridRow, lane: number) => {
    const next = pickCell(pickRef.current, row, lane);
    pickRef.current = next.pick;
    setPick(next.pick);
    setDrawerOpen(next.open);
  }, []);
  const clearPick = () => {
    setPick(null);
    setDrawerOpen(false);
  };

  const pickedA = pick?.a ?? null;
  const pickedB = pick?.b ?? null;
  const pickedPath = pick?.row.path ?? null;
  const columns = useMemo<Column<GridRow>[]>(
    () => [
      { id: "path", header: "Setting path", width: "minmax(340px,1.6fr)", minWidth: 340, cell: (row) => <PathCell path={row.path} /> },
      ...lanes.map(
        (lane, index): Column<GridRow> => ({
          id: `lane-${lane.id}`,
          header: <LaneHeader lane={lane} />,
          width: "minmax(150px,1fr)",
          minWidth: 150,
          cell: (row) => {
            const facts = rowFacts(row.cells);
            const cell = row.cells[index];
            if (cell === undefined) return null;
            return (
              <ValueCell
                row={row}
                lane={index}
                cell={cell}
                differs={facts.differing.has(index)}
                muted={!facts.hidden && facts.differing.size === 0}
                picked={pickedPath === row.path && (pickedA === index || pickedB === index)}
                offline={isOffline(lane)}
                onPick={onPick}
              />
            );
          },
        }),
      ),
    ],
    [lanes, pickedPath, pickedA, pickedB, onPick],
  );

  const onScroll = (event: UIEvent<HTMLDivElement>) => {
    const el = event.currentTarget;
    const last = Math.max(0, Math.floor((el.scrollTop + el.clientHeight - HEADER_HEIGHT) / ROW_HEIGHT));
    pages.lookAt(indexes === null ? last : (indexes[Math.min(last, indexes.length - 1)] ?? last));
  };

  const offline = lanes.filter(isOffline);
  const filtered = q.trim() !== "" || view.channel !== null;
  const noun = view.onlyDiff ? "differing settings" : "settings";
  const loaded = pages.items.length;
  const status =
    query.state === "loading" ? (
      <LoadingStatus text={`Reading effective files · ${lanes.length} ${lanes.length === 1 ? "swimlane" : "swimlanes"}`} />
    ) : query.state === "ready" ? (
      <span>
        {q.trim() === "" ? loaded.toLocaleString("en-US") : `${rows.length.toLocaleString("en-US")} of ${loaded.toLocaleString("en-US")}`}
        {pages.hasMore ? "+" : ""} {noun}
        {pages.hasMore ? " so far" : ""}
      </span>
    ) : null;

  const emptyTitle = filtered ? "No settings match" : view.onlyDiff ? `All ${lanes.length} swimlanes have the same values` : "This file has no settings here";
  const empty = (
    <EmptyState
      icon={filtered ? Search : Check}
      title={emptyTitle}
      action={
        filtered ? (
          <Button
            variant="secondary"
            onClick={() => {
              setQ("");
              if (view.channel !== null) onView({ ...view, channel: null });
            }}
          >
            Clear search and channel
          </Button>
        ) : view.onlyDiff ? (
          <Button
            variant="secondary"
            onClick={() => {
              onView({ ...view, onlyDiff: false });
            }}
          >
            Show all rows
          </Button>
        ) : undefined
      }
    >
      {filtered
        ? `Nothing in ${file} matches${q.trim() === "" ? "" : ` "${q.trim()}"`}${view.channel === null ? "" : ` in the channel ${view.channel}`}${pages.hasMore ? ", among the settings loaded so far" : ""}.`
        : view.onlyDiff
          ? `Every setting in ${file} reads the same value in each swimlane.`
          : `${file} has no settings in these swimlanes. It may be empty, or not a file the grid can read setting by setting.`}
    </EmptyState>
  );

  const table = (
    <DataTable
      label={`${visibleText(file.split("/").pop() ?? file)} across swimlanes`}
      // The same hook the performance page has, so that the P10 specs that scroll the grid find the scroller on this screen too.
      data-testid="grid-scroller"
      density="compact"
      className="flex-1"
      rows={rows}
      columns={columns}
      rowKey={(row) => row.path}
      onScroll={onScroll}
      onActivate={(row) => {
        setPick(defaultPick(row, lanes.length));
        setDrawerOpen(lanes.length > 1);
      }}
      empty={empty}
      footer={{ hasMore: pages.hasMore, onLoadMore: () => void pages.loadMore(), loading: pages.isFetching, truncated: pages.truncated, step: GRID_PAGE }}
    />
  );
  const skeleton = <DataTable label="Settings" density="compact" state="loading" rows={[]} columns={columns} rowKey={(row) => row.path} className="flex-1" />;

  const a = pickedA === null ? undefined : lanes[pickedA];
  const b = pickedB === null ? undefined : lanes[pickedB];
  // The row of the pick as it is now (a live refresh may have changed it), or as it was when it was picked.
  const pickRow = pick === null ? null : (rows.find((r) => r.path === pick.row.path) ?? pick.row);
  const retry = () => {
    swimlanes.refetch();
    void pages.refetch();
  };

  return (
    <>
      <GridToolbar {...toolbar} channels={seen} q={q} onQ={setQ} searchRef={searchRef} rowsDisabled={query.state !== "ready"} status={status} />
      {pick === null || pickRow === null ? null : (
        <PickBar
          row={pickRow}
          lanes={lanes}
          a={pick.a}
          b={pick.b}
          drawerOpen={drawerOpen}
          onA={(index) => {
            setPick({ ...pick, a: index });
          }}
          onB={(index) => {
            setPick({ ...pick, b: index });
          }}
          onToggleDrawer={() => {
            setDrawerOpen((open) => !open);
          }}
          onClear={clearPick}
        />
      )}
      {offline.length === 0 || query.state === "loading" ? null : <OfflineBanner lanes={offline} total={lanes.length} now={clock} onRetry={retry} />}
      {pages.truncated ? <TruncationBanner shown={loaded} noun="settings" className="border-b" /> : null}
      {/* The drawer is positioned against this box, not the whole grid: it starts under the toolbar, the pick bar and the banners, so the
          bar's buttons (Hide diff, Clear) stay where a pointer can reach them while the diff is open. */}
      <div className="relative flex min-h-0 flex-1 flex-col">
        <QueryBoundaryView state={query} noun="comparison" source={`${file} across ${lanes.length} swimlanes`} className="flex min-h-0 flex-1 flex-col" loading={skeleton}>
          {() => table}
        </QueryBoundaryView>
        <div className="flex min-h-7 flex-none flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-muted px-3 py-1 text-xs text-muted-foreground">
          <span className="inline-flex items-center gap-1.5">
            <span aria-hidden="true" className="size-3 rounded-[2px] border border-warning-border bg-diff-mod-bg" />
            Value differs
          </span>
          <span>
            <em>absent</em> = key is not in the file
          </span>
          <span>
            <span className="font-mono">&quot;&quot;</span> empty = key is there, value is blank
          </span>
          <span>Hidden = the secret scan flagged it, so it is never shown. Only the server can say whether two hidden values are equal.</span>
          <span>Shared settings always show. Channel filter applies to channels.*</span>
        </div>
        {drawerOpen && pick !== null && pickRow !== null && a !== undefined && b !== undefined && pickedA !== null && pickedB !== null && pickedA !== pickedB ? (
          <CellDrawer
            file={file}
            channel={view.channel}
            row={pickRow}
            a={{ lane: a, index: pickedA }}
            b={{ lane: b, index: pickedB }}
            onClose={() => {
              setDrawerOpen(false);
            }}
            onSwap={() => {
              setPick({ ...pick, a: pick.b, b: pick.a });
            }}
          />
        ) : null}
      </div>
    </>
  );
}

function OfflineBanner({ lanes, total, now, onRetry }: { lanes: readonly LaneColumn[]; total: number; now: number; onRetry: () => void }) {
  const first = lanes[0];
  if (first === undefined) return null;
  const asOf = (lane: LaneColumn) => (lane.asOf === null ? `${lane.id} (no scan on record)` : `${lane.id} as of ${formatClock(lane.asOf)}, ${formatRelative(lane.asOf, now)}`);
  const live = total - lanes.length;
  return (
    <Banner
      variant="warning"
      icon={<WifiOff aria-hidden="true" className="size-3.5" />}
      title={lanes.length === 1 ? `${first.id} agent is offline` : `${lanes.length} agents are offline`}
      action={
        <Button variant="secondary" size="sm" onClick={onRetry}>
          <RefreshCw data-icon="inline-start" />
          {lanes.length === 1 ? `Retry ${first.id}` : "Retry"}
        </Button>
      }
    >
      {lanes.length === 1 ? `Its column shows the NFS files ${first.asOf === null ? "from the last scan" : `as of ${formatClock(first.asOf)}, ${formatRelative(first.asOf, now)}`}.` : `Their columns show the NFS files of their last scan: ${lanes.map(asOf).join("; ")}.`}{" "}
      {live === 0 ? "No column is live." : `The other ${live} ${live === 1 ? "column is" : "columns are"} live.`}
    </Banner>
  );
}
