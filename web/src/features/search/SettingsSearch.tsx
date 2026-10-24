import { useMemo, type RefObject } from "react";
import { searchSettings } from "@/api/generated/endpoints/search/search";
import type { SettingHit } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { TierChip } from "@/components/app/TierBadge";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { SearchInput } from "@/components/ui/input";
import { Columns2, Search } from "@/icons";
import { useQueryState } from "@/lib/query/use-query-state";
import { AppLink, useNavigateTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import { LanesFilter } from "./LanesFilter";
import type { LaneInfo } from "./lanes";
import { usePagedOneShot } from "./one-shot";
import { RequestId } from "@/components/app/RequestId";
import { fileParts, pluralise, termSegments } from "./results";
import { SuggestionList, type Suggestion } from "./Suggestions";
import type { Commit } from "./tabs";
import type { SearchParams } from "./url";
import { useDraft } from "./use-draft";

// The Settings tab: find a setting by a part of its path, in every swimlane at once (frames 1g to 1i). The server matches the
// PATH of a setting (`pg_trgm` on the path), never its value, and answers by keyset pages. Each row is a setting with the file
// it is in, its value and its line; the line opens the file, and "Open in grid" compares the file across swimlanes.

/** `searchSettings` has `q` at most 256 characters. */
const MAX_QUERY = 256;
const PAGE_SIZE = 100;
/** The most pages asked for: 1,000 settings. A list that stops there says so. */
const MAX_PAGES = 10;

type Props = {
  params: SearchParams;
  commit: Commit;
  lanes: readonly LaneInfo[];
  queryRef: RefObject<HTMLInputElement | null>;
};

const rowKey = (hit: SettingHit): string => `${hit.swimlane}\u0000${hit.file}\u0000${hit.path}\u0000${hit.location.lines.start}`;

function KindChip({ tenant }: { tenant: boolean }) {
  return (
    <span
      className={
        tenant
          ? "inline-flex h-[18px] shrink-0 items-center rounded-sm border border-info-border bg-info-bg px-1.5 text-xs font-medium text-info-fg"
          : "inline-flex h-[18px] shrink-0 items-center rounded-sm border border-border px-1.5 text-xs font-medium text-muted-foreground"
      }
    >
      {tenant ? "tenant" : "base"}
    </span>
  );
}

function PathCell({ path, query }: { path: string; query: string }) {
  return (
    <span className="font-mono text-xs">
      {termSegments(path, [query]).map((segment, i) =>
        segment.mark ? (
          <mark key={i} className="rounded-[2px] bg-selection px-0 font-semibold text-foreground shadow-[inset_0_0_0_1px_var(--info-border)]">
            <SafeText text={segment.text} />
          </mark>
        ) : (
          <SafeText key={i} text={segment.text} />
        ),
      )}
    </span>
  );
}

export function SettingsSearch({ params, commit, lanes, queryRef }: Props) {
  const goTo = useNavigateTo();
  const query = useDraft(params.q, (q) => commit({ q }, "replace"));
  const laneMap = useMemo(() => new Map(lanes.map((l) => [l.id, l])), [lanes]);
  const text = params.q.trim();
  const tooLong = params.q.length > MAX_QUERY;
  const noLanes = params.lanes !== null && params.lanes.length === 0;
  const asked = text !== "" && !tooLong && !noLanes;

  const paged = usePagedOneShot<{ items: SettingHit[]; next: string | null }, SettingHit>(
    asked
      ? {
          key: JSON.stringify(["settings", text, params.lanes]),
          fetch: (cursor, signal) =>
            searchSettings({ q: text, limit: PAGE_SIZE, ...(params.lanes !== null ? { swimlanes: params.lanes } : {}), ...(cursor === undefined ? {} : { cursor }) }, { signal }),
          select: (page) => page,
          maxPages: MAX_PAGES,
        }
      : null,
  );
  const state = useQueryState(paged.source, { isEmpty: (items) => items.length === 0 });
  const rows = useMemo(() => state.data ?? [], [state.data]);
  const swimlanes = useMemo(() => new Set(rows.map((r) => r.swimlane)).size, [rows]);
  const searched = params.lanes === null ? lanes.length : params.lanes.length;

  const columns = useMemo<Column<SettingHit>[]>(
    () => [
      {
        id: "lane",
        header: "Swimlane",
        width: "96px",
        cell: (hit) => {
          const info = laneMap.get(hit.swimlane);
          return (
            <span className="flex items-center gap-1.5">
              {info?.tier ? <TierChip tier={info.tier} /> : null}
              <SafeText text={hit.swimlane} className="truncate font-mono text-xs font-medium" />
            </span>
          );
        },
      },
      {
        id: "file",
        header: "File",
        width: "minmax(0,1.1fr)",
        cell: (hit) => (
          <span className="flex items-center gap-1.5" title={hit.file}>
            <SafeText text={fileParts(hit.file).name} className="truncate font-mono text-xs" />
            <KindChip tenant={hit.tenant !== null && hit.tenant !== undefined} />
          </span>
        ),
      },
      { id: "path", header: "Setting path", width: "minmax(0,1.6fr)", cell: (hit) => <PathCell path={hit.path} query={text} /> },
      {
        id: "value",
        header: "Value",
        width: "96px",
        cell: (hit) => (
          <span className="font-mono text-xs" title={hit.value.text}>
            <SafeText text={hit.value.text} />
          </span>
        ),
      },
      {
        id: "line",
        header: "Line",
        width: "56px",
        align: "right",
        cell: (hit) => (
          <AppLink
            route="files"
            params={{ id: hit.swimlane }}
            search={{ path: hit.file, line: String(hit.location.lines.start) }}
            aria-label={`Open ${fileParts(hit.file).name} at line ${hit.location.lines.start}`}
            className="font-mono text-xs tabular-nums"
          >
            {hit.location.lines.start}
          </AppLink>
        ),
      },
      {
        id: "grid",
        header: <span className="sr-only">Compare</span>,
        width: "148px",
        cell: (hit) =>
          hit.logical_file === null || hit.logical_file === undefined ? null : (
            <AppLink
              route="compare"
              search={{ file: hit.logical_file }}
              className="inline-flex h-[22px] items-center justify-center gap-1.5 rounded-sm border border-border bg-secondary px-2 text-xs font-medium whitespace-nowrap text-secondary-foreground no-underline hover:bg-secondary-hover hover:text-secondary-foreground"
            >
              <Columns2 aria-hidden="true" className="size-3" />
              Open in grid
            </AppLink>
          ),
      },
    ],
    [laneMap, text],
  );

  const open = (hit: SettingHit): void => {
    goTo(linkTo("files", { id: hit.swimlane }, { path: hit.file, line: String(hit.location.lines.start) }));
  };

  let summary: string;
  if (text === "") summary = "Type to search";
  else if (tooLong) summary = "Query not searched";
  else if (noLanes) summary = "No swimlanes selected";
  else if (state.state === "loading") summary = "Searching…";
  else if (state.state === "error") summary = "Search failed";
  else summary = `${paged.hasMore || paged.truncated ? `${rows.length}+` : rows.length.toLocaleString("en-US")} ${rows.length === 1 && !paged.hasMore ? "setting" : "settings"} in ${pluralise(swimlanes, "swimlane", "swimlanes")}`;

  const suggestions: Suggestion[] = [
    { id: "text", label: `Search file text for “${visibleText(text)}”`, note: "Finds comments and any other text", action: "Open Text", onApply: () => commit({ tab: "text" }, "push") },
    ...(params.lanes !== null && params.lanes.length > 0 && params.lanes.length < lanes.length
      ? [{ id: "lanes", label: `Search all ${lanes.length} swimlanes`, note: `Currently ${params.lanes.length}`, action: "Widen", onApply: () => commit({ lanes: null }, "push") }]
      : []),
  ];

  let body;
  if (text === "") {
    body = (
      <EmptyState icon={Search} title="Search settings" className="pt-10">
        Type a setting name, or a part of its path, to see it in every swimlane.
      </EmptyState>
    );
  } else if (tooLong) {
    body = (
      <EmptyState icon={Search} title="Query not searched" className="pt-10">
        A setting query is at most {MAX_QUERY} characters. Shorten it.
      </EmptyState>
    );
  } else if (noLanes) {
    body = (
      <EmptyState icon={Search} title="No swimlanes selected" className="pt-10">
        Pick at least one swimlane to search, or select all of them.
      </EmptyState>
    );
  } else {
    body = (
      <QueryBoundaryView
        state={state}
        noun="settings"
        source="the settings index"
        staleBanner={false}
        className="flex min-h-0 flex-1 flex-col"
        loading={<DataTable label="Settings" rows={[]} columns={columns} rowKey={rowKey} density="compact" state="loading" className="min-h-0 flex-1" />}
        empty={
          <div className="flex flex-col items-center gap-4 px-6 py-10">
            <EmptyState icon={Search} title={`No settings match “${visibleText(text)}”`} className="p-0">
              {`Searched ${searched} of ${lanes.length} swimlanes. Settings search matches setting paths, not values, comments or other text.`}
            </EmptyState>
            <SuggestionList items={suggestions} />
          </div>
        }
      >
        {(items) => (
          <>
            <DataTable
              label="Settings"
              rows={items}
              columns={columns}
              rowKey={rowKey}
              density="compact"
              onActivate={open}
              className="min-h-0 flex-1"
              footer={{ hasMore: paged.hasMore, onLoadMore: () => void paged.loadMore(), loading: paged.loadingMore, truncated: paged.truncated, step: PAGE_SIZE }}
            />
            {paged.loadMoreError ? (
              <Alert variant="danger" className="rounded-none border-x-0">
                <AlertTitle>{"Couldn't load more settings"}</AlertTitle>
                <AlertDescription className="flex flex-wrap items-center gap-2">
                  The {items.length.toLocaleString("en-US")} settings above are unchanged.
                  {paged.loadMoreError.requestId ? <RequestId id={paged.loadMoreError.requestId} /> : null}
                  <Button type="button" variant="secondary" size="sm" className="ml-auto" onClick={() => void paged.loadMore()}>
                    Try again
                  </Button>
                </AlertDescription>
              </Alert>
            ) : null}
          </>
        )}
      </QueryBoundaryView>
    );
  }

  return (
    <>
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted p-2">
        <SearchInput
          ref={queryRef}
          aria-label="Setting query"
          maxLength={MAX_QUERY}
          placeholder="Setting name or part of its path"
          kbd="/"
          value={query.value}
          onChange={(event) => query.set(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") query.flush();
          }}
          className="font-mono text-xs"
        />
        <LanesFilter lanes={lanes} selected={params.lanes} onChange={(next) => commit({ lanes: next }, "push")} counts={state.state === "ready" ? new Map(countBy(rows)) : undefined} />
      </div>
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-border px-3 text-xs text-muted-foreground">
        <strong aria-live="polite" className="font-semibold tabular-nums text-foreground">
          {summary}
        </strong>
        <span>Matches setting paths, ignoring case</span>
      </div>
      {body}
      {state.state === "ready" ? <div className="flex h-8 shrink-0 items-center border-t border-border bg-muted px-3 text-xs text-muted-foreground">Open in grid compares this setting across swimlanes.</div> : null}
    </>
  );
}

function countBy(rows: readonly SettingHit[]): Array<[string, number]> {
  const counts = new Map<string, number>();
  for (const row of rows) counts.set(row.swimlane, (counts.get(row.swimlane) ?? 0) + 1);
  return [...counts];
}
