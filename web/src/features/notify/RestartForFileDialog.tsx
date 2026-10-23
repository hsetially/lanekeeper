import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type RefObject } from "react";
import { Role, type NfsPath, type RestartOffer, type ServiceRef, type SwimlaneDetail } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { DestructiveConfirm } from "@/components/app/DestructiveConfirm";
import { SafeText } from "@/components/app/SafeText";
import { WriteError, type WriteErrorInfo } from "@/components/app/WriteError";
import { OfferErrorDialog, OfferLoadingDialog } from "@/components/notify/OfferPhases";
import { OutcomeDialog } from "@/components/notify/OutcomeDialog";
import { ServiceList } from "@/components/notify/ServiceList";
import { StepList } from "@/components/notify/StepList";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { swimlaneMutations } from "@/features/swimlanes/mutations";
import { restartFailure } from "@/features/swimlanes/services/write-error";
import { TriangleAlert } from "@/icons";
import { isRetryableProblem } from "@/lib/query/use-query-state";
import { AppLink } from "@/routes/AppLink";
import { useServiceWriteGate } from "./gates";
import { restartOfferQuery } from "./queries";
import { abandonRest, begin, describeStop, mark, summarise, type Step } from "./restart-run";
import { offerNeedsOperator, offerReadFailure } from "./write-error";

// "Restart services" for a file that was just written (D49, "restart all"). The hub lists the services that read the file: every
// service for a file in the root folder, one application's Deployments for a file in its folder. The screen restarts them ONE
// REQUEST AT A TIME with `restartService`, because a restart is audited and locked per service. So:
//  - the confirmation lists exactly what the hub listed, says when the list is not the whole of what the swimlane has
//    (`truncated`), and is confirmed by typing the swimlane's name, because several services can go at once;
//  - the offer is read when the dialog opens and not again once the person has confirmed;
//  - a run stops at the first request the hub refuses and says which were never sent: it never skips a refused one, and it never
//    sends a request again by itself;
//  - each answer is shown for what it is. "Accepted" is the hub's answer to a REQUEST. `ServiceState` has no restarting state, so
//    nothing here says a service restarted, and "sent for approval" is not done;
//  - leaving the screen stops the run: no request is sent after the dialog is gone.

type Props = {
  lane: SwimlaneDetail;
  /** The file that was written. */
  path: NfsPath;
  open: boolean;
  onClose: () => void;
  /** Where focus goes when the dialog closes: the button that started the flow (in every state of the dialog). */
  returnFocusRef?: RefObject<HTMLElement | null>;
};

type Run = { readonly steps: readonly Step[]; /** The request that is out (an index), or the number of services when none is. */ readonly at: number; readonly failure: WriteErrorInfo | null; readonly finished: boolean };

const WHAT = "the services to restart";
const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);
const baseName = (path: string): string => path.slice(path.lastIndexOf("/") + 1);

export function RestartForFileDialog({ lane, path, open, onClose, returnFocusRef }: Props) {
  const client = useQueryClient();
  const session = useSession();
  const role = session.status === "ready" ? session.role : null;
  const canRead = roleAtLeast(role, Role.operator);
  const isAdmin = roleAtLeast(role, Role.admin);
  const needsApproval = session.status === "ready" && session.me.requires_approval;
  const gate = useServiceWriteGate(lane.agent);
  const mutation = useMutation(swimlaneMutations.restart(client));
  const [run, setRun] = useState<Run | null>(null);
  // The offer is a read for an Operator, asked while the dialog is open and until the person has confirmed it.
  const offer = useQuery({ ...restartOfferQuery(lane.id, path), enabled: open && canRead && run === null });
  const { refetch } = offer;
  // Leaving stops the run. Set in the effect, not at the first render, so that a second mount (Strict Mode) is alive again.
  const alive = useRef(true);
  const isAlive = (): boolean => alive.current;
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  if (!open) return null;
  if (!canRead) return <OfferErrorDialog info={offerNeedsOperator(WHAT)} onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} />;

  const name = baseName(path);

  async function start(services: readonly ServiceRef[]): Promise<void> {
    let steps = begin(services);
    for (const [index, service] of services.entries()) {
      if (!isAlive()) return;
      steps = mark(steps, index, "sending");
      setRun({ steps, at: index, failure: null, finished: false });
      try {
        const result = await mutation.mutateAsync({ swimlane: lane.id, service });
        steps = result.outcome === "proposal_created" ? mark(steps, index, "sent-for-approval", result.id) : mark(steps, index, "accepted");
      } catch (error) {
        const problem = error instanceof ApiProblem ? error : new ApiProblem({ status: 0, kind: "network", title: "Could not reach the server" });
        const failure = restartFailure(problem, { swimlane: lane.display_name, baseline: lane.baseline });
        if (isAlive()) setRun({ steps: abandonRest(mark(steps, index, "failed"), index + 1), at: index, failure, finished: true });
        return;
      }
    }
    if (isAlive()) setRun({ steps, at: services.length, failure: null, finished: true });
  }

  if (run?.finished === true) {
    const summary = summarise(run.steps);
    const list = <StepList label="Restart requests" steps={run.steps} />;
    if (run.failure !== null) {
      const refused = run.steps.find((step) => step.state === "failed");
      return (
        <OutcomeDialog open onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} tone="warning" title="Restart stopped early" description={describeStop(summary, refused === undefined ? lane.display_name : `${refused.service.namespace}/${refused.service.name}`)}>
          <div className="flex flex-col gap-3 text-sm">
            <WriteError {...run.failure} />
            {list}
            <p className="text-muted-foreground">Nothing was sent again. Open this again to restart the rest.</p>
          </div>
        </OutcomeDialog>
      );
    }
    const viewRequests = isAdmin && summary.sentForApproval > 0 ? (
      <Button asChild variant="secondary">
        <AppLink route="approvals">View requests</AppLink>
      </Button>
    ) : null;
    if (summary.sentForApproval === summary.total) {
      return (
        <OutcomeDialog open onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} tone="info" title="Restart sent for approval" description="Nothing restarts for these services until an Admin approves their requests." actions={viewRequests}>
          {list}
        </OutcomeDialog>
      );
    }
    const mixed = summary.sentForApproval > 0;
    return (
      <OutcomeDialog
        open
        onClose={onClose}
        {...(returnFocusRef ? { returnFocusRef } : {})}
        tone="success"
        title={mixed ? "Restart requested" : `Restart requested for ${summary.total} ${plural(summary.total, "service", "services")}`}
        description={
          mixed
            ? `The hub accepted ${summary.accepted} restart ${plural(summary.accepted, "request", "requests")} and sent ${summary.sentForApproval} for approval. Nothing restarts for ${plural(summary.sentForApproval, "that one", "those")} until an Admin approves ${plural(summary.sentForApproval, "it", "them")}.`
            : `The hub accepted ${summary.accepted} restart ${plural(summary.accepted, "request", "requests")}, one at a time. It reports no restart progress.`
        }
        actions={viewRequests}
      >
        {list}
      </OutcomeDialog>
    );
  }

  if (offer.isError && offer.data === undefined) {
    return <OfferErrorDialog info={offerReadFailure(offer.error, WHAT)} {...(isRetryableProblem(offer.error) ? { onRetry: () => void refetch() } : {})} onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} />;
  }

  if (offer.isPending) {
    return <OfferLoadingDialog title={`Restart services for ${name} in ${lane.id}?`} description="Reading which services read this file." what={WHAT} onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} />;
  }

  const listed: RestartOffer = offer.data;
  if (listed.services.length === 0) {
    return (
      <OutcomeDialog open onClose={onClose} {...(returnFocusRef ? { returnFocusRef } : {})} tone="info" title="No services to restart" description={`The hub lists no service to restart for a change to ${path} in ${lane.display_name}.`}>
        <p className="text-sm text-muted-foreground">The hub builds the list from the services it holds for the swimlane. It lists none when it holds none.</p>
      </OutcomeDialog>
    );
  }

  const n = listed.services.length;
  const these = n === 1 ? "this service" : `these ${n} services`;
  return (
    <DestructiveConfirm
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
      title={`Restart ${n} ${plural(n, "service", "services")} in ${lane.id}?`}
      consequence={
        needsApproval
          ? `Asking an Admin to restart ${these}, which read ${path}. Each restart is its own request, and nothing restarts until it is approved.`
          : `Restarting replaces the pods of ${these}, which read ${path}. Requests to ${plural(n, "it", "them")} can fail while ${plural(n, "its", "their")} pods come back. The requests are sent one at a time, and each is recorded in the audit trail.`
      }
      confirmValue={lane.id}
      actionLabel={`Restart ${n} ${plural(n, "service", "services")}`}
      workingLabel="Restarting…"
      working={run !== null}
      canConfirm={gate.allowed}
      onConfirm={() => {
        void start(listed.services);
      }}
      titleIcon={<TriangleAlert className="size-4" />}
      size="md"
      {...(returnFocusRef ? { returnFocusRef } : {})}
    >
      <div className="flex flex-col gap-3 text-sm">
        <ServiceList label={`${n} ${plural(n, "service", "services")} to restart${listed.truncated ? " (not the whole list)" : ""}`} services={listed.services.map((service) => ({ service }))} />
        {listed.truncated ? <Alert variant="warning">The swimlane has more services than this list holds. Restarting these leaves the others as they are.</Alert> : null}
        <Alert variant="warning">
          <span>
            Other people testing in <SafeText text={lane.display_name} className="font-mono" /> will be interrupted.
          </span>
        </Alert>
        {needsApproval ? (
          <Alert variant="info">{`Your changes need approval, so each restart goes to an Admin as its own request (${n} ${plural(n, "request", "requests")}). Nothing restarts until ${plural(n, "it is", "they are")} approved.`}</Alert>
        ) : null}
        {gate.allowed ? null : (
          <Alert variant="warning">
            <SafeText text={gate.reason ?? "Not available."} />
          </Alert>
        )}
        {run === null ? null : (
          <p role="status" className="text-muted-foreground">
            Request {Math.min(run.at + 1, n)} of {n} sent.
          </p>
        )}
      </div>
    </DestructiveConfirm>
  );
}
