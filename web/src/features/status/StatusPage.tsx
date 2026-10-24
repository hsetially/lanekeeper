import { useQuery } from "@tanstack/react-query";
import { useCallback, useId, useMemo, useRef, useState } from "react";
import type { FlagStatus } from "@/api/generated/model";
import { AgentDot } from "@/components/app/AgentDot";
import { Banner } from "@/components/app/Banner";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { CopyValue } from "@/components/app/CopyValue";
import { isMaskedValue } from "@/components/app/copy-text";
import { SafeText } from "@/components/app/SafeText";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { agentView } from "@/features/swimlanes/agent";
import { useLanes, type LaneInfo } from "@/features/search/lanes";
import { Button } from "@/components/ui/button";
import { SearchInput } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { ArrowLeftRight, BookOpen, ChevronDown, ChevronRight, ChevronsUpDown, Split, Zap } from "@/icons";
import { useShortcut } from "@/keyboard";
import { useQueryState } from "@/lib/query/use-query-state";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import type { PageProps } from "@/routes/props";
import { combine } from "./combine";
import { buildRows, groupFlags, type Cell, type StatusRow } from "./model";
import { featureStatusQuery } from "./queries";

// Feature status (the design's Feature Status page): the documented flags against the swimlanes. One row for each flag, with
// the template default, and a cell for each swimlane that says what the swimlane has; a cell that differs from the default is
// marked with an icon and a word as well as a colour. Open a flag to see its channels. The values are the server's (the docs
// say which flags there are, the registry says what each swimlane has); the screen only lays them out.

const MAX_COMPARE_LANES = 64;

/** The agent's connection, as the three states the design draws: the contract has connected, disconnected and never seen. */

function ValueCell({ cell, lane, row, compareLanes, offline }: { cell: Cell; lane: LaneInfo; row: StatusRow; compareLanes: string; offline: boolean }) {
  if (cell.state === "absent") {
    return (
      <span data-offline={offline || undefined} className={offline ? "pl-1.5 text-muted-foreground italic" : "pl-1.5 text-muted-foreground"} title="Key not found in this swimlane">
        —
      </span>
    );
  }
  const setting = row.setting ?? row.flag;
  const title = `${lane.name} · ${setting} = ${cell.text}${cell.differs && row.def !== null ? ` · template default ${row.def}` : ""}`;
  const Icon = cell.mixed ? Split : ArrowLeftRight;
  return (
    <span data-offline={offline || undefined} className={offline ? "italic" : undefined}>
      <AppLink
        route="compare"
        search={{ file: row.file, ...(row.kind === "channel" ? { channel: row.name } : {}), ...(compareLanes === "" ? {} : { lanes: compareLanes }) }}
        title={title}
        {...(cell.differs ? { "data-differs": "true" } : {})}
        className={
          cell.differs
            ? "inline-flex h-5 max-w-full items-center gap-1 rounded-sm border border-warning-border bg-diff-mod-bg px-1.5 font-mono text-xs font-semibold text-diff-mod-fg no-underline hover:bg-diff-mod-gutter hover:text-diff-mod-fg"
            : "pl-1.5 font-mono text-xs text-foreground no-underline hover:text-foreground hover:underline"
        }
      >
        {cell.differs ? <Icon aria-hidden="true" className="size-2.5" /> : null}
        <SafeText text={cell.text} className="truncate" />
      </AppLink>
    </span>
  );
}

export default function StatusPage({ search }: PageProps<"status">) {
  const filterRef = useRef<HTMLInputElement>(null);
  const switchId = useId();
  const { lanes, source: laneSource } = useLanes();
  const laneState = useQueryState(laneSource);
  const flagsQuery = useQuery(featureStatusQuery({}));
  const flagState = useQueryState(flagsQuery, { isEmpty: (data) => data.flags.length === 0 });
  const state = combine(flagState, laneState);

  const [filter, setFilter] = useState("");
  const [onlyDiffering, setOnlyDiffering] = useState(false);
  // Which flags are open: what the person chose, else open only for the flag a link named (`?setting=`).
  const [chosen, setChosen] = useState<ReadonlyMap<string, boolean>>(new Map());

  useShortcut("local.filter.focus", () => {
    const field = filterRef.current;
    if (field === null) return false;
    field.focus();
    field.select();
    return undefined;
  });

  const flags = useMemo(() => state.data?.[0].flags ?? [], [state.data]);
  const laneIds = useMemo(() => lanes.map((l) => l.id), [lanes]);
  const focusSetting = search.setting;
  const focusEntry = useMemo(() => (focusSetting === undefined ? undefined : flags.find((f) => f.setting === focusSetting)), [flags, focusSetting]);
  const focusFlag = focusEntry?.channel !== null && focusEntry !== undefined ? focusEntry.flag : undefined;
  const channelFlags = useMemo(() => groupFlags(flags).filter((g) => g.channels.length > 0).map((g) => g.name), [flags]);
  const expanded = useMemo(() => new Set(channelFlags.filter((name) => chosen.get(name) ?? name === focusFlag)), [channelFlags, chosen, focusFlag]);
  const built = useMemo(() => buildRows(flags, laneIds, { expanded, filter, onlyDiffering }), [flags, laneIds, expanded, filter, onlyDiffering]);

  const compareLanes = laneIds.length > 0 && laneIds.length <= MAX_COMPARE_LANES ? laneIds.join(",") : "";
  const offline = useMemo(() => lanes.filter((l) => l.agent !== "connected"), [lanes]);

  const toggle = useCallback(
    (name: string): void => {
      setChosen((previous) => new Map(previous).set(name, !(previous.get(name) ?? name === focusFlag)));
    },
    [focusFlag],
  );

  const columns = useMemo<Column<StatusRow>[]>(
    () => [
      {
        id: "flag",
        header: "Flag",
        width: "minmax(220px,1.4fr)",
        minWidth: 220,
        cell: (row) =>
          row.kind === "channel" ? (
            <span className="flex items-center pl-[26px]">
              <SafeText text={row.name} className="truncate font-mono text-xs text-muted-foreground" />
            </span>
          ) : (
            <span className="flex items-center gap-1.5">
              {row.channels > 0 ? (
                <button
                  type="button"
                  aria-label={`Toggle channels of ${row.flag}`}
                  aria-expanded={expanded.has(row.flag)}
                  onClick={() => toggle(row.flag)}
                  className="inline-flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent text-muted-foreground hover:bg-secondary-hover"
                >
                  {expanded.has(row.flag) ? <ChevronDown aria-hidden="true" className="size-3.5" /> : <ChevronRight aria-hidden="true" className="size-3.5" />}
                </button>
              ) : (
                <span className="w-5 shrink-0" />
              )}
              <SafeText text={row.name} className={row.flag === focusFlag || row.setting === focusSetting ? "truncate rounded-sm bg-selection px-1 font-mono text-xs font-medium" : "truncate font-mono text-xs font-medium"} />
            </span>
          ),
      },
      {
        id: "doc",
        header: "Doc",
        width: "168px",
        minWidth: 120,
        cell: (row) =>
          row.kind === "channel" ? null : (
            <AppLink route="search" search={{ type: "docs", q: row.flag }} title={`Search the docs for ${row.flag}`} className="inline-flex max-w-full items-center gap-1.5 text-xs no-underline hover:underline">
              <BookOpen aria-hidden="true" className="size-3 shrink-0" />
              Docs
            </AppLink>
          ),
      },
      { id: "default", header: "Default", width: "80px", minWidth: 80, cell: (row) => <span className="font-mono text-xs text-muted-foreground">{row.def === null ? "—" : <SafeText text={row.def} />}</span> },
      ...lanes.map<Column<StatusRow>>((lane) => ({
        id: `lane:${lane.id}`,
        header: (
          <span className="flex min-w-0 flex-col text-xs leading-4 font-normal">
            <span className="flex min-w-0 items-center gap-1.5 text-foreground">
              <AgentDot state={agentView(lane.agent).state} />
              <SafeText text={lane.name} className="min-w-0 truncate font-mono font-medium" />
            </span>
            <span className={cn("truncate", lane.agent !== "connected" ? "text-muted-foreground" : (built.differPerLane.get(lane.id) ?? 0) > 0 ? "text-diff-mod-fg" : "text-muted-foreground")}>
              {lane.agent !== "connected" ? "last known" : `${built.differPerLane.get(lane.id) ?? 0} differ`}
            </span>
          </span>
        ),
        width: "84px",
        minWidth: 84,
        cell: (row) => <ValueCell cell={row.cells.get(lane.id) ?? { state: "absent" }} lane={lane} row={row} compareLanes={compareLanes} offline={lane.agent !== "connected"} />,
      })),
    ],
    [lanes, expanded, toggle, built.differPerLane, compareLanes, focusFlag, focusSetting],
  );

  const subtitle = state.state === "loading" ? "Loading…" : state.state === "error" ? "Not loaded" : `${built.total} documented ${built.total === 1 ? "flag" : "flags"} across ${laneIds.length} ${laneIds.length === 1 ? "swimlane" : "swimlanes"} · ${built.differing} differ from the template default in at least one swimlane`;

  return (
    <div data-slot="status-page" className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex shrink-0 flex-wrap items-end justify-between gap-x-4 gap-y-3 pb-3">
        <div className="flex flex-col gap-1">
          <h1 className="m-0 text-xl leading-[26px] font-semibold tracking-tight">Feature status</h1>
          <span className="text-muted-foreground">{subtitle}</span>
        </div>
      </div>
      <div className="mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card">
        <div className="relative z-[3] flex min-h-10 shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b border-border bg-muted px-2 py-1">
          <SearchInput ref={filterRef} aria-label="Filter flags" placeholder="Filter flags" kbd="/" value={filter} onChange={(event) => setFilter(event.target.value)} boxClassName="w-60" className="font-mono text-xs" />
          <span className="inline-flex items-center gap-2 px-1.5">
            <Switch id={switchId} checked={onlyDiffering} onCheckedChange={setOnlyDiffering} />
            <Label htmlFor={switchId} className="font-normal">
              Only flags that differ
            </Label>
          </span>
          <span aria-hidden="true" className="h-5 w-px bg-border" />
          <Button type="button" variant="ghost" className="px-2 font-normal" onClick={() => setChosen(new Map(channelFlags.map((name) => [name, true])))}>
            <ChevronsUpDown aria-hidden="true" />
            Show channels
          </Button>
          <Button type="button" variant="ghost" className="px-2 font-normal" onClick={() => setChosen(new Map(channelFlags.map((name) => [name, false])))}>
            Hide channels
          </Button>
          <span className="ml-auto text-xs tabular-nums text-muted-foreground">{state.state === "ready" || state.state === "empty" ? `Showing ${built.shown} of ${built.total} flags` : ""}</span>
        </div>
        {offline.length > 0 && (state.state === "ready" || state.state === "empty") ? (
          <Banner
            variant="warning"
            title={offline.length === 1 ? `Agent for ${offline[0]?.name ?? ""} is offline` : `Agents for ${offline.map((l) => l.name).join(", ")} are offline`}
            action={
              offline.length === 1 && offline[0] ? (
                <AppLink route="swimlane" params={{ id: offline[0].id }} className="text-sm font-medium underline">
                  Check agent
                </AppLink>
              ) : (
                <AppLink route="swimlanes" className="text-sm font-medium underline">
                  Check agents
                </AppLink>
              )
            }
          >
            Showing the last known values for {offline.map((l) => l.name).join(", ")}.
          </Banner>
        ) : null}
        {focusSetting !== undefined && (state.state === "ready" || state.state === "empty") ? <FocusNote setting={focusSetting} lane={search.lane} entry={focusEntry} /> : null}
        <QueryBoundaryView
          state={state}
          noun="feature flags"
          source="feature flags"
          className="flex min-h-0 flex-1 flex-col"
          loading={<DataTable label="Feature flags" rows={[]} columns={columns} rowKey={(row) => row.id} density="compact" state="loading" className="min-h-0 flex-1" />}
          empty={
            <EmptyState icon={Zap} title="No feature flags found" className="pt-10">
              A flag appears here once a doc has a Feature Flags table that names a setting.
            </EmptyState>
          }
        >
          {() => (
            <>
              <DataTable
                label="Feature flags"
                rows={built.rows}
                columns={columns}
                rowKey={(row) => row.id}
                rowClassName={(row) => (row.kind === "channel" ? "bg-muted" : undefined)}
                density="compact"
                empty="No flag matches the filter."
                className="min-h-0 flex-1"
              />
              <div className="flex min-h-8 shrink-0 flex-wrap items-center gap-x-5 gap-y-1 border-t border-border bg-muted px-3 py-1 text-xs text-muted-foreground">
                <span className="inline-flex items-center gap-1.5">
                  <span className="inline-flex h-5 items-center gap-1 rounded-sm border border-warning-border bg-diff-mod-bg px-1.5 font-mono font-semibold text-diff-mod-fg">
                    <ArrowLeftRight aria-hidden="true" className="size-2.5" />
                    true
                  </span>
                  Differs from the template default
                </span>
                <span className="inline-flex items-center gap-1.5">
                  <span className="font-mono text-foreground">false</span>Same as the default
                </span>
                <span className="inline-flex items-center gap-1.5">
                  <span>—</span>Key not found
                </span>
                <span className="inline-flex items-center gap-1.5">
                  <AgentDot state="offline" />
                  Offline: last known values, dimmed
                </span>
                <span className="ml-auto">Click a value to compare it across swimlanes.</span>
              </div>
            </>
          )}
        </QueryBoundaryView>
      </div>
    </div>
  );
}

function FocusNote({ setting, lane, entry }: { setting: string; lane: string | undefined; entry: FlagStatus | undefined }) {
  const value = entry === undefined || lane === undefined ? undefined : entry.values.find((v) => v.swimlane === lane)?.value;
  return (
    <div role="note" className="flex min-h-10 shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b border-info-border bg-info-bg px-3 py-1.5 text-info-fg">
      {entry === undefined ? (
        <span>
          <SafeText text={setting} className="font-mono text-xs" /> is not a documented flag.
        </span>
      ) : (
        <span>
          <CopyValue value={setting} what="setting path" valueClassName="font-mono text-xs font-medium" />
          {lane === undefined ? null : (
            <>
              {" in "}
              <SafeText text={lane} className="font-mono text-xs font-medium" />
            </>
          )}
          {value === undefined ? null : value === null ? ": no such key" : isMaskedValue(value.text) ? <>{": "}<SafeText text={value.text} className="font-mono text-xs font-semibold" /></> : <>{": "}<CopyValue value={value.text} what="value" valueClassName="font-mono text-xs font-semibold" /></>}
          {entry.template_default === null ? null : <>{` (template default ${entry.template_default.text})`}</>}
        </span>
      )}
      <AppLink route="status" className="ml-auto text-sm font-medium underline">
        Show all flags
      </AppLink>
    </div>
  );
}
