import { useRef, type ReactNode } from "react";
import type { PrJob, PrLink } from "@/api/generated/model";
import { CopyButton } from "@/components/app/CopyButton";
import { COPY_GROUP, COPY_REVEAL } from "@/components/app/CopyValue";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { CircleCheck, CircleDashed, GitPullRequest, Key, LoaderCircle, Ban, Info } from "@/icons";
import { AppLink } from "@/routes/AppLink";
import { httpsUrl, jobSteps, prNumberOf } from "./model";

// The views after the form (Consequence dialogs, 1f to 1k): the PR that already exists, the job as the server reports it, the
// PR when it is done, and the two ways GitHub can say no. Nothing here makes up progress: a step is done because the job's
// state says so, and "created" is said only for a completed job.

function Fx({ tone, icon, title, children }: { tone: "info" | "success" | "danger" | "warning"; icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <Alert variant={tone} icon={icon}>
      <b className="font-semibold">{title}</b>
      {children ? <span className="text-muted-foreground">{children}</span> : null}
    </Alert>
  );
}

function PrLine({ title, branch, base, url }: { title: string; branch?: string; base?: string; url: string | null }) {
  const address = useRef<HTMLAnchorElement>(null);
  return (
    <div className="flex items-start gap-2 rounded-md border border-border p-3">
      <GitPullRequest aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-info-fg" />
      <div className="flex min-w-0 flex-col gap-1">
        <b className="font-semibold">
          <SafeText text={title} />
        </b>
        {branch === undefined || base === undefined ? null : (
          <span className="flex flex-wrap items-center gap-1 font-mono text-xs">
            <SafeText text={branch} /> → <SafeText text={base} />
          </span>
        )}
        {url === null ? null : (
          // The address as the server gave it (https, checked before it is a link); the button copies the same text the link opens.
          <span className={`${COPY_GROUP} inline-flex items-start gap-0.5`}>
            <a ref={address} href={url} target="_blank" rel="noopener noreferrer" className="text-sm break-all">
              {url.replace(/^https:\/\//, "")}
            </a>
            <CopyButton value={url} what="PR link" source={address} className={COPY_REVEAL} />
          </span>
        )}
      </div>
    </div>
  );
}

export function ExistsStep({ link }: { link: PrLink }) {
  const url = httpsUrl(link.pr_url);
  return (
    <div className="flex flex-col gap-3">
      <Fx tone="info" icon={<Info aria-hidden="true" />} title={`PR #${link.pr_number} is already open for this change.`}>
        Same target and same content, so there is nothing new to create.
      </Fx>
      <PrLine title={`Pull request #${link.pr_number}`} url={url} />
    </div>
  );
}

export function existsAction(link: PrLink): ReactNode {
  const url = httpsUrl(link.pr_url);
  return url === null ? null : (
    <Button asChild>
      <a href={url} target="_blank" rel="noopener noreferrer">
        <GitPullRequest aria-hidden="true" />
        Open PR #{link.pr_number}
      </a>
    </Button>
  );
}

export function JobStep({ job, resumed }: { job: PrJob; resumed: boolean }) {
  const steps = jobSteps(job.state);
  return (
    <div className="flex flex-col gap-3">
      {resumed ? <Alert variant="info">Resumed job {job.id} from where it was.</Alert> : null}
      <Fx tone="info" icon={<LoaderCircle aria-hidden="true" className="animate-lk-spin" />} title="Creating your pull request.">
        You can close this dialog. The job keeps running and the file's drift badge updates when it finishes.
      </Fx>
      <ol aria-live="polite" className="m-0 flex list-none flex-col gap-2 p-0">
        {steps.map((step) => (
          <li key={step.label} data-status={step.status} className="flex items-start gap-2">
            {step.status === "done" ? (
              <CircleCheck aria-label="Done" className="mt-0.5 size-4 shrink-0 text-success-fg" />
            ) : step.status === "active" ? (
              <LoaderCircle aria-label="Working" className="mt-0.5 size-4 shrink-0 animate-lk-spin text-info-fg" />
            ) : (
              <CircleDashed aria-label="Waiting" className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
            )}
            <b className={step.status === "pending" ? "font-semibold text-muted-foreground" : "font-semibold"}>{step.label}</b>
          </li>
        ))}
      </ol>
      {(job.warnings ?? []).map((w, i) => (
        <Alert key={i} variant="warning">
          <SafeText text={w} />
        </Alert>
      ))}
      <p className="m-0 text-xs text-muted-foreground">
        Job <span className="font-mono">{job.id}</span> · started by you · stored on the server, so it goes on if this dialog is closed.
      </p>
    </div>
  );
}

export function DoneStep({ job, target, lane, title }: { job: PrJob; target: "tenant" | "base"; lane: string; title: string }) {
  const url = job.pr_url === null ? null : httpsUrl(job.pr_url);
  const number = url === null ? null : prNumberOf(url);
  return (
    <div className="flex flex-col gap-3">
      <Fx tone="success" icon={<CircleCheck aria-hidden="true" />} title={number === null ? "PR created." : `PR #${number} created.`}>
        {target === "tenant" ? `The file stays NFS ahead until the PR merges and ${lane} syncs.` : "Base changes reach every swimlane on its next sync after the PR merges."}
      </Fx>
      <PrLine title={title} url={url} />
    </div>
  );
}

export function doneAction(job: PrJob): ReactNode {
  const url = job.pr_url === null ? null : httpsUrl(job.pr_url);
  if (url === null) return null;
  const number = prNumberOf(url);
  return (
    <Button asChild>
      <a href={url} target="_blank" rel="noopener noreferrer">
        <GitPullRequest aria-hidden="true" />
        Open PR {number === null ? "" : `#${number} `}on GitHub
      </a>
    </Button>
  );
}

export function FailedJobStep({ job }: { job: PrJob }) {
  return (
    <div className="flex flex-col gap-3">
      <Fx tone="danger" icon={<Ban aria-hidden="true" />} title="The pull request job failed.">
        The job stopped before the PR was opened. Check GitHub for a branch before you try again.
      </Fx>
      {job.error === null ? null : (
        <dl className="m-0 grid grid-cols-[96px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
          <dt className="text-muted-foreground">What the server said</dt>
          <dd className="m-0">
            <SafeText text={job.error} />
          </dd>
        </dl>
      )}
    </div>
  );
}

export function TokenStep({ why, requestId, repo }: { why: "expired" | "unusable"; requestId: string | undefined; repo: string | undefined }) {
  return (
    <div className="flex flex-col gap-3">
      <Fx
        tone="danger"
        icon={<Key aria-hidden="true" />}
        title={why === "expired" ? "Your GitHub token expired, so the PR was not created." : "Your GitHub token cannot be used, so the PR was not created."}
      >
        Renew it in My token, then retry. Your title and description are kept.
      </Fx>
      <dl className="m-0 grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
        <dt className="text-muted-foreground">What happened</dt>
        <dd className="m-0">{why === "expired" ? "GitHub did not accept the token saved in My token, because it has expired." : "GitHub or Lanekeeper could not use the token saved in My token."}</dd>
        {repo === undefined ? null : (
          <>
            <dt className="text-muted-foreground">Repository</dt>
            <dd className="m-0 font-mono text-xs">
              <SafeText text={repo} />
            </dd>
          </>
        )}
        <dt className="text-muted-foreground">Written to Git</dt>
        <dd className="m-0">Nothing. No branch or commit was created.</dd>
        {requestId === undefined ? null : (
          <>
            <dt className="text-muted-foreground">Request ID</dt>
            <dd className="m-0">
              <RequestId id={requestId} />
            </dd>
          </>
        )}
      </dl>
    </div>
  );
}

export function PushStep({ repo, requestId, alternative }: { repo: string; requestId: string | undefined; alternative: string | null }) {
  return (
    <div className="flex flex-col gap-3">
      <Fx tone="danger" icon={<Ban aria-hidden="true" />} title={`You don't have push access to ${repo}, so the PR was not created.`}>
        Ask a maintainer for Write access, then retry. Nothing was written to Git.
      </Fx>
      <dl className="m-0 grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
        <dt className="text-muted-foreground">Why</dt>
        <dd className="m-0">
          Your GitHub token is valid, but it cannot push to <span className="font-mono text-xs">{repo}</span>.
        </dd>
        <dt className="text-muted-foreground">Next step</dt>
        <dd className="m-0">
          Ask a maintainer of <span className="font-mono text-xs">{repo}</span> to add you with Write access.{alternative === null ? "" : ` ${alternative}`}
        </dd>
        {requestId === undefined ? null : (
          <>
            <dt className="text-muted-foreground">Request ID</dt>
            <dd className="m-0">
              <RequestId id={requestId} />
            </dd>
          </>
        )}
      </dl>
    </div>
  );
}

export function MyTokenLink() {
  return (
    <Button asChild>
      <AppLink route="github-token">
        <Key aria-hidden="true" />
        Go to My token
      </AppLink>
    </Button>
  );
}
