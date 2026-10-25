import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { AgentRegistration as Registration } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { DestructiveConfirm } from "@/components/app/DestructiveConfirm";
import { GatedButton } from "@/components/app/GatedButton";
import { SafeText } from "@/components/app/SafeText";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { adminMutations } from "@/features/admin/mutations";
import { checkServiceAccount } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import { Trash2 } from "@/icons";

// The service account the agent of a swimlane runs as (Workload Identity, S5): the hub accepts an agent that proves it runs as this
// account, and no other. Registering or changing it names that in a dialog; removing it asks for the swimlane's name, because the
// agent can no longer connect. The registration comes from the Admin agents list, which the section already holds.
//
// The design's preview of the identity binding (project, namespace and Kubernetes account) is not drawn: the app knows only the
// account, not the names of the Kubernetes side (design/DEVIATIONS.md).

export type AgentKnowledge = { state: "loading" } | { state: "error" } | { state: "ready"; registration: Registration | undefined };

export function AgentAccount({ swimlane, known }: { swimlane: string; known: AgentKnowledge }) {
  const client = useQueryClient();
  const save = useMutation(adminMutations.putAgent(client));
  const remove = useMutation(adminMutations.deleteAgent(client));
  const registered = known.state === "ready" ? known.registration?.service_account_email : undefined;
  const [draft, setDraft] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [asking, setAsking] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const value = draft ?? registered ?? "";
  const verb = registered === undefined ? "Register agent" : "Save service account";

  const submit = (): void => {
    const trimmed = value.trim();
    const bad = checkServiceAccount(trimmed);
    if (bad !== null) {
      setProblem(bad);
      return;
    }
    if (trimmed === registered) {
      setProblem("That is the service account already registered");
      return;
    }
    setProblem(null);
    setAsking(trimmed);
  };

  return (
    <div className="flex flex-col gap-2">
      <h4 className="text-sm font-semibold">Agent</h4>
      {known.state === "loading" ? (
        <div role="status" aria-busy="true" aria-label="Loading the agent registration">
          <Skeleton className="h-7 w-96" />
        </div>
      ) : known.state === "error" ? (
        <p className="text-sm text-muted-foreground">The agent registrations could not be loaded, so this one is not shown. Retry the Agents list above.</p>
      ) : (
        <>
          {registered === undefined ? <p className="text-sm text-muted-foreground">No agent is registered for {swimlane}. Register the service account its agent runs as.</p> : null}
          <Field invalid={problem !== null}>
            <FieldLabel>Service account</FieldLabel>
            <div className="flex items-start gap-2">
              <div className="min-w-0 max-w-[520px] flex-1">
                <FieldControl>
                  <Input
                    value={value}
                    autoComplete="off"
                    spellCheck={false}
                    placeholder="lanekeeper-agent@project.iam.gserviceaccount.com"
                    className="font-mono"
                    onChange={(event) => {
                      setDraft(event.target.value);
                      setProblem(null);
                    }}
                  />
                </FieldControl>
              </div>
              <GatedButton
                minRole="admin"
                onClick={() => {
                  submit();
                }}
              >
                {verb}
              </GatedButton>
              {registered === undefined ? null : (
                <GatedButton
                  minRole="admin"
                  variant="secondary"
                  aria-label={`Remove registration of ${swimlane}`}
                  onClick={() => {
                    setRemoving(true);
                  }}
                >
                  <Trash2 data-icon="inline-start" />
                  Remove
                </GatedButton>
              )}
            </div>
            <FieldError>{problem}</FieldError>
            <FieldDescription>A Google service account. Agents appear in the list above after their first heartbeat.</FieldDescription>
          </Field>
        </>
      )}

      <ConsequenceDialog
        open={asking !== null}
        onOpenChange={(open) => {
          if (!open) {
            save.reset();
            setAsking(null);
          }
        }}
        title={registered === undefined ? `Register the agent of ${swimlane}` : `Change the agent of ${swimlane}`}
        consequence={`Only an agent that runs as ${asking ?? ""} can connect for ${swimlane}. An agent running as any other account is refused.`}
        actionLabel={verb}
        workingLabel="Saving…"
        working={save.isPending}
        error={save.error === null ? null : adminWriteError(save.error, "The agent could not be registered")}
        onConfirm={() => {
          if (asking === null) return;
          save.mutate(
            { swimlane, body: { service_account_email: asking } },
            {
              onSuccess: () => {
                setDraft(null);
                setAsking(null);
              },
            },
          );
        }}
      >
        <dl className="m-0 grid grid-cols-[104px_minmax(0,1fr)] gap-x-2 gap-y-1.5 rounded-md border border-info-border bg-info-bg px-3 py-2 text-sm">
          <dt className="text-xs text-muted-foreground">Agent runs as</dt>
          <dd className="m-0 font-mono text-xs break-all">
            <SafeText text={asking ?? ""} />
          </dd>
          <dt className="text-xs text-muted-foreground">Grant needed</dt>
          <dd className="m-0 text-xs">roles/iam.workloadIdentityUser on that service account</dd>
        </dl>
      </ConsequenceDialog>

      <DestructiveConfirm
        open={removing}
        onOpenChange={(open) => {
          if (!open) {
            remove.reset();
            setRemoving(false);
          }
        }}
        title={`Remove the agent registration of ${swimlane}`}
        consequence={`The agent of ${swimlane} can no longer connect to the hub until a service account is registered again. The files and history the hub already holds for it stay.`}
        confirmValue={swimlane}
        actionLabel="Remove registration"
        workingLabel="Removing…"
        working={remove.isPending}
        error={remove.error === null ? null : adminWriteError(remove.error, "The registration could not be removed")}
        onConfirm={() => {
          remove.mutate(
            { swimlane },
            {
              onSuccess: () => {
                setDraft(null);
                setRemoving(false);
              },
            },
          );
        }}
      />
    </div>
  );
}
