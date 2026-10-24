import { useState } from "react";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { adminQueries } from "@/features/admin/queries";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { Layers, Server } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { useNow } from "@/live/use-now";
import type { AgentKnowledge } from "./AgentAccount";
import { SwimlaneSettings } from "./SwimlaneSettings";
import { AgentsTable, SwimlaneTable } from "./tables";

// Swimlanes and agents. The swimlanes are the list the whole app shares (a live event updates it); the agents are the Admin list of
// registrations (`agent_status_changed` keeps it fresh). Registering a swimlane is NOT here: the contract has no operation for it
// (CG15), so the page says so instead of showing a form that cannot work.

export function SwimlanesSection() {
  const lanes = useCursorPages({ ...swimlaneListQuery(), autoLoad: true });
  const laneState = useQueryState(cursorPagesSource(lanes), { isEmpty: (items) => items.length === 0 });
  const agents = useCursorPages({ ...adminQueries.agents(), autoLoad: true });
  const agentState = useQueryState(cursorPagesSource(agents), { isEmpty: (items) => items.length === 0 });
  const now = useNow(true, 60_000);
  const [selected, setSelected] = useState<string | null>(null);

  const offline = agents.items.filter((a) => a.status === "disconnected").length;
  const never = agents.items.filter((a) => a.status === "never_seen").length;
  const agentCount =
    agentState.state === "ready" ? `${agents.items.length} registered${offline > 0 ? ` · ${offline} offline` : ""}${never > 0 ? ` · ${never} never seen` : ""}` : "";
  const known: AgentKnowledge =
    agentState.state === "loading" ? { state: "loading" } : agentState.state === "error" ? { state: "error" } : { state: "ready", registration: agents.items.find((a) => a.swimlane === selected) };

  return (
    <div className="flex flex-col gap-6">
      <Alert variant="info">
        <AlertTitle>Registering a swimlane from this page is not available yet</AlertTitle>
        <AlertDescription>
          A swimlane's name, tier, project and cluster cannot be set here: the hub has no operation for it. The swimlanes below are the ones the hub knows. Choose Manage to set a swimlane's tenants, its auto-notify switch and its agent.
        </AlertDescription>
      </Alert>

      <section aria-label="Swimlanes" className="flex flex-col gap-2">
        <div className="flex min-h-6 items-center gap-2">
          <h3 className="text-sm font-semibold">Swimlanes</h3>
          <span className="text-xs text-muted-foreground">{laneState.state === "ready" ? `${lanes.items.length} registered` : ""}</span>
        </div>
        <QueryBoundaryView
          state={laneState}
          noun="swimlanes"
          source="the swimlane list"
          empty={
            <div className="rounded-md border border-border bg-card">
              <EmptyState icon={Layers} title="No swimlanes registered">
                Swimlanes can't be registered from this page yet. They appear here once the hub knows them.
              </EmptyState>
            </div>
          }
        >
          {(rows) => <SwimlaneTable rows={rows} selected={selected} onManage={(id) => setSelected(id)} />}
        </QueryBoundaryView>
      </section>

      <section aria-label="Agents" className="flex flex-col gap-2">
        <div className="flex min-h-6 items-center gap-2">
          <h3 className="text-sm font-semibold">Agents</h3>
          <span className="text-xs text-muted-foreground">{agentCount}</span>
        </div>
        <QueryBoundaryView
          state={agentState}
          noun="agents"
          source="the agent registrations"
          empty={
            <div className="rounded-md border border-border bg-card">
              <EmptyState icon={Server} title="No agents registered">
                An agent connects once the service account it runs as is registered for its swimlane. Choose Manage on a swimlane to register one.
              </EmptyState>
            </div>
          }
        >
          {(rows) => <AgentsTable rows={rows} now={now} />}
        </QueryBoundaryView>
      </section>

      {selected === null ? (
        <p className="text-sm text-muted-foreground">Choose Manage on a swimlane to set its tenants, its auto-notify switch and its agent.</p>
      ) : (
        <SwimlaneSettings key={selected} swimlane={selected} known={known} />
      )}
    </div>
  );
}
