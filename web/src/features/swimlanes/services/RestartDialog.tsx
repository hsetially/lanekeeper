import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { Role, type ServiceState, type SwimlaneDetail, type WriteResult } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { ProblemDialog } from "@/components/app/ProblemDialog";
import { SafeText } from "@/components/app/SafeText";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { useServiceWriteGate } from "@/features/notify/gates";
import { User } from "@/icons";
import { AppLink } from "@/routes/AppLink";
import { swimlaneMutations } from "../mutations";
import { pendingFilesSummary } from "../pickup";
import { restartFailure } from "./write-error";

// Restart one service. A consequence dialog, as the design draws it (Consequence Dialogs, frame 4a): what will happen first, the
// primary button names the action, focus starts on Cancel, a running write locks the dialog, and a refusal stays inside it
// with its request ID. Nothing in the page changes until the server has answered (a restart is never optimistic), and an
// answer of "sent for approval" is said as that: nothing restarted yet.
//
// The contract has no pod list and no restart progress (`ServiceState` has neither), so the dialog does not show pods, and after
// a restart it says the request was accepted, never that the service is back.

export type RestartOutcome = { readonly kind: "applied" } | { readonly kind: "approval"; readonly request: number };

type Props = {
  lane: SwimlaneDetail;
  /** The service to restart, or null when the dialog is closed. */
  service: ServiceState | null;
  onClose: () => void;
  /** The server accepted the restart, or sent it for approval. The page says so after the dialog is gone. */
  onDone: (service: ServiceState, outcome: RestartOutcome) => void;
};

export function RestartDialog({ lane, service, onClose, onDone }: Props) {
  const client = useQueryClient();
  const session = useSession();
  const isAdmin = roleAtLeast(session.status === "ready" ? session.role : null, Role.admin);
  const mutation = useMutation(swimlaneMutations.restart(client));
  const { reset } = mutation;
  // The same gate as the button that opened the dialog, read again here: the dialog can outlive the reason the button was on. If live
  // updates pause, or the agent goes away, or the person's role changes while it is open, the confirm button follows (the server
  // refuses all of it anyway; this only keeps a press from being sent on a screen that can no longer say what the lane looks like).
  const gate = useServiceWriteGate(lane.agent);
  const open = service !== null;
  // A dialog that is opened again starts clean: no error and no answer from the time before.
  useEffect(() => {
    if (open) reset();
  }, [open, reset, service]);

  const run = () => {
    if (service === null) return;
    mutation.mutate(
      { swimlane: lane.id, service: service.service },
      {
        onSuccess: (result: WriteResult) => {
          if (result.outcome === "proposal_created") return;
          onDone(service, { kind: "applied" });
          onClose();
        },
      },
    );
  };

  if (service === null) return null;
  const name = service.service.name;
  const answer = mutation.data;

  if (answer?.outcome === "proposal_created") {
    const request = answer.id;
    return (
      <ProblemDialog
        open
        onOpenChange={(next) => {
          if (!next) {
            onDone(service, { kind: "approval", request });
            onClose();
          }
        }}
        variant="approval"
        title="Restart sent for approval"
        description={`Nothing restarts until an Admin approves request AP-${request}.`}
        actions={
          <>
            {isAdmin ? (
              <Button asChild variant="secondary">
                <AppLink route="approval" params={{ id: String(request) }}>
                  View request
                </AppLink>
              </Button>
            ) : null}
            <Button
              onClick={() => {
                onDone(service, { kind: "approval", request });
                onClose();
              }}
            >
              Close
            </Button>
          </>
        }
      >
        <p className="text-sm text-muted-foreground">
          <SafeText text={name} className="font-mono" /> in <SafeText text={lane.display_name} className="font-mono" />, request AP-{request}.
        </p>
      </ProblemDialog>
    );
  }

  const files = pendingFilesSummary(service);
  const failure = mutation.error === null ? null : restartFailure(mutation.error, { swimlane: lane.display_name, baseline: lane.baseline });

  return (
    <ConsequenceDialog
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
      title={`Restart ${name} in ${lane.id}?`}
      consequence={`Restarting ${name} replaces its pods. Requests to it can fail while its pods come back.`}
      actionLabel={`Restart ${name}`}
      workingLabel="Restarting…"
      working={mutation.isPending}
      canConfirm={gate.allowed}
      error={failure}
      onConfirm={run}
      size="md"
    >
      <div className="flex flex-col gap-3 text-sm">
        {files.first !== null ? (
          <p>
            The restart picks up <SafeText text={files.first} className="font-mono text-xs font-medium" />
            {files.more > 0 ? ` and ${files.more} more ${files.more === 1 ? "file" : "files"}` : ""}.
          </p>
        ) : null}
        <Alert variant="warning" icon={<User aria-hidden="true" />}>
          <span>
            Other people testing in <SafeText text={lane.display_name} className="font-mono" /> will be interrupted.
          </span>
        </Alert>
        {gate.allowed ? null : (
          <Alert variant="warning">
            <SafeText text={gate.reason ?? "Not available."} />
          </Alert>
        )}
      </div>
    </ConsequenceDialog>
  );
}

