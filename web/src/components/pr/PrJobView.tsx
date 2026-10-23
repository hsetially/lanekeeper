import type { ReactNode } from "react";
import type { PrJob } from "@/api/generated/model";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Ban, CircleCheck, CircleDashed, GitPullRequest, LoaderCircle, RefreshCw, TriangleAlert, WifiOff } from "@/icons";
import { formatRelative, formatTime } from "@/lib/time";
import { useNow } from "@/live/use-now";
import { AppLink } from "@/routes/AppLink";
import { jobSteps, phaseOf } from "./job-steps";
import { PrAddressLine } from "./PrCard";
import { PrBadge } from "./PrBadge";
import { prAddress } from "./pr-url";

// The pull-request job as the server reports it (Consequence dialogs, frames 1g to 1i). The job is the server's: this view draws
// `PrJob` and invents nothing.
//
//  - Progress is the job's STATE (job-steps.ts). The contract carries no step times and no step text, so none is drawn.
//  - "Created" is said only for a COMPLETED job, and the pull request is shown only from `pr_url` of a completed job, checked as
//    an address (https, a host, no credentials). A completed job with no usable address says so and points to the list.
//  - A failed job says what the server said (`error`, untrusted text, through SafeText) and shows no steps: the state does not
//    say how far it got.
//  - The job is stored on the server and goes on without this page, but the page cannot keep the job's id (the app uses no web
//    storage outside the preferences), so a job is never "resumed after a reload" (design frame 1h is a deviation). "Find my pull
//    request" goes to the swimlane's pull requests, where the pull request appears once the job opens it.
//
// `behind` and `refreshFailed` are the connected view's (PrJobProgress): the live stream is down so this may be behind the server,
// or the last read of the job failed and the last state is what is shown.

type Props = {
  job: PrJob;
  /** What a created pull request means for the target (tenant or base): shown under "created". */
  effect?: ReactNode;
  /** The title the person gave the pull request: the job does not carry it. */
  title?: string | undefined;
  /** Live updates are paused: say the view may be behind, and offer to read the job now. */
  behind?: { onCheck: () => void; checking: boolean } | null | undefined;
  /** The last read of the job failed: the state shown is the last one that was read. */
  refreshFailed?: { requestId?: string | undefined; onRetry: () => void } | null | undefined;
};

function StepList({ state }: { state: PrJob["state"] }) {
  return (
    <ol aria-live="polite" className="m-0 flex list-none flex-col gap-2 p-0">
      {jobSteps(state).map((step) => (
        <li key={step.label} data-status={step.status} className="flex items-start gap-2">
          {step.status === "done" ? (
            <CircleCheck aria-label="Done" className="mt-0.5 size-4 shrink-0 text-success-fg" />
          ) : step.status === "active" ? (
            <LoaderCircle aria-label="Working" className="mt-0.5 size-4 shrink-0 animate-lk-spin text-info-fg" />
          ) : (
            <CircleDashed aria-label="Waiting" className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          )}
          <b className={step.status === "waiting" ? "font-semibold text-muted-foreground" : "font-semibold"}>{step.label}</b>
        </li>
      ))}
    </ol>
  );
}

export function PrJobView({ job, effect, title, behind, refreshFailed }: Props) {
  const phase = phaseOf(job.state);
  const now = useNow(true, 30_000);
  const address = job.pr_url === null ? null : prAddress(job.pr_url);
  const files = job.paths.length;

  return (
    <div data-slot="pr-job" data-phase={phase} className="flex flex-col gap-3">
      {phase === "queued" ? (
        <Alert variant="info" icon={<LoaderCircle aria-hidden="true" className="animate-lk-spin" />}>
          <b className="font-semibold">Waiting to start.</b>
          <span className="text-muted-foreground">The job is queued on the server. You can close this dialog; the job starts on its own.</span>
        </Alert>
      ) : null}
      {phase === "running" ? (
        <Alert variant="info" icon={<LoaderCircle aria-hidden="true" className="animate-lk-spin" />}>
          <b className="font-semibold">Creating your pull request.</b>
          <span className="text-muted-foreground">You can close this dialog. The job keeps running and the file's drift badge updates when it finishes.</span>
        </Alert>
      ) : null}
      {phase === "completed" ? (
        <>
          <Alert variant="success" icon={<CircleCheck aria-hidden="true" />}>
            <b className="font-semibold">{address !== null && address.number !== null ? `PR #${address.number} created.` : "Pull request created."}</b>
            {effect === undefined ? null : <span className="text-muted-foreground">{effect}</span>}
          </Alert>
          {address === null ? (
            <Alert variant="warning" icon={<TriangleAlert aria-hidden="true" />}>
              <b className="font-semibold">The job finished, but the server gave no address that can be opened.</b>
              <span className="text-muted-foreground">Find the pull request in the list of this swimlane.</span>
            </Alert>
          ) : (
            <>
              <div data-slot="pr-card" className="flex items-start gap-2 rounded-md border border-border p-3">
                <GitPullRequest aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-info-fg" />
                <div className="flex min-w-0 flex-col gap-1">
                  <b className="font-semibold">{title === undefined || title === "" ? "Pull request" : <SafeText text={title} />}</b>
                  <PrAddressLine address={address} />
                </div>
              </div>
              {address.number === null ? null : (
                <p className="m-0 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                  Drift list now shows <PrBadge info={{ kind: "open", link: { pr_number: address.number, pr_url: address.href }, outdated: false }} /> on this file.
                </p>
              )}
            </>
          )}
        </>
      ) : null}
      {phase === "failed" ? (
        <>
          <Alert variant="danger" icon={<Ban aria-hidden="true" />}>
            <b className="font-semibold">The pull request job failed.</b>
            <span className="text-muted-foreground">The job stopped before the PR was opened. Check GitHub for a branch before you try again.</span>
          </Alert>
          {job.error === null ? null : (
            <dl className="m-0 grid grid-cols-[96px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
              <dt className="text-muted-foreground">What the server said</dt>
              <dd className="m-0">
                <SafeText text={job.error} />
              </dd>
            </dl>
          )}
        </>
      ) : null}

      {phase === "queued" || phase === "running" ? <StepList state={job.state} /> : null}

      {(job.warnings ?? []).map((warning, i) => (
        <Alert key={i} variant="warning">
          <SafeText text={warning} />
        </Alert>
      ))}

      {behind ? (
        <Alert variant="warning" icon={<WifiOff aria-hidden="true" />}>
          <span>Live updates are paused, so this may be behind the server.</span>
          <span>
            <Button variant="secondary" size="sm" loading={behind.checking} onClick={behind.onCheck}>
              <RefreshCw aria-hidden="true" />
              Check now
            </Button>
          </span>
        </Alert>
      ) : null}
      {refreshFailed ? (
        <Alert variant="warning" icon={<TriangleAlert aria-hidden="true" />}>
          <span>Couldn't refresh this job. Showing the last state we have.</span>
          <span className="flex flex-wrap items-center gap-2">
            {refreshFailed.requestId === undefined ? null : <RequestId id={refreshFailed.requestId} />}
            <Button variant="secondary" size="sm" onClick={refreshFailed.onRetry}>
              Retry
            </Button>
          </span>
        </Alert>
      ) : null}

      <p className="m-0 text-xs text-muted-foreground">
        Job <span className="font-mono">{job.id}</span> · {files} {files === 1 ? "file" : "files"} · updated <span title={formatTime(job.updated_at)}>{formatRelative(job.updated_at, now)}</span>
        {phase === "failed" ? null : (
          <>
            {" "}
            ·{" "}
            <AppLink route="swimlane-pull-requests" params={{ id: job.swimlane }} className="underline underline-offset-2">
              Find my pull request
            </AppLink>
          </>
        )}
      </p>
    </div>
  );
}
