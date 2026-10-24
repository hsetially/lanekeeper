// TEST PAGES (perf build only): the route-change pages of P10. A layout with links, and three screens that are chunks of
// their own, each a real list of real data (the swimlanes, from the generated hook, cached) in a DataTable. A route change
// between them is the thing the budget names: a click or a key, to the new route's h1 painted, with the chunk and the data
// already there.
import { useQueryClient } from "@tanstack/react-query";
import { Link, Outlet } from "@tanstack/react-router";
import { useEffect, useMemo } from "react";
import { apiCaches } from "@/api/client";
import type { SwimlaneSummary } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { DataTable, type Column } from "@/components/app/virtual/DataTable";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { compareKeys } from "@/lib/query/keys";

export function PerfNavLayout() {
  const queryClient = useQueryClient();
  // For the spec: what a diff costs when its data is not in memory. It drops the compare answers from the query cache, and
  // the ETag cache with them (or the answer would be revalidated, which is measured separately).
  useEffect(() => {
    window.__perfControl = {
      dropCompare: (alsoEtag: boolean) => {
        queryClient.removeQueries({ queryKey: compareKeys.diffs() });
        if (alsoEtag) apiCaches.etags.clear();
      },
    };
    return () => {
      delete window.__perfControl;
    };
  }, [queryClient]);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <nav aria-label="Pages" className="flex gap-3 border-b border-border p-3">
        {(["a", "b", "c"] as const).map((id) => (
          <Link key={id} to={`/testpages/perf/nav/${id}`} preload="intent" className="underline">
            Page {id.toUpperCase()}
          </Link>
        ))}
        <Link to="/testpages/perf/diff" preload="intent" className="underline">
          Open the diff
        </Link>
        <Link to="/testpages/perf/grid" preload="intent" className="underline">
          Open the grid
        </Link>
      </nav>
      <Outlet />
    </div>
  );
}

export function PerfScreen({ title }: { title: string }) {
  const pages = useCursorPages({ ...swimlaneListQuery(), staleTime: 60_000 });
  const columns = useMemo<Column<SwimlaneSummary>[]>(
    () => [
      { id: "name", header: "Swimlane", width: "minmax(0,1fr)", cell: (row) => <SafeText text={row.display_name} /> },
      { id: "tenants", header: "Tenants", width: "160px", cell: (row) => <SafeText text={row.tenants.join(", ")} /> },
      { id: "findings", header: "Findings", width: "80px", align: "right", cell: (row) => String(row.finding_count) },
    ],
    [],
  );
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <h1 className="p-3 text-lg font-semibold">{title}</h1>
      <DataTable label={title} rows={pages.items} columns={columns} rowKey={(row) => row.id} state={pages.status === "pending" ? "loading" : pages.status === "error" ? "error" : "ready"} className="min-h-0 flex-1" />
    </div>
  );
}
