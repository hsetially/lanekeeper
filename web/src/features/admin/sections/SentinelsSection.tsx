import type { SentinelInfo } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import type { Column } from "@/components/app/virtual/DataTable";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { badgeBase } from "@/components/ui/badge";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminQueries } from "@/features/admin/queries";
import { CircleHelp, Radio, TriangleAlert } from "@/icons";
import { formatRelative, formatTime } from "@/lib/time";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { cn } from "@/lib/utils";
import { useNow } from "@/live/use-now";
import { backlogText, countText, disconnected, sentinelView, type SentinelTone } from "./sentinels-model";

// Sentinels: the agent on each NFS VM that records who edits files (D72). The hub's API gives a list and nothing more: a host, a
// version, a status, the time it was last seen and the records still waiting on the VM. There is no operation to register, switch
// off or configure a sentinel, no swimlane or last-record time on the row (plan CG15) and no limit for "silent for too long", so this
// page is read only and the gap it names is the hub's own status (disconnected). The last-seen time is the sentinel's last hello or
// batch of records, not a heartbeat (D123), so it is called "Last seen". Every host, id and version is text the hub was given, so
// each goes through SafeText.

const TONE: Readonly<Record<SentinelTone, { Icon: typeof Radio; classes: string }>> = {
  ok: { Icon: Radio, classes: "border-status-sync-border bg-status-sync-bg text-status-sync-fg" },
  gap: { Icon: TriangleAlert, classes: "border-danger-border bg-danger-bg text-danger-fg" },
  unknown: { Icon: CircleHelp, classes: "border-status-unknown-border bg-status-unknown-bg text-status-unknown-fg" },
};

/** The status as an icon, a word and a colour, never a colour alone. */
function SentinelStatus({ status }: { status: SentinelInfo["status"] }) {
  const view = sentinelView(status);
  const { Icon, classes } = TONE[view.tone];
  return (
    // `max-w-full` and a truncating word: the badge is never wider than its cell, and a word that does not fit ends in an ellipsis
    // (the labels of another language can be 40% longer) instead of being cut off by the cell.
    <span data-slot="sentinel-status" data-tone={view.tone} className={cn(badgeBase, "max-w-full", classes)}>
      <Icon aria-hidden="true" />
      <span className="min-w-0 truncate">{view.label}</span>
    </span>
  );
}

const lastSeen = (row: SentinelInfo, now: number): string => (row.last_seen_at === null ? "never seen" : `last seen ${formatRelative(row.last_seen_at, now)}`);

function HostList({ rows, now }: { rows: readonly SentinelInfo[]; now: number }) {
  return (
    <>
      {rows.map((row, index) => (
        <span key={row.id}>
          {index === 0 ? "" : ", "}
          <SafeText text={row.host} className="font-mono text-xs" /> ({lastSeen(row, now)})
        </span>
      ))}
    </>
  );
}

function GapAlert({ rows, now }: { rows: readonly SentinelInfo[]; now: number }) {
  const gone = disconnected(rows);
  const first = gone[0];
  if (first === undefined) return null;
  const one = gone.length === 1;
  return (
    <Alert variant="danger">
      <AlertTitle>{one ? <><SafeText text={first.host} className="font-mono" /> is disconnected</> : `${gone.length} sentinels are disconnected`}</AlertTitle>
      <AlertDescription>
        {one ? `${lastSeen(first, now).replace(/^./, (c) => c.toUpperCase())}. ` : <><HostList rows={gone} now={now} />{". "}</>}
        {one ? "Edits made on this VM can't be attributed to a person until it connects again." : "Edits made on these VMs can't be attributed to a person until they connect again."}
      </AlertDescription>
    </Alert>
  );
}

export function SentinelsSection() {
  const pages = useCursorPages({ ...adminQueries.sentinels(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const now = useNow(true, 60_000);

  const columns: Column<SentinelInfo>[] = [
    {
      id: "host",
      header: "VM",
      width: "minmax(0,1.4fr)",
      minWidth: 160,
      cell: (row) => (
        <span className="flex min-w-0 items-baseline gap-2">
          <b className="truncate font-mono text-xs font-medium">
            <SafeText text={row.host} />
          </b>
          {row.id === row.host ? null : <SafeText text={row.id} className="truncate font-mono text-xs text-muted-foreground" />}
        </span>
      ),
    },
    { id: "version", header: "Version", width: "minmax(0,0.7fr)", minWidth: 90, cell: (row) => <SafeText text={row.version} className="font-mono text-xs" /> },
    {
      id: "last-seen",
      header: "Last seen",
      width: "minmax(0,0.9fr)",
      minWidth: 120,
      cell: (row) => {
        if (row.last_seen_at === null) return <span className="text-muted-foreground">Never</span>;
        const gap = sentinelView(row.status).tone === "gap";
        return (
          <span title={formatTime(row.last_seen_at)} {...(gap ? { "data-gap": "true" } : {})} className={gap ? "font-medium text-danger-fg" : undefined}>
            {formatRelative(row.last_seen_at, now)}
          </span>
        );
      },
    },
    {
      id: "backlog",
      header: "Spool backlog",
      width: "minmax(0,0.8fr)",
      minWidth: 110,
      cell: (row) => <span className={row.spool_backlog > 0 ? "tabular-nums" : "text-muted-foreground"}>{backlogText(row.spool_backlog)}</span>,
    },
    { id: "status", header: "Status", width: "minmax(0,1.1fr)", minWidth: 170, cell: (row) => <SentinelStatus status={row.status} /> },
  ];

  const count = state.state === "ready" ? countText(pages.items) : "";
  return (
    <div className="flex flex-col gap-3">
      {state.state === "ready" ? <GapAlert rows={pages.items} now={now} /> : null}
      <div className="flex min-h-7 items-center gap-2">
        <h3 className="text-sm font-semibold">Sentinels</h3>
        <span className="text-xs text-muted-foreground">{count}</span>
      </div>
      <QueryBoundaryView
        state={state}
        noun="sentinels"
        source="the sentinels"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={Radio} title="No sentinels registered">
              Until a sentinel reports from an NFS VM, edits made on it can't be attributed to a person.
            </EmptyState>
          </div>
        }
      >
        {(rows) => <AdminTable label="Sentinels" rows={rows} columns={columns} rowKey={(row) => row.id} />}
      </QueryBoundaryView>
      <p className="max-w-[680px] text-xs text-muted-foreground">
        A sentinel records who edits files on its NFS VM, and never reads or sends file content. Last seen is the time of its last hello or batch of records, so on a VM where nothing is written it can be old while the sentinel is connected. The spool backlog is the records it holds that the hub has not received yet. The hub's API shows the sentinels here and has no operation to register, switch off or configure a sentinel, so this list is read only.
      </p>
    </div>
  );
}
