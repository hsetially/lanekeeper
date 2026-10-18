import { useEffect, useMemo, useRef, useState } from "react";
import { PickupState, Role, type ServiceState, type SwimlaneDetail } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { Banner } from "@/components/app/Banner";
import { EmptyState } from "@/components/app/EmptyState";
import { GatedButton } from "@/components/app/GatedButton";
import { SafeText } from "@/components/app/SafeText";
import { StatusBadge } from "@/components/app/StatusBadge";
import { useWriteGate, type GateResult } from "@/components/app/write-gate";
import { VirtualList } from "@/components/app/virtual/VirtualList";
import { SearchInput } from "@/components/ui/input";
import { Server, RotateCw } from "@/icons";
import { useShortcut } from "@/keyboard";
import { formatClock, formatRelative } from "@/lib/time";
import { linkTo } from "@/routes/links";
import { useNow } from "@/live/use-now";
import { agentView } from "../agent";
import { useReplaceTo } from "@/routes/AppLink";
import { pendingFilesSummary, pickupVariant } from "../pickup";
import { filterServices } from "./filter";
import { RestartDialog, type RestartOutcome } from "./RestartDialog";

// The Services tab: every service of the swimlane, whether it has picked up its config (D85: live, live by a time, needs notify or
// restart, needs a config-server restart), what it is waiting on, and a Restart button for an Operator. The contract has no pod
// count or start time for a service, so the design's "Pods ready" and "Started" columns are not here (design/DEVIATIONS.md).

const ROW = 52;
const COLUMNS = "grid-cols-[minmax(0,1.2fr)_minmax(0,2fr)_112px]";

function ConfigStatus({ service, now }: { service: ServiceState; now: number }) {
  const files = pendingFilesSummary(service);
  const waiting = service.pickup === PickupState.needs_notify_or_restart || service.pickup === PickupState.needs_config_server_restart;
  return (
    <span className="flex min-w-0 flex-col gap-0.5">
      <span className="flex min-w-0 items-center gap-2">
        {service.pickup === PickupState.live_within_ttl && service.live_by !== null ? (
          <StatusBadge variant="live-by" detail={formatClock(service.live_by)} />
        ) : (
          <StatusBadge variant={pickupVariant(service.pickup)} />
        )}
        {waiting && service.pending_since !== null ? (
          <span title={`since ${formatRelative(service.pending_since, now)}`} className="min-w-0 truncate text-xs text-muted-foreground">
            since {formatRelative(service.pending_since, now)}
          </span>
        ) : null}
      </span>
      {files.first !== null ? (
        <span title={files.all.join("\n")} className="truncate text-xs text-muted-foreground">
          Waiting on <SafeText text={files.first} className="font-mono" />
          {files.more > 0 ? ` and ${files.more} more` : ""}
        </span>
      ) : null}
    </span>
  );
}

export function ServicesTab({ lane, q }: { lane: SwimlaneDetail; q: string }) {
  const now = useNow(true, 30_000);
  const replace = useReplaceTo();
  const [text, setText] = useState(q);
  const written = useRef(q);
  useEffect(() => {
    if (q !== written.current) {
      written.current = q;
      setText(q);
    }
  }, [q]);
  useEffect(() => {
    if (text === written.current) return undefined;
    const timer = setTimeout(() => {
      written.current = text;
      replace(linkTo("services", { id: lane.id }, text === "" ? {} : { q: text }));
    }, 250);
    return () => {
      clearTimeout(timer);
    };
  }, [text, lane.id, replace]);

  const filterRef = useRef<HTMLInputElement>(null);
  useShortcut("local.filter.focus", () => {
    filterRef.current?.focus();
    filterRef.current?.select();
  });

  const rows = useMemo(() => filterServices(lane.services, text), [lane.services, text]);
  const [target, setTarget] = useState<ServiceState | null>(null);
  const [result, setResult] = useState<{ service: ServiceState; outcome: RestartOutcome } | null>(null);

  const session = useSession();
  const editorOnly = session.status === "ready" && session.role === Role.editor;
  const operator = useWriteGate("operator");
  const agentUp = agentView(lane.agent).connected;
  const gate: GateResult = !operator.allowed ? operator : agentUp ? operator : { allowed: false, reason: "Unavailable while the agent is offline" };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {editorOnly ? (
        <Banner variant="info" title="Restart needs Operator">
          You can edit files here, but restarting services needs the Operator permission.
        </Banner>
      ) : null}
      {result ? (
        result.outcome.kind === "applied" ? (
          <Banner
            variant="success"
            title={`Restart requested for ${result.service.service.name}`}
            onDismiss={() => {
              setResult(null);
            }}
          >
            The hub accepted it. Restart progress is not shown here; the config status changes when the agent reports it.
          </Banner>
        ) : (
          <Banner variant="info" title={`Restart of ${result.service.service.name} sent for approval`}>
            Request AP-{result.outcome.request}. Nothing restarts until an Admin approves.
          </Banner>
        )
      ) : null}
      <div className="flex h-8 flex-none items-center gap-2 border-b border-border bg-muted px-4 text-xs text-muted-foreground">
        <SearchInput
          ref={filterRef}
          aria-label="Filter services"
          placeholder="Filter services…"
          kbd="/"
          boxClassName="h-6 w-56 flex-none"
          value={text}
          onChange={(event) => {
            setText(event.target.value);
          }}
        />
        <span className="ml-auto tabular-nums">
          {rows.length === lane.services.length ? `${lane.services.length} ${lane.services.length === 1 ? "service" : "services"}` : `${rows.length} of ${lane.services.length} services`}
        </span>
      </div>
      {lane.services.length === 0 ? (
        <EmptyState icon={Server} title="No services reported" className="flex-1">
          The <SafeText text={lane.display_name} className="font-mono" /> agent has not reported any services yet. Check that it is running in its namespace.
        </EmptyState>
      ) : rows.length === 0 ? (
        <EmptyState title="No service matches" className="flex-1">
          Nothing is left with “<SafeText text={text} />”.
        </EmptyState>
      ) : (
        <VirtualList
          label="Services"
          role="table"
          className="flex-1"
          count={rows.length}
          rowHeight={ROW}
          header={
            <div role="row" className={`grid ${COLUMNS} h-8 items-center gap-3 border-b border-border px-4 text-xs font-medium text-muted-foreground`}>
              <span role="columnheader">Service</span>
              <span role="columnheader">Config status</span>
              <span role="columnheader" className="text-right">
                Action
              </span>
            </div>
          }
          headerHeight={32}
          getRowKey={(i) => {
            const service = rows[i];
            return service === undefined ? i : `${service.service.namespace}/${service.service.name}`;
          }}
          renderRow={(i) => {
            const service = rows[i];
            if (service === undefined) return null;
            return (
              <div className={`grid h-full ${COLUMNS} items-center gap-3 border-b border-border px-4`}>
                <span role="cell" className="min-w-0 truncate font-mono text-xs font-medium">
                  <SafeText text={service.service.name} />
                </span>
                <span role="cell" className="min-w-0">
                  <ConfigStatus service={service} now={now} />
                </span>
                <span role="cell" className="flex justify-end">
                  <GatedButton
                    gate={gate}
                    variant="secondary"
                    size="sm"
                    aria-label={`Restart ${service.service.name}`}
                    onClick={() => {
                      setTarget(service);
                    }}
                  >
                    <RotateCw data-icon="inline-start" />
                    Restart
                  </GatedButton>
                </span>
              </div>
            );
          }}
        />
      )}
      <p className="flex-none px-4 py-2.5 text-xs text-muted-foreground">
        Restart needs Operator or above. A file that applies to all services, like application.properties, shows on every service that has not picked it up yet.
      </p>
      <RestartDialog
        lane={lane}
        service={target}
        onClose={() => {
          setTarget(null);
        }}
        onDone={(service, outcome) => {
          setResult({ service, outcome });
        }}
      />
    </div>
  );
}
