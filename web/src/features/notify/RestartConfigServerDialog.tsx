import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, type RefObject } from "react";
import { Role, type ConfigServerRestartOffer, type SwimlaneDetail } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { CopyButton } from "@/components/app/CopyButton";
import { DestructiveConfirm } from "@/components/app/DestructiveConfirm";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { SafeText } from "@/components/app/SafeText";
import { OfferErrorDialog, OfferLoadingDialog } from "@/components/notify/OfferPhases";
import { OutcomeDialog } from "@/components/notify/OutcomeDialog";
import { SentForApprovalDialog } from "@/components/notify/SentForApprovalDialog";
import { ServiceList } from "@/components/notify/ServiceList";
import { Alert } from "@/components/ui/alert";
import { TriangleAlert } from "@/icons";
import { swimlaneMutations } from "@/features/swimlanes/mutations";
import { restartFailure } from "@/features/swimlanes/services/write-error";
import { isRetryableProblem } from "@/lib/query/use-query-state";
import { useServiceWriteGate } from "./gates";
import { configServerRestartOfferQuery } from "./queries";
import { offerNeedsOperator, offerReadFailure } from "./write-error";

// "Restart config-server" (D83, D87): the restart that finding C11 asks for, which is the ordinary `restartService` on the one
// Deployment the hub names. The hub offers it only while C11 is open for the swimlane and the cluster report lists exactly one
// Deployment with the configured name; with none or several the offer is null and the hub does not guess, so the dialog says
// that, as the hub's own answer, and offers nothing to confirm.
//
// While the config-server restarts, every other service of the swimlane is unserved (they all ask it for their files), so the
// confirmation lists them exactly as the hub gave them, says when the list is not the whole of them (`truncated`), and asks for
// the Deployment's name to be typed. Nothing is optimistic. The offer is read when the dialog opens and not again once the person
// has pressed the button, so what they confirmed is what is restarted. `ServiceState` has no restarting state, so after the hub
// accepts the request the dialog says "accepted" and shows no progress.

type Props = {
  lane: SwimlaneDetail;
  open: boolean;
  onClose: () => void;
  /** Where focus goes when the dialog closes: the button that started the flow (in every state of the dialog). */
  returnFocusRef?: RefObject<HTMLElement | null>;
};

const OFFER = "the config-server restart offer";
const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

export function RestartConfigServerDialog({ lane, open, onClose, returnFocusRef }: Props) {
  const client = useQueryClient();
  const session = useSession();
  const role = session.status === "ready" ? session.role : null;
  const canRead = roleAtLeast(role, Role.operator);
  const isAdmin = roleAtLeast(role, Role.admin);
  const needsApproval = session.status === "ready" && session.me.requires_approval;
  const gate = useServiceWriteGate(lane.agent);
  const mutation = useMutation(swimlaneMutations.restart(client));
  const { reset } = mutation;
  // The offer is a read for an Operator, asked while the dialog is open. Once the restart is out (or has answered with a refusal) it
  // is pinned: it is not read again, so it cannot change under a person who has already confirmed it.
  const offer = useQuery({ ...configServerRestartOfferQuery(lane.id), enabled: open && canRead && mutation.isIdle });
  const { refetch } = offer;
  // A dialog that is opened again starts clean: no error and no answer from the time before.
  useEffect(() => {
    if (open) reset();
  }, [open, reset]);

  if (!open) return null;

  if (!canRead) {
    return <OfferErrorDialog info={offerNeedsOperator(OFFER)} onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} />;
  }

  const answer = mutation.data;
  const picked = offer.data?.offer ?? null;
  const subject = picked === null ? lane.display_name : `${picked.service.name} in ${lane.display_name}`;

  if (answer !== undefined && picked !== null) {
    if (answer.outcome === "proposal_created") {
      return (
        <SentForApprovalDialog
          open
          title="Config-server restart sent for approval"
          description={`Nothing restarts until an Admin approves request AP-${answer.id}.`}
          subject={subject}
          request={answer.id}
          canOpenRequest={isAdmin}
          onClose={onClose}
          {...(returnFocusRef ? { returnFocusRef } : {})}
        />
      );
    }
    return (
      <OutcomeDialog open onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} tone="success" title="Restart requested" description={`The hub accepted the restart of ${picked.service.name} in ${lane.display_name}.`}>
        <div className="flex flex-col gap-2 text-sm">
          <p className="flex flex-wrap items-center gap-x-1.5">
            <span className="text-muted-foreground">Audit event</span>
            <span className="font-mono">{answer.audit_id}</span>
            <CopyButton value={String(answer.audit_id)} what="audit event number" />
          </p>
          <p className="text-muted-foreground">The hub reports no restart progress. Finding C11 clears once the config-server has restarted.</p>
        </div>
      </OutcomeDialog>
    );
  }

  if (offer.isError && picked === null) {
    return <OfferErrorDialog info={offerReadFailure(offer.error, OFFER)} {...(isRetryableProblem(offer.error) ? { onRetry: () => void refetch() } : {})} onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} />;
  }

  if (offer.isPending || offer.data === undefined) {
    return <OfferLoadingDialog title={`Restart the config-server in ${lane.id}?`} description="Reading what the restart would take away." what={OFFER} onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} />;
  }

  if (picked === null) {
    return (
      <OutcomeDialog
        open
        onClose={onClose}
        {...(returnFocusRef ? { returnFocusRef } : {})}
        tone="info"
        title="No config-server restart is offered"
        description={`The hub offers one for ${lane.display_name} only while check C11 is open for it and the cluster report lists exactly one config-server Deployment.`}
      >
        <p className="text-sm text-muted-foreground">It offers none now. Its answer does not say which of the two is missing. With no Deployment, or with several, the hub does not guess which one is meant.</p>
      </OutcomeDialog>
    );
  }

  return (
    <Confirm
      lane={lane}
      offer={picked}
      needsApproval={needsApproval}
      gateReason={gate.allowed ? null : (gate.reason ?? "Not available.")}
      working={mutation.isPending}
      failure={mutation.error === null ? null : restartFailure(mutation.error, { swimlane: lane.display_name, baseline: lane.baseline })}
      onConfirm={() => {
        mutation.mutate({ swimlane: lane.id, service: picked.service });
      }}
      onClose={onClose}
      {...(returnFocusRef ? { returnFocusRef } : {})}
    />
  );
}

type ConfirmProps = {
  lane: SwimlaneDetail;
  offer: ConfigServerRestartOffer;
  needsApproval: boolean;
  /** Why the restart cannot be confirmed now (the role, the paused live updates, the agent), or null when it can. */
  gateReason: string | null;
  working: boolean;
  failure: WriteErrorInfo | null;
  onConfirm: () => void;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
};

function Confirm({ lane, offer, needsApproval, gateReason, working, failure, onConfirm, onClose, returnFocusRef }: ConfirmProps) {
  const name = offer.service.name;
  const n = offer.affected.length;
  return (
    <DestructiveConfirm
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
      title={`Restart ${name} in ${lane.id}?`}
      consequence={
        needsApproval
          ? `Asking an Admin to restart the config-server, which replaces its pods. While it restarts, the other services of ${lane.display_name} cannot be served their config, because they all ask it for their files. Nothing restarts until the request is approved.`
          : `Restarting the config-server replaces its pods. While it restarts, the other services of ${lane.display_name} cannot be served their config, because they all ask it for their files. It is recorded in the audit trail.`
      }
      confirmValue={name}
      actionLabel={`Restart ${name}`}
      workingLabel="Restarting…"
      working={working}
      canConfirm={gateReason === null}
      error={failure}
      onConfirm={onConfirm}
      titleIcon={<TriangleAlert className="size-4" />}
      size="md"
      {...(returnFocusRef ? { returnFocusRef } : {})}
    >
      <div className="flex flex-col gap-3 text-sm">
        {n === 0 ? (
          <p className="text-muted-foreground">
            <SafeText text={`The hub lists no other service in ${lane.display_name}.`} />
          </p>
        ) : (
          <ServiceList label={`${n} other ${plural(n, "service", "services")} affected${offer.truncated ? " (not the whole list)" : ""}`} services={offer.affected.map((service) => ({ service }))} />
        )}
        {offer.truncated ? <Alert variant="warning">The swimlane has more services than this list shows, so the list is not the whole of the services affected.</Alert> : null}
        <Alert variant="warning">
          <span>
            Other people testing in <SafeText text={lane.display_name} className="font-mono" /> will be interrupted.
          </span>
        </Alert>
        {needsApproval ? <Alert variant="info">Your changes need approval, so this goes to an Admin as a request. Nothing restarts until it is approved.</Alert> : null}
        {gateReason === null ? null : (
          <Alert variant="warning">
            <SafeText text={gateReason} />
          </Alert>
        )}
      </div>
    </DestructiveConfirm>
  );
}
