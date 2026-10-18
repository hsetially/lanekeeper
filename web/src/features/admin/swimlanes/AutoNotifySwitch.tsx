import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId } from "react";
import { ErrorState } from "@/components/app/ErrorState";
import { WriteError } from "@/components/app/WriteError";
import { useWriteGate } from "@/components/app/write-gate";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { adminMutations } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { adminWriteError } from "@/features/admin/write-error";
import { isRetryableProblem } from "@/lib/query/use-query-state";

// The auto-notify switch of one swimlane: when on, the agent tells the config-server which paths changed after each write (D-notify;
// off by default). This is THE one write of the Admin area that is shown before the hub answers: the contract gives it no 202, no
// 409 and no expected version, it carries no secret, and it is a PUT of the whole value, so the answer is what was sent
// (src/lib/query/optimistic.ts, which also puts the old value back if the hub refuses and always reads the setting again after).

export function AutoNotifySwitch({ swimlane }: { swimlane: string }) {
  const client = useQueryClient();
  const query = useQuery(adminQueries.autoNotify(swimlane));
  const set = useMutation(adminMutations.setAutoNotify(client));
  const gate = useWriteGate("admin");
  const id = useId();
  const enabled = query.data?.enabled ?? false;

  return (
    <div className="flex flex-col gap-2">
      <h4 className="text-sm font-semibold">Auto-notify</h4>
      {query.isPending ? (
        <div role="status" aria-busy="true" aria-label="Loading auto-notify">
          <Skeleton className="h-5 w-64" />
        </div>
      ) : query.isError ? (
        <ErrorState title="Couldn't load the auto-notify setting" problem={query.error} {...(isRetryableProblem(query.error) ? { onRetry: () => void query.refetch() } : {})} />
      ) : (
        <div className="flex items-start gap-3">
          <Switch
            id={id}
            aria-label={`Auto-notify for ${swimlane}`}
            aria-describedby={`${id}-help`}
            checked={enabled}
            disabled={!gate.allowed || set.isPending}
            {...(gate.reason === null ? {} : { title: gate.reason })}
            onCheckedChange={(next) => {
              set.reset();
              set.mutate({ swimlane, body: { enabled: next } });
            }}
          />
          <p id={`${id}-help`} className="max-w-prose text-sm text-muted-foreground">
            {enabled ? "On: after each write, the agent tells the config-server which paths changed." : "Off: services pick up a change when someone notifies them or restarts them."}
          </p>
        </div>
      )}
      {set.error === null ? null : <WriteError {...adminWriteError(set.error, "The auto-notify setting could not be changed")} />}
    </div>
  );
}
