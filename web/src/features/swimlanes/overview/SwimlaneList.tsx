import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Role, type SwimlaneSummary } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { STATUS_VARIANTS } from "@/components/app/StatusBadge";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { DataTable, type Column, type SortState } from "@/components/app/virtual/DataTable";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { Layers, Plus, RefreshCw } from "@/icons";
import { prefs } from "@/lib/prefs";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState, type QueryState } from "@/lib/query/use-query-state";
import { cn } from "@/lib/utils";
import { useShortcut } from "@/keyboard";
import { useNow } from "@/live/use-now";
import { useStableList } from "@/live/stable-list";
import { AppLink, openInNewTab, useNavigateTo, useReplaceTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import type { SearchOf } from "@/routes/props";
import { useRecentlyChanged } from "../use-recently-changed";
import { AgentCell, BaselineCell, BranchCell, DriftCell, FindingsCell, LastScanCell, PickupCell, ProjectCell, SwimlaneCell, VersionCell } from "./cells";
import { COLUMN_LABEL, visibleColumns, type ColumnId } from "./columns";
import { facts, filterLanes, sortLanes, tiersOf, type AgentFilter } from "./lanes";
import { Toolbar } from "./Toolbar";

// The swimlane list (Shell, page "swimlanes"): the home page. Every swimlane the person may see, one row each, with its tier,
// tenants, agent, drift, pickup, findings, baseline and last scan. The whole list is loaded (it is a few hundred rows at the
// most), filtered and sorted here, and kept fresh by the live events; a row that changes in place updates in place, and a row
// that arrives or goes is counted in a chip until the person asks for it, so the list never moves under their pointer.

const keyOfLane = (lane: SwimlaneSummary): string => lane.id;
const DEFAULT_SORT: SortState = { column: "swimlane", direction: "asc" };
const TEXT_DEBOUNCE_MS = 250;

/** What a row shows, as one string: when it differs between two lists the row changed in place. */
const signatureOf = (lane: SwimlaneSummary): string =>
  JSON.stringify([lane.agent, lane.last_scan_at, lane.base_version, lane.drift_counts, lane.finding_count, lane.tier, lane.project, lane.tenants, lane.baseline, lane.pickup_counts, lane.syncing]);

const LEGEND = [
  ["conflict", "text-status-conflict-fg"],
  ["nfs", "text-status-nfs-fg"],
  ["git", "text-status-git-fg"],
  ["unknown", "text-status-unknown-fg"],
  ["needs-notify-or-restart", "text-status-restart-fg"],
  ["needs-config-server-restart", "text-status-restart-fg"],
] as const;

type Filters = { q: string; tier: string | null; drift: boolean };

function addressOf({ q, tier, drift }: Filters): string {
  return linkTo("swimlanes", undefined, { ...(q === "" ? {} : { q }), ...(tier === null ? {} : { tier }), ...(drift ? { state: "drift" } : {}) });
}

export function SwimlaneList({ search }: { search: SearchOf<"swimlanes"> }) {
  const pages = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const query = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  // The stable list keeps the first list it is given, and a list that is still loading (or empty) must not be that list: the
  // screen starts over when the first swimlanes arrive. Its filters start from the address, so nothing the person chose is lost.
  const phase = query.data === undefined ? "pending" : query.data.length === 0 ? "empty" : "list";
  return <Screen key={phase} search={search} query={query} truncated={pages.truncated} />;
}

function Screen({ search, query, truncated }: { search: SearchOf<"swimlanes">; query: QueryState<SwimlaneSummary[]>; truncated: boolean }) {
  const session = useSession();
  const role = session.status === "ready" ? session.role : null;
  const isAdmin = roleAtLeast(role, Role.admin);
  const navigate = useNavigateTo();
  const replace = useReplaceTo();
  const now = useNow(true, 30_000);

  const all = useMemo<SwimlaneSummary[]>(() => query.data ?? [], [query.data]);
  const stable = useStableList(all, keyOfLane);
  const changed = useRecentlyChanged(stable.shown, keyOfLane, signatureOf);

  // The filters. Text, tier and Has drift live in the address (so a link keeps them); the agent filter is for this visit.
  const urlText = search.q ?? "";
  const tier = search.tier ?? null;
  const hasDrift = search.state === "drift";
  const [text, setText] = useState(urlText);
  const [agent, setAgent] = useState<AgentFilter | null>(null);
  const written = useRef(urlText);
  const latest = useRef<Filters>({ q: urlText, tier, drift: hasDrift });
  latest.current = { q: text, tier, drift: hasDrift };

  // The address changed by itself (Back, a link): take its text. Our own writes set `written` first, so they do not come back.
  useEffect(() => {
    if (urlText !== written.current) {
      written.current = urlText;
      setText(urlText);
    }
  }, [urlText]);
  // What the person types goes to the address a moment after they stop, replacing the entry: Back leaves the screen.
  useEffect(() => {
    if (text === written.current) return undefined;
    const timer = setTimeout(() => {
      written.current = text;
      replace(addressOf({ ...latest.current, q: text }));
    }, TEXT_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [text, replace]);
  const change = (next: Partial<Filters>) => {
    written.current = next.q ?? latest.current.q;
    replace(addressOf({ ...latest.current, ...next }));
  };

  const [sort, setSort] = useState<SortState>(DEFAULT_SORT);
  const [shown, setShown] = useState<ColumnId[]>(() => visibleColumns(prefs.get("tableColumns", "swimlanes")));
  const chooseColumn = (id: ColumnId, on: boolean) => {
    setShown((current) => {
      const next = visibleColumns(on ? [...current, id] : current.filter((c) => c !== id));
      prefs.set("tableColumns", "swimlanes", next);
      return next;
    });
  };

  const rows = useMemo(() => sortLanes(filterLanes(stable.shown, { q: text, tier, hasDrift, agent }), sort), [stable.shown, text, tier, hasDrift, agent, sort]);
  const tiers = useMemo(() => tiersOf(all), [all]);
  const filtered = text.trim() !== "" || tier !== null || hasDrift || agent !== null;
  const clearFilters = () => {
    setText("");
    setAgent(null);
    change({ q: "", tier: null, drift: false });
  };

  // The selection: by id, and only of rows that are on screen.
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    const ids = new Set(rows.map(keyOfLane));
    setSelected((current) => {
      const kept = [...current].filter((id) => ids.has(id));
      return kept.length === current.size ? current : new Set(kept);
    });
  }, [rows]);
  const selectedIds = useMemo(() => rows.filter((lane) => selected.has(lane.id)).map(keyOfLane), [rows, selected]);
  // The row chosen last is where a range starts (the keymap's "Select row; Shift extends the range").
  const anchor = useRef<string | null>(null);
  const toggle = useCallback((id: string) => {
    anchor.current = id;
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }, []);
  // Shift and Space or x: every row from the anchor to this one, in the order of the list, is selected; none is taken out. The
  // anchor stays, so the range can be moved. With no anchor on screen it is a plain choice of this row.
  const extend = (id: string) => {
    const ids = rows.map(keyOfLane);
    const from = anchor.current === null ? -1 : ids.indexOf(anchor.current);
    const to = ids.indexOf(id);
    if (from < 0 || to < 0) {
      toggle(id);
      return;
    }
    const range = ids.slice(Math.min(from, to), Math.max(from, to) + 1);
    setSelected((current) => new Set([...current, ...range]));
  };

  const filterRef = useRef<HTMLInputElement>(null);
  const tableBox = useRef<HTMLDivElement>(null);
  useShortcut("local.filter.focus", () => {
    filterRef.current?.focus();
    filterRef.current?.select();
  });
  useShortcut("lists-tables-trees.compare-the-selected-rows-row", () => {
    if (selectedIds.length < 2) return false;
    navigate(linkTo("compare", undefined, { lanes: selectedIds.join(",") }));
    return undefined;
  });
  // The arrow keys from anywhere on the page start moving through the list; once the list has focus it moves itself, and Enter
  // (which it handles itself too) opens the swimlane under the cursor.
  useShortcut("swimlanes.move-and-open-a-swimlane", (hit) => {
    const grid = tableBox.current?.querySelector<HTMLElement>('[role="grid"]');
    if (!grid || hit.binding > 1 || grid.contains(document.activeElement)) return false;
    grid.focus();
    grid.dispatchEvent(new KeyboardEvent("keydown", { key: hit.event.key, bubbles: true, cancelable: true }));
    return undefined;
  });

  const columns = useMemo<Column<SwimlaneSummary>[]>(() => {
    // The widths are the design's, a little wider where the cell has more to say (a set of tenants, a time), and each is also the
    // least the column keeps: a window too narrow for all of them scrolls the table sideways, never the page.
    const cell: Record<Exclude<ColumnId, "swimlane">, Omit<Column<SwimlaneSummary>, "id" | "header">> = {
      project: { width: "84px", minWidth: 84, cell: (lane) => <ProjectCell lane={lane} /> },
      branch: { width: "72px", minWidth: 72, cell: (lane) => <BranchCell lane={lane} /> },
      base: { width: "88px", minWidth: 88, cell: (lane) => <VersionCell lane={lane} /> },
      agent: { width: "136px", minWidth: 136, cell: (lane) => <AgentCell lane={lane} now={now} /> },
      drift: { width: "minmax(0,1fr)", minWidth: 120, sortable: true, cell: (lane) => <DriftCell lane={lane} /> },
      pickup: { width: "88px", minWidth: 88, align: "right", cell: (lane) => <PickupCell lane={lane} /> },
      findings: { width: "52px", minWidth: 52, align: "right", sortable: true, cell: (lane) => <FindingsCell lane={lane} /> },
      baseline: { width: "208px", minWidth: 208, cell: (lane) => <BaselineCell lane={lane} isAdmin={isAdmin} /> },
      last: { width: "76px", minWidth: 76, sortable: true, cell: (lane) => <LastScanCell lane={lane} now={now} changed={changed.has(lane.id)} /> },
    };
    const picker: Column<SwimlaneSummary> = {
      id: "select",
      header: <span className="sr-only">Select</span>,
      width: "28px",
      minWidth: 28,
      cell: (lane) => (
        <Checkbox
          checked={selected.has(lane.id)}
          aria-label={`Select ${lane.display_name}`}
          onCheckedChange={() => {
            toggle(lane.id);
          }}
        />
      ),
    };
    const swimlane: Column<SwimlaneSummary> = { id: "swimlane", header: COLUMN_LABEL.swimlane, width: "132px", minWidth: 132, sortable: true, cell: (lane) => <SwimlaneCell lane={lane} /> };
    const rest = shown
      .filter((id): id is Exclude<ColumnId, "swimlane"> => id !== "swimlane")
      .map((id): Column<SwimlaneSummary> => ({ id, header: COLUMN_LABEL[id], ...cell[id] }));
    return [picker, swimlane, ...rest];
  }, [shown, now, changed, isAdmin, selected, toggle]);

  const addSwimlane = (variant: "default" | "secondary") =>
    isAdmin ? (
      <Button asChild variant={variant}>
        <AppLink route="admin-swimlanes">
          <Plus data-icon="inline-start" />
          Add a swimlane
        </AppLink>
      </Button>
    ) : null;
  // Only a list that is not here (loading, or failed) dims the controls; an empty one has the filters as they always are.
  const dim = query.state === "loading" || query.state === "error";
  const line = query.state === "loading" ? "Loading…" : query.state === "empty" ? "No agents registered" : query.state === "error" ? "Error" : facts(all);
  const selectedKeys = selected;

  return (
    <div className="flex min-h-0 flex-1 flex-col px-6 pt-4">
      <div className="flex flex-none flex-wrap items-end justify-between gap-x-4 gap-y-3 pb-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl leading-[26px] font-semibold tracking-[-0.01em]">Swimlanes</h1>
          <span className="text-muted-foreground">{line}</span>
        </div>
        {addSwimlane("secondary")}
      </div>
      {truncated ? <TruncationBanner shown={all.length} noun="swimlanes" className="mb-2 rounded-md border" /> : null}
      <div className="mb-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card" aria-busy={query.state === "loading" || undefined}>
        <Toolbar
          disabled={dim}
          inputRef={filterRef}
          text={text}
          onText={setText}
          tiers={tiers}
          tier={tier}
          onTier={(next) => {
            change({ tier: next });
          }}
          agent={agent}
          onAgent={setAgent}
          hasDrift={hasDrift}
          onHasDrift={(on) => {
            change({ drift: on });
          }}
          columns={shown}
          onColumn={chooseColumn}
          selected={selectedIds}
        />
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
        <div ref={tableBox} className="flex min-h-0 flex-1 flex-col">
          <QueryBoundaryView
            state={query}
            noun="swimlanes"
            source="agent state"
            className="flex min-h-0 flex-1 flex-col"
            loading={<DataTable label="Swimlanes" state="loading" density="compact" rows={all} columns={columns} rowKey={keyOfLane} />}
            empty={
              <EmptyState icon={Layers} title="No swimlanes registered yet" className="flex-1" action={isAdmin ? addSwimlane("default") : <span className="text-sm text-muted-foreground">Ask an admin to add one.</span>}>
                A swimlane appears here once an agent registers for its Kubernetes namespace.
              </EmptyState>
            }
          >
            {() => (
              <DataTable
                label="Swimlanes"
                density="compact"
                className="flex-1"
                rows={rows}
                columns={columns}
                rowKey={keyOfLane}
                sort={sort}
                onSortChange={(next) => {
                  setSort(next ?? DEFAULT_SORT);
                }}
                selectedKeys={selectedKeys}
                onToggleSelect={(lane, _index, how) => {
                  if (how.extend) extend(lane.id);
                  else toggle(lane.id);
                }}
                onActivate={(lane) => {
                  navigate(linkTo("swimlane", { id: lane.id }));
                }}
                onOpenInNewTab={(lane) => {
                  openInNewTab(linkTo("swimlane", { id: lane.id }));
                }}
                // The design's highlight of a swimlane that just changed (Shell, page "swimlanes"): the info background and a 1 px info border.
                rowClassName={(lane) => (changed.has(lane.id) ? "bg-info-bg shadow-[inset_0_0_0_1px_var(--info-border)]" : undefined)}
                empty={
                  filtered ? (
                    <EmptyState title="No swimlane matches" action={<Button variant="secondary" onClick={clearFilters}>Clear filters</Button>}>
                      Nothing is left with these filters.
                    </EmptyState>
                  ) : undefined
                }
              />
            )}
          </QueryBoundaryView>
        </div>
        {query.state === "ready" ? (
          <div className="flex min-h-7 flex-none flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-muted px-3 text-xs text-muted-foreground">
            <span className="tabular-nums">
              Showing {rows.length} of {all.length}
            </span>
            {LEGEND.map(([variant, color]) => {
              const spec = STATUS_VARIANTS[variant];
              return (
                <span key={variant} className="inline-flex items-center gap-1">
                  <spec.Icon aria-hidden="true" className={cn("size-3", color)} />
                  {spec.label}
                </span>
              );
            })}
            <span className="ml-auto">Counts are files per state. Hover a badge for its name.</span>
          </div>
        ) : null}
      </div>
    </div>
  );
}
