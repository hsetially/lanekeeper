import { useEffect, useMemo, useRef, useState } from "react";
import { Role, type ServiceState, type SwimlaneDetail } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { Banner } from "@/components/app/Banner";
import { EmptyState } from "@/components/app/EmptyState";
import { GatedButton } from "@/components/app/GatedButton";
import { SafeText } from "@/components/app/SafeText";
import { VirtualList } from "@/components/app/virtual/VirtualList";
import { SearchInput } from "@/components/ui/input";
import { AutoNotifyStatus } from "@/features/notify/AutoNotifyStatus";
import { ConfigServerRestartBanner } from "@/features/notify/ConfigServerRestartBanner";
import { useServiceWriteGate } from "@/features/notify/gates";
import { NotifyServicesButton } from "@/features/notify/NotifyServicesButton";
import { ServicePickup } from "@/features/pickup";
import { Server, RotateCw } from "@/icons";
import { useShortcut } from "@/keyboard";
import { linkTo } from "@/routes/links";
import { useNow } from "@/live/use-now";
import { useReplaceTo } from "@/routes/AppLink";
import { filterServices } from "./filter";
import { RestartDialog, type RestartOutcome } from "./RestartDialog";

// The Services tab: every service of the swimlane, whether it has picked up its config (D85: live, live by a time, needs notify or
// restart, needs a config-server restart), what it is waiting on, and a Restart button for an Operator. The contract has no pod
// count or start time for a service, so the design's "Pods ready" and "Started" columns are not here (design/DEVIATIONS.md).

const ROW = 52;
const COLUMNS = "grid-cols-[minmax(0,1.2fr)_minmax(0,2fr)_112px]";

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
  // Operator, live updates up, agent connected: the one gate of every button that acts on the services (src/features/notify/gates.ts).
  const gate = useServiceWriteGate(lane.agent);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {editorOnly ? (
        <Banner variant="info" title="Restart needs Operator">
          You can edit files here, but restarting services needs the Operator permission.
        </Banner>
      ) : null}
      <ConfigServerRestartBanner lane={lane} />
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
      <div data-slot="services-strip" className="flex min-h-8 flex-none flex-wrap items-center gap-x-2 gap-y-1 border-b border-border bg-muted px-4 py-1 text-xs text-muted-foreground">
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
        <div className="ml-auto flex items-center gap-2">
          <span className="tabular-nums">
            {rows.length === lane.services.length ? `${lane.services.length} ${lane.services.length === 1 ? "service" : "services"}` : `${rows.length} of ${lane.services.length} services`}
          </span>
          <NotifyServicesButton lane={lane} />
        </div>
      </div>
      <div className="flex-none border-b border-border px-4 py-1.5">
        <AutoNotifyStatus lane={lane} />
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
                  <ServicePickup service={service} now={now} />
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
