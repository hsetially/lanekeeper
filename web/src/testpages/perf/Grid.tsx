// TEST PAGE (perf build only): the 5,000-row comparison grid of P10 through the real paging path. 40 swimlane columns, the
// rows loaded by the generated `getGrid` through `useCursorPages` (a keyset cursor, 500 a page), drawn by DataTable.
import { Link } from "@tanstack/react-router";
import { useEffect, useMemo } from "react";
import { getGrid } from "@/api/generated/endpoints/compare/compare";
import type { GridRow } from "@/api/generated/model";
import { SafeCell } from "@/components/app/virtual/SafeCell";
import { SafeText } from "@/components/app/SafeText";
import { useEditorWarmup } from "@/editor/warm";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { compareKeys } from "@/lib/query/keys";
import { resetLab, setLab } from "./lab";

const FILE = "services/payments/application.yml";
const LANES = Array.from({ length: 40 }, (_, i) => `swimlane-${String(i + 1).padStart(2, "0")}`);

// A page of the grid is large: 500 rows by 40 swimlanes is 2.7 MB of JSON in this contract, and the hub sends it without
// Content-Encoding (W28). The first request asks for few rows, so that the first screen is drawn once they arrive and not once
// 2.7 MB has; the pages after it are the full 500. (Lighthouse's LCP of this page was 3.2 s with a full first page.)
const FIRST_PAGE = 50;
const PAGE = 500;

const select = (page: Awaited<ReturnType<typeof getGrid>>) => ({ items: page.rows.items, next: page.rows.next });

export function PerfGrid() {
  const pages = useCursorPages({
    queryKey: compareKeys.grid(FILE, LANES, false, null),
    fetchPage: (cursor, signal) => getGrid({ file: FILE, swimlanes: LANES, limit: cursor === undefined ? FIRST_PAGE : PAGE, ...(cursor === undefined ? {} : { cursor }) }, { signal }),
    select,
    autoLoad: true,
  });

  const columns = useMemo<Column<GridRow>[]>(
    () => [
      { id: "path", header: "Setting", width: "320px", minWidth: 320, cell: (row) => <SafeText text={row.path} /> },
      ...LANES.map((lane, i) => ({
        id: lane,
        header: lane,
        width: "120px",
        minWidth: 120,
        cell: (row: GridRow) => {
          const value = row.cells[i]?.value;
          return value ? <SafeCell text={value.text} /> : "-";
        },
      })),
    ],
    [],
  );

  const done = pages.status === "success" && !pages.hasMore && !pages.isFetching;
  // As the file view will: warm the editor for an Editor once the page can be used.
  useEditorWarmup(pages.status === "success");
  useEffect(() => {
    resetLab();
    return resetLab;
  }, []);
  useEffect(() => {
    setLab({ phase: done ? "ready" : "loading", rows: pages.items.length });
  }, [done, pages.items.length]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <h1 className="p-3 text-lg font-semibold">Grid</h1>
      <Link to="/testpages/perf/nav/a" className="px-3 pb-2 text-sm underline">
        Back to the pages
      </Link>
      <DataTable
        data-testid="grid-scroller"
        label="Comparison grid"
        rows={pages.items}
        columns={columns}
        rowKey={(row) => row.path}
        state={pages.status === "pending" ? "loading" : pages.status === "error" ? "error" : "ready"}
        className="min-h-0 flex-1"
        footer={{ hasMore: pages.hasMore, loading: pages.isFetching, truncated: pages.truncated, onLoadMore: () => void pages.loadMore() }}
      />
    </div>
  );
}
