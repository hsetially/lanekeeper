import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import type { AuditEvent } from "@/api/generated/model";
import { Banner } from "@/components/app/Banner";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { TruncationBanner } from "@/components/app/TruncationBanner";
import { VirtualList } from "@/components/app/virtual/VirtualList";
import { Button } from "@/components/ui/button";
import { auditListQuery } from "@/features/audit/queries";
import { swimlaneQuery } from "@/features/swimlanes/queries";
import { History, RefreshCw } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { formatRelative, formatTime } from "@/lib/time";
import { useNow } from "@/live/use-now";
import { actorLabel, newestFirst, viaLabel } from "./rows";

// The audit rows of one swimlane, newest first, for an Admin: the Activity tab (a feed) and the swimlane's Audit page (the same
// rows with the sequence number and the request ID). The contract has one audit read and it is Admin-only (W11), so this is
// the whole of "what happened here" until a viewer-scoped feed exists (CG9).
//
// No event names the audit trail, so it is not live: the swimlane's own events (a file seen, drift, findings, a sync window) are
// the signal that something happened, and when the swimlane has been refreshed since the rows were read the list says so and
// offers to read them again. Nothing polls.

export type Variant = "activity" | "audit";

const ROW = 52;

function Row({ event, variant, now }: { event: AuditEvent; variant: Variant; now: number }) {
  return (
    <div className="grid h-full grid-cols-[96px_64px_minmax(0,1fr)_auto] items-start gap-3 border-b border-border px-4 py-2">
      <span title={formatTime(event.at, { seconds: true })} className="pt-px text-xs whitespace-nowrap text-muted-foreground tabular-nums">
        {variant === "audit" ? `#${event.seq}` : formatRelative(event.at, now)}
      </span>
      <span className="pt-px text-xs text-muted-foreground">{viaLabel(event.via)}</span>
      <span className="flex min-w-0 flex-col gap-0.5">
        <span className="truncate">
          <SafeText text={event.action} className="font-mono text-xs font-medium" />
          {event.target !== null ? (
            <>
              {" "}
              <SafeText text={event.target} className="font-mono text-xs" />
            </>
          ) : null}
        </span>
        <span className="truncate text-xs text-muted-foreground" title={event.actor ?? undefined}>
          {actorLabel(event.actor)}
          {variant === "audit" ? ` · ${formatTime(event.at, { seconds: true })}` : ""}
        </span>
      </span>
      {variant === "audit" && event.request_id !== null ? <RequestId id={event.request_id} /> : <span className="text-xs text-muted-foreground">Audit</span>}
    </div>
  );
}

export function AuditList({ swimlane, variant }: { swimlane: string; variant: Variant }) {
  const now = useNow(true, 30_000);
  const pages = useCursorPages({ ...auditListQuery({ swimlane }), autoLoad: false });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  // The swimlane's own query is kept fresh by the live events. If it is newer than the rows, something probably happened since.
  const lane = useQuery(swimlaneQuery(swimlane));
  const behind = state.state === "ready" && lane.dataUpdatedAt > pages.dataUpdatedAt + 1000;
  const ordered = useMemo(() => newestFirst(pages.items), [pages.items]);

  return (
    <QueryBoundaryView
      state={state}
      noun="activity"
      source="the audit trail"
      className="flex min-h-0 flex-1 flex-col"
      empty={
        <EmptyState icon={History} title="No activity yet" className="flex-1">
          Nothing has been recorded for this swimlane.
        </EmptyState>
      }
    >
      {() => (
        <>
          {behind ? (
            <Banner
              variant="info"
              title="This swimlane has changed since these rows were read"
              action={
                <Button variant="secondary" size="sm" onClick={() => void pages.refetch()}>
                  <RefreshCw data-icon="inline-start" />
                  Refresh
                </Button>
              }
            />
          ) : null}
          {pages.truncated ? <TruncationBanner shown={ordered.length} noun="events" /> : null}
          <div className="flex h-8 flex-none items-center gap-3 border-b border-border bg-muted px-4 text-xs text-muted-foreground">
            <span>{variant === "activity" ? "Audited changes for this swimlane" : "Every audited event for this swimlane"}</span>
            <span className="ml-auto">Newest first</span>
          </div>
          <VirtualList
            label={variant === "activity" ? "Activity" : "Audit events"}
            role="list"
            className="flex-1"
            count={ordered.length}
            rowHeight={ROW}
            getRowKey={(i) => ordered[i]?.id ?? i}
            renderRow={(i) => {
              const event = ordered[i];
              return event === undefined ? null : <Row event={event} variant={variant} now={now} />;
            }}
          />
          {pages.hasMore ? (
            <div className="flex h-9 flex-none items-center gap-3 border-t border-border px-4 text-xs text-muted-foreground">
              <span>Showing {ordered.length}</span>
              <Button variant="secondary" size="sm" loading={pages.isFetching} onClick={() => void pages.loadMore()}>
                Load more
              </Button>
            </div>
          ) : null}
        </>
      )}
    </QueryBoundaryView>
  );
}
