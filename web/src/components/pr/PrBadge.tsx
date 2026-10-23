import type { PrLink } from "@/api/generated/model";
import { badgeBase } from "@/components/ui/badge";
import type { PrBadgeKind } from "@/features/prs/badges";
import { GitPullRequest, Hourglass } from "@/icons";
import { cn } from "@/lib/utils";
import { prAddress } from "./pr-url";

// The two pull-request badges of the design (Consequence dialogs, frame 1l), beside a file's drift status:
//
//  - "PR #418 open" (`bd-pr`, the info colours): an open pull request carries this change; it links to the pull request;
//  - "Fix merged, awaiting sync" (`bd-merged`, dashed, the success colours): the pull request merged, and the swimlane has not
//    synced yet; "Clears on the next sync, no action needed". Not a link, as the design draws it.
//
// An icon, a word and a colour, never the colour alone. The address is the server's: the badge is a link only if the address is
// https with a host and no credentials (pr-url.ts), and the link leaves for GitHub in another tab with `rel="noopener noreferrer"`.
// `info` comes from `usePrBadges(swimlane).badgeFor(...)` (features/prs/use-pr-badges.ts); null draws nothing, or `empty` in the
// quiet words of a table cell ("No PR").

const OPEN = "border-info-border bg-info-bg text-info-fg";
const MERGED = "border-dashed border-success-border bg-transparent text-success-fg";

const OPEN_NOTE = "An open pull request carries this change. Clears when it merges or closes.";
const MERGED_NOTE = "The pull request merged, but the swimlane has not synced yet. Clears on the next sync, no action needed.";
const OUTDATED_NOTE = "This pull request was raised for an earlier version of this file.";

/** What a badge needs: `PrBadgeInfo` has it, and so can a pull request known only by its number and address (a finished job). */
export type PrBadgeFacts = { readonly kind: PrBadgeKind; readonly link: Pick<PrLink, "pr_number" | "pr_url">; readonly outdated: boolean };

export function PrBadge({ info, empty, className }: { info: PrBadgeFacts | null; empty?: string; className?: string }) {
  if (info === null) return empty === undefined ? null : <span className="text-xs text-muted-foreground">{empty}</span>;

  if (info.kind === "merged") {
    return (
      <span data-slot="pr-badge" data-pr-state="merged" title={MERGED_NOTE} className={cn(badgeBase, "max-w-full", MERGED, className)}>
        <Hourglass aria-hidden="true" />
        <span className="min-w-0 truncate">Fix merged, awaiting sync</span>
        <span className="sr-only">, PR #{info.link.pr_number} merged</span>
      </span>
    );
  }

  const classes = cn(badgeBase, "max-w-full", OPEN, className);
  const label = `PR #${info.link.pr_number} open`;
  const address = prAddress(info.link.pr_url);
  const note = info.outdated ? OUTDATED_NOTE : OPEN_NOTE;
  const body = (
    <>
      <GitPullRequest aria-hidden="true" />
      <span className="min-w-0 truncate">{label}</span>
    </>
  );
  if (address === null) {
    return (
      <span data-slot="pr-badge" data-pr-state="open" title={note} className={classes}>
        {body}
      </span>
    );
  }
  return (
    <a data-slot="pr-badge" data-pr-state="open" href={address.href} target="_blank" rel="noopener noreferrer" title={note} className={cn(classes, "no-underline hover:underline")}>
      {body}
      <span className="sr-only">{info.outdated ? ", on GitHub, raised for an earlier version of this file" : ", on GitHub"}</span>
    </a>
  );
}
