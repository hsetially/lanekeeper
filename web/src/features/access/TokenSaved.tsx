import { useId } from "react";
import type { GithubCredential } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { CircleCheck } from "@/icons";
import { formatTime } from "@/lib/time";
import { useNow } from "@/live/use-now";
import { AppLink } from "@/routes/AppLink";
import { expiryWords } from "./expiry";

/** When the token ends, as "2027-01-07 00:00 UTC · in 90 days", or that GitHub did not say. */
export function ExpiryText({ expiresAt, now }: { expiresAt: number | null | undefined; now: number }) {
  if (expiresAt === null || expiresAt === undefined) return <span className="text-muted-foreground">GitHub did not report an expiry</span>;
  return (
    <>
      {formatTime(expiresAt)} <span className="text-muted-foreground">· {expiryWords(expiresAt, now)}</span>
    </>
  );
}

/**
 * The design's success state of the onboarding (frames 1k and 1l): the token was verified and stored, and the only things shown
 * are the linked GitHub login and the expiry. The token itself is not here and cannot be shown again.
 */
export function TokenSaved({ credential }: { credential: GithubCredential }) {
  const titleId = useId();
  const now = useNow(false);
  return (
    <div role="status" aria-labelledby={titleId} className="flex flex-col gap-4 rounded-md border border-success-border bg-success-bg p-4">
      <div className="flex items-center gap-2 text-success-fg">
        <CircleCheck aria-hidden="true" className="size-4" />
        <strong id={titleId} className="text-base font-semibold">
          Token verified and saved
        </strong>
      </div>
      <dl className="m-0 grid grid-cols-[140px_minmax(0,1fr)] gap-x-3 gap-y-2">
        <dt className="text-muted-foreground">Linked GitHub login</dt>
        <dd className="m-0 font-mono text-mono font-medium">
          <SafeText text={credential.github_login} />
        </dd>
        <dt className="text-muted-foreground">Expires</dt>
        <dd className="m-0 tabular-nums">
          <ExpiryText expiresAt={credential.expires_at} now={now} />
        </dd>
      </dl>
      <span className="text-muted-foreground">
        The token itself is stored encrypted and can't be viewed again. To change it later, use{" "}
        <AppLink route="github-token" className="text-link underline underline-offset-2 hover:text-primary-hover">
          My GitHub token
        </AppLink>
        .
      </span>
      <Button asChild className="h-8 self-start px-3">
        <AppLink route="swimlanes">Continue to Lanekeeper</AppLink>
      </Button>
    </div>
  );
}
