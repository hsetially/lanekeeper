import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, type RefObject } from "react";
import { Role, type SwimlaneDetail, type WriteProposalCreated } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { CopyButton } from "@/components/app/CopyButton";
import { SafeText } from "@/components/app/SafeText";
import { OutcomeDialog } from "@/components/notify/OutcomeDialog";
import { PathList } from "@/components/notify/PathList";
import { SentForApprovalDialog } from "@/components/notify/SentForApprovalDialog";
import { ServiceList } from "@/components/notify/ServiceList";
import { Alert } from "@/components/ui/alert";
import { swimlaneMutations } from "@/features/swimlanes/mutations";
import { useServiceWriteGate } from "./gates";
import type { NotifyPlan } from "./paths";
import { notifyFailure } from "./write-error";

// "Notify services" (D85): tell the config-server which files changed, through the swimlane's agent. An Operator's write; the hub
// records it in the audit trail. Like every write it is never optimistic: the dialog says first what will be announced and that it
// is recorded, sends nothing until the person confirms, and then shows what the server answered and nothing more:
//  - `NotifyResult`: the count the hub accepted and the audit event. The contract has no result for each service and no list of the
//    paths it left out, so the dialog invents neither. A count lower than what was sent is said as partial, with the words "its
//    answer does not say which";
//  - `proposal_created`: the request went to an Admin and NOTHING was sent to the config-server;
//  - a refusal stays in the dialog with its request ID (notifyFailure).
// The screen never sends a second request by itself: a person who wants to try again presses the button again.
//
// `notifications_enabled` is shown as each agent reported it for a service (Q35). Nothing here says what it changes.

type Props = {
  lane: SwimlaneDetail;
  /** What to announce, fixed when the dialog was opened (so what the person read is what is sent). */
  plan: NotifyPlan;
  open: boolean;
  onClose: () => void;
  /** Where focus goes when the dialog closes: the button that started the flow (in every state of the dialog). */
  returnFocusRef?: RefObject<HTMLElement | null>;
};

const noun = (n: number, one: string, many: string): string => (n === 1 ? one : many);

// A notification answers `NotifyResult` (a count and an audit event) or, for a person whose writes need approval, the proposal that
// was made instead. They are told apart by `outcome`, which only the proposal has. Read from `unknown`, so the check is made at run
// time and not only by the types.
function isProposal(answer: unknown): answer is WriteProposalCreated {
  return typeof answer === "object" && answer !== null && "outcome" in answer && answer.outcome === "proposal_created";
}

export function NotifyServicesDialog({ lane, plan, open, onClose, returnFocusRef }: Props) {
  const client = useQueryClient();
  const session = useSession();
  const gate = useServiceWriteGate(lane.agent);
  const mutation = useMutation(swimlaneMutations.notify(client));
  const { reset } = mutation;
  // A dialog that is opened again starts clean: no error and no answer from the time before.
  useEffect(() => {
    if (open) reset();
  }, [open, reset]);

  if (!open) return null;

  const sent = plan.paths.length;
  const isAdmin = roleAtLeast(session.status === "ready" ? session.role : null, Role.admin);
  const needsApproval = session.status === "ready" && session.me.requires_approval;
  const files = sent === 1 ? "this file" : `these ${sent} files`;
  const subject = `${sent} ${noun(sent, "path", "paths")} in ${lane.display_name}`;
  const answer = mutation.data;

  if (answer !== undefined && isProposal(answer)) {
    return (
      <SentForApprovalDialog
        open
        title="Notification sent for approval"
        description={`Nothing is sent to the config-server until an Admin approves request AP-${answer.id}.`}
        subject={subject}
        request={answer.id}
        canOpenRequest={isAdmin}
        onClose={onClose}
        {...(returnFocusRef ? { returnFocusRef } : {})}
      />
    );
  }

  if (answer !== undefined) {
    const accepted = answer.accepted;
    const all = accepted >= sent;
    return (
      <OutcomeDialog
        open
        onClose={onClose}
        {...(returnFocusRef ? { returnFocusRef } : {})}
        tone={all ? "success" : "warning"}
        title={all ? "Notification sent" : "Notification partly accepted"}
        description={
          all
            ? `The hub accepted ${accepted} ${noun(accepted, "path", "paths")} for ${lane.display_name}.`
            : `The hub accepted ${accepted} of ${sent} paths for ${lane.display_name}. Its answer does not say which paths it left out or why.`
        }
      >
        <div className="flex flex-col gap-2 text-sm">
          <p className="flex flex-wrap items-center gap-x-1.5">
            <span className="text-muted-foreground">Audit event</span>
            <span className="font-mono">{answer.audit_id}</span>
            <CopyButton value={String(answer.audit_id)} what="audit event number" />
          </p>
          <p className="text-muted-foreground">The hub&apos;s answer does not say which services picked the change up. A service&apos;s pickup state changes when the agent reports it.</p>
          {all ? null : <p className="text-muted-foreground">Nothing was sent again. Check the services&apos; pickup state, then notify again if you need to.</p>}
        </div>
      </OutcomeDialog>
    );
  }

  const failure = mutation.error === null ? null : notifyFailure(mutation.error, { swimlane: lane.display_name, baseline: lane.baseline });
  const run = (): void => {
    mutation.mutate({ swimlane: lane.id, body: { paths: [...plan.paths] } });
  };
  const listed = plan.services.map((service) => ({ service: service.service, note: `notifications reported ${service.notifications_enabled ? "on" : "off"}` }));

  return (
    <ConsequenceDialog
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
      title={`Notify services in ${lane.id}?`}
      consequence={
        needsApproval
          ? `Ask an Admin to tell the config-server that ${files} changed in ${lane.display_name}. Nothing is sent until the request is approved.`
          : `Tell the config-server that ${files} changed in ${lane.display_name}. The agent sends the request. It is recorded in the audit trail, and nothing is restarted.`
      }
      actionLabel="Notify services"
      workingLabel="Notifying…"
      working={mutation.isPending}
      canConfirm={gate.allowed && sent > 0}
      error={failure}
      onConfirm={run}
      size="md"
      {...(returnFocusRef ? { returnFocusRef } : {})}
    >
      <div className="flex flex-col gap-3 text-sm">
        {sent === 0 ? (
          <Alert variant="info">No service is waiting on a file, so there is nothing to announce.</Alert>
        ) : (
          <PathList label={`${sent} ${noun(sent, "file", "files")} to announce`} paths={plan.paths} />
        )}
        {plan.left > 0 ? (
          <Alert variant="warning">
            {plan.left} more {noun(plan.left, "file is", "files are")} waiting and {noun(plan.left, "is", "are")} not in this request. One request holds at most 100 paths. Notify again afterwards to send the rest.
          </Alert>
        ) : null}
        {plan.services.length > 0 ? (
          <ServiceList label={`${plan.services.length} ${noun(plan.services.length, "service", "services")} waiting on ${noun(sent, "this file", "these files")}`} services={listed} />
        ) : sent > 0 ? (
          <p className="text-muted-foreground">
            No service is listed as waiting on {noun(sent, "this file", "these files")}. The notification is still sent.
          </p>
        ) : null}
        {needsApproval ? <Alert variant="info">Your changes need approval, so this goes to an Admin as a request. Nothing is sent to the config-server until it is approved.</Alert> : null}
        {gate.allowed ? null : (
          <Alert variant="warning">
            <SafeText text={gate.reason ?? "Not available."} />
          </Alert>
        )}
      </div>
    </ConsequenceDialog>
  );
}
