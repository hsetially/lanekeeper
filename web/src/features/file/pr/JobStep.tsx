import type { ReactNode } from "react";
import type { PrJob } from "@/api/generated/model";
import { RequestId } from "@/components/app/RequestId";
import { SafeText } from "@/components/app/SafeText";
import { prAddress } from "@/components/pr/pr-url";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { GitPullRequest, Key, Ban } from "@/icons";
import { AppLink } from "@/routes/AppLink";

// The views after the form that are not the pull request's own (Consequence dialogs, 1j and 1k): the two ways GitHub can say no
// (an unusable token, no push access), and the action of a finished job. The pull request that already exists and the job as the
// server reports it are shared pieces (src/components/pr), which the Pull requests page and the live updates use too.

function Fx({ tone, icon, title, children }: { tone: "info" | "success" | "danger" | "warning"; icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <Alert variant={tone} icon={icon}>
      <b className="font-semibold">{title}</b>
      {children ? <span className="text-muted-foreground">{children}</span> : null}
    </Alert>
  );
}

/** The footer's action for a finished job: the pull request on GitHub, from the address the server gave, if it can be opened. */
export function doneAction(job: PrJob): ReactNode {
  const address = job.pr_url === null ? null : prAddress(job.pr_url);
  if (address === null) return null;
  return (
    <Button asChild>
      <a href={address.href} target="_blank" rel="noopener noreferrer">
        <GitPullRequest aria-hidden="true" />
        Open PR {address.number === null ? "" : `#${address.number} `}on GitHub
      </a>
    </Button>
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
