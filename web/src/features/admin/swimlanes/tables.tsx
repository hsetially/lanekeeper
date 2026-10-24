import type { AgentRegistration, SwimlaneSummary } from "@/api/generated/model";
import { AgentDot } from "@/components/app/AgentDot";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import { TierChip } from "@/components/app/TierBadge";
import type { Column } from "@/components/app/virtual/DataTable";
import { Button } from "@/components/ui/button";
import { AdminTable } from "@/features/admin/AdminTable";
import { TenantSet } from "@/features/shell/Tenants";
import { agentView } from "@/features/swimlanes/agent";
import { formatRelative, formatTime } from "@/lib/time";

// The two lists of the swimlanes section: the swimlanes the hub knows (from the list the whole app shares, so a live event updates
// it), and the agent registered for each (the Admin list, `agent_status_changed` keeps it fresh). Names, projects, accounts and
// replicas are text the hub was given, so each goes through SafeText.

/** A certificate that ends within this long is said to expire soon. */
const SOON_MS = 14 * 24 * 60 * 60 * 1000;

export function SwimlaneTable({ rows, selected, onManage }: { rows: readonly SwimlaneSummary[]; selected: string | null; onManage: (id: string, opener: Element) => void }) {
  const columns: Column<SwimlaneSummary>[] = [
    {
      id: "swimlane",
      header: "Swimlane",
      width: "minmax(0,1.2fr)",
      minWidth: 140,
      cell: (lane) => (
        <span className="flex min-w-0 items-baseline gap-2">
          <b className="truncate font-medium">
            <SafeText text={lane.display_name} />
          </b>
          {lane.display_name === lane.id ? null : <SafeText text={lane.id} className="truncate font-mono text-xs text-muted-foreground" />}
        </span>
      ),
    },
    { id: "project", header: "Project", width: "minmax(0,1fr)", minWidth: 120, cell: (lane) => (lane.project === null || lane.project === undefined ? <span className="text-muted-foreground">None</span> : <SafeText text={lane.project} className="font-mono text-xs" />) },
    { id: "tier", header: "Tier", width: "80px", minWidth: 80, cell: (lane) => (lane.tier === null || lane.tier === undefined ? <span className="text-muted-foreground">None</span> : <TierChip tier={lane.tier} />) },
    { id: "tenants", header: "Tenants", width: "minmax(0,1fr)", minWidth: 120, cell: (lane) => <TenantSet tenants={lane.tenants} /> },
    {
      id: "agent",
      header: "Agent",
      width: "112px",
      minWidth: 112,
      cell: (lane) => {
        const view = agentView(lane.agent);
        return (
          <span className="flex min-w-0 items-center gap-2">
            <AgentDot state={view.state} />
            <span className="min-w-0 truncate">{view.label}</span>
          </span>
        );
      },
    },
    {
      id: "manage",
      header: <span className="sr-only">Manage</span>,
      width: "112px",
      minWidth: 112,
      align: "right",
      cell: (lane) => (
        <Button
          size="sm"
          variant={selected === lane.id ? "default" : "secondary"}
          aria-label={`Manage ${lane.id}`}
          aria-pressed={selected === lane.id}
          onClick={(event) => {
            onManage(lane.id, event.currentTarget);
          }}
        >
          Manage
        </Button>
      ),
    },
  ];
  return <AdminTable label="Swimlanes" rows={rows} columns={columns} rowKey={(lane) => lane.id} />;
}

export function AgentsTable({ rows, now }: { rows: readonly AgentRegistration[]; now: number }) {
  const columns: Column<AgentRegistration>[] = [
    { id: "swimlane", header: "Swimlane", width: "112px", minWidth: 96, cell: (agent) => <SafeText text={agent.swimlane} className="font-mono text-xs font-medium" /> },
    {
      id: "account",
      header: "Service account",
      width: "minmax(0,1.6fr)",
      minWidth: 200,
      cell: (agent) => (
        <span title={visibleText(agent.service_account_email)} className="block truncate">
          <SafeText text={agent.service_account_email} className="font-mono text-xs" />
        </span>
      ),
    },
    {
      id: "status",
      header: "Status",
      width: "112px",
      minWidth: 112,
      cell: (agent) => {
        const view = agentView(agent.status);
        return (
          <span className="flex min-w-0 items-center gap-2">
            <AgentDot state={view.state} />
            <span className="min-w-0 truncate">{view.label}</span>
          </span>
        );
      },
    },
    { id: "replica", header: "Replica", width: "minmax(0,1fr)", minWidth: 120, cell: (agent) => (agent.connected_replica === null ? <span className="text-muted-foreground">None</span> : <SafeText text={agent.connected_replica} className="font-mono text-xs" />) },
    {
      id: "heartbeat",
      header: "Last heartbeat",
      width: "104px",
      minWidth: 104,
      cell: (agent) => (agent.last_seen_at === null ? <span className="text-muted-foreground">Never</span> : <span title={formatTime(agent.last_seen_at)}>{formatRelative(agent.last_seen_at, now)}</span>),
    },
    {
      id: "certificate",
      header: "Certificate expires",
      width: "minmax(0,1fr)",
      minWidth: 150,
      cell: (agent) => {
        const ends = agent.certificate_expires_at;
        if (ends === null) return <span className="text-muted-foreground">None yet</span>;
        const soon = ends - now < SOON_MS;
        return (
          <span title={formatTime(ends)} className="inline-flex items-center gap-1.5">
            <span className={soon ? "text-warning-fg" : "text-muted-foreground"}>{formatRelative(ends, now)}</span>
            {soon ? <span className="rounded-sm border border-warning-border bg-warning-bg px-1 text-xs text-warning-fg">expires soon</span> : null}
          </span>
        );
      },
    },
  ];
  return <AdminTable label="Agents" rows={rows} columns={columns} rowKey={(agent) => agent.swimlane} />;
}
