import type { PrState } from "@/api/generated/model";
import { badgeBase } from "@/components/ui/badge";
import { CircleCheck, CircleX, GitPullRequest } from "@/icons";
import { cn } from "@/lib/utils";

// The state of a pull-request link (`PrState`) in a list: an icon, a word and a colour. Open is the info colours of the design's
// "PR open" badge, merged the success colours, closed a quiet neutral. Not the badge a file wears (PrBadge says what a pull request
// means for ONE FILE: "Fix merged, awaiting sync"); this says what the pull request IS.

const OPEN = "border-info-border bg-info-bg text-info-fg";
const MERGED = "border-success-border bg-success-bg text-success-fg";
const CLOSED = "border-border bg-muted text-muted-foreground";

export function PrStateChip({ state }: { state: PrState }) {
  const [Icon, label, tone] = state === "open" ? ([GitPullRequest, "Open", OPEN] as const) : state === "merged" ? ([CircleCheck, "Merged", MERGED] as const) : ([CircleX, "Closed", CLOSED] as const);
  return (
    <span data-slot="pr-state" data-pr-state={state} className={cn(badgeBase, tone)}>
      <Icon aria-hidden="true" />
      {label}
    </span>
  );
}
