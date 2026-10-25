import { useRef } from "react";
import type { PrLink } from "@/api/generated/model";
import { useSession } from "@/auth/session";
import { CopyButton } from "@/components/app/CopyButton";
import { COPY_GROUP, COPY_REVEAL } from "@/components/app/CopyValue";
import { SafeText } from "@/components/app/SafeText";
import { userLabel } from "@/components/attribution/model";
import { useActorNames } from "@/features/attribution/names";
import { GitPullRequest } from "@/icons";
import { formatRelative, formatTime } from "@/lib/time";
import { useNow } from "@/live/use-now";
import { prAddress, type PrAddress } from "./pr-url";

// One pull request, as the dialogs draw it (Consequence dialogs, frames 1f and 1i): its number, who opened it and when, the file it
// is for, and its address as a link with a copy button (copy inventory, row 25: shown on hover and focus, copying the same address
// the link opens). A link carries no title, no branch and no checks (`PrLink` has none), so none is drawn.

/** The address of a pull request as a link to GitHub, with its copy button: the text the link opens is the text that is copied. */
export function PrAddressLine({ address }: { address: PrAddress }) {
  const anchor = useRef<HTMLAnchorElement>(null);
  return (
    <span className={`${COPY_GROUP} inline-flex items-start gap-0.5`}>
      <a ref={anchor} href={address.href} target="_blank" rel="noopener noreferrer" className="text-sm break-all">
        <SafeText text={address.display} />
      </a>
      <CopyButton value={address.href} what="PR link" source={anchor} className={COPY_REVEAL} />
    </span>
  );
}

export function PrCard({ link }: { link: PrLink }) {
  const session = useSession();
  const { nameOf } = useActorNames();
  const now = useNow(true, 60_000);
  const address = prAddress(link.pr_url);
  const mine = session.status === "ready" && session.me.id === link.author;
  return (
    <div data-slot="pr-card" className="flex items-start gap-2 rounded-md border border-border p-3">
      <GitPullRequest aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-info-fg" />
      <div className="flex min-w-0 flex-col gap-1">
        <b className="font-semibold">Pull request #{link.pr_number}</b>
        <span className="text-xs text-muted-foreground">
          #{link.pr_number} · opened by {mine ? "you" : <SafeText text={userLabel(link.author, { nameOf })} />} <span title={formatTime(link.created_at)}>{formatRelative(link.created_at, now)}</span>
        </span>
        <span className="font-mono text-xs break-all">
          <SafeText text={link.path} />
        </span>
        {address === null ? (
          <span className="text-xs text-muted-foreground">The server gave no address that can be opened. Find the pull request in the list.</span>
        ) : (
          <PrAddressLine address={address} />
        )}
      </div>
    </div>
  );
}
