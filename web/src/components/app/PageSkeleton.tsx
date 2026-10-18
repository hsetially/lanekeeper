import { ListSkeleton } from "@/components/app/QueryBoundary";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * What a route shows while its screen loads: the page title's place, a line of facts, and rows at the table's height, so
 * that the screen's arrival moves nothing. A busy, named region (the blocks are hidden from assistive technology).
 */
export function PageSkeleton({ label = "Loading", rows = 10, columns = 4 }: { label?: string; rows?: number; columns?: number }) {
  return (
    <div role="status" aria-busy="true" aria-label={label} data-slot="page-skeleton" className="flex flex-col gap-3 px-6 pt-4">
      <Skeleton className="h-6 w-56" />
      <Skeleton className="h-4 w-80" />
      <div className="mt-2 overflow-hidden rounded-md border border-border">
        <Skeleton className="h-10 rounded-none" />
        <ListSkeleton rows={rows} columns={columns} />
      </div>
    </div>
  );
}
